//! Video that reaches the glass without a GPUI frame.
//!
//! A [`VideoLayer`] is a `CAMetalLayer` attached to a
//! [`NativeHost`](gpui::composition::NativeHost). Whoever decodes the video hands each
//! picture to it from any thread; the layer's own thread draws it with GPUI's surface shader
//! (the Y'CbCr matrix, range, bit depth and chroma layout the buffer carries) into a drawable
//! the size of the picture, which Core Animation scales to the layer. GPUI's frame only
//! places the layer, like any native, so a picture costs the main thread nothing and waits
//! for no GPUI frame, and GPUI draws nothing while only the video changes.
//!
//! Pictures never queue in front of the display. At most one drawn picture is on its way to
//! the glass; a picture handed over meanwhile waits in a one-picture mailbox and is drawn
//! the moment the one before it is shown, and a newer picture replaces it there (it is
//! reported as dropped). A decoder that runs a little faster than the display, as a 60 Hz
//! stream does on a 59.94 Hz panel, otherwise fills the drawable queue and adds up to two
//! refreshes to every picture.
//!
//! Threads: the layer's thread writes the layer's drawables and its `drawableSize`, which it
//! alone knows; the main thread writes only the geometry, through the host's container, in
//! GPUI's frame transaction. `docs/composition.md` has the measurements.

use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use core_video::pixel_buffer::CVPixelBuffer;
use gpui::{
    Bounds, ContentMask, PaintSurface, PresentedFrame, ScaledPixels, Scene,
    composition::NativeHost, point, size,
};
use objc::{class, msg_send, runtime::YES, sel, sel_impl};
use parking_lot::{Condvar, Mutex};

use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

/// How long a drawn picture may go unreported before the next one is drawn anyway: the iOS
/// simulator's drawables never report, and a report may be lost when the layer leaves the
/// screen.
const UNREPORTED_AFTER: Duration = Duration::from_millis(50);

/// Receives what became of each picture handed to [`VideoLayer::present`], by the sequence
/// number `present` returned, on a Metal thread or the presenting one. A picture replaced
/// in the mailbox before it was drawn is reported with no `presented_at`.
pub type VideoPresentedSink = Arc<dyn Fn(u64, PresentedFrame) + Send + Sync>;

/// How a [`VideoLayer`] presents.
#[derive(Clone, Debug)]
pub struct VideoLayerOptions {
    /// Drawables the layer keeps (2 or 3). With one picture at most on its way to the glass,
    /// two suffice; the third is spare for a picture drawn while one is still on screen.
    pub maximum_drawable_count: u64,
}

impl Default for VideoLayerOptions {
    fn default() -> Self {
        Self {
            maximum_drawable_count: 3,
        }
    }
}

/// A video layer: hand it pictures from any thread.
///
/// Cloning is cheap; every clone presents to the same layer. The layer's thread ends when
/// the last clone is dropped.
#[derive(Clone)]
pub struct VideoLayer {
    handle: Arc<Handle>,
}

/// Stops the layer's thread when the last [`VideoLayer`] goes.
struct Handle {
    shared: Arc<Shared>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.shared.state.lock().stopped = true;
        self.shared.wake.notify_all();
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    sink: Mutex<Option<VideoPresentedSink>>,
}

/// A picture on its way from the decoding thread to the layer's.
struct Picture(CVPixelBuffer);

// SAFETY: a CVPixelBuffer is a CoreFoundation object, whose retain and release are
// thread-safe, and the layer only reads it: CoreVideo lets any thread sample a buffer.
unsafe impl Send for Picture {}

#[derive(Default)]
struct State {
    next_sequence: u64,
    /// The newest picture not drawn yet.
    mailbox: Option<(u64, Picture)>,
    /// Pictures drawn and not yet reported shown or dropped, oldest first, with when each
    /// was drawn.
    in_flight: VecDeque<(u64, Instant)>,
    stopped: bool,
}

impl Shared {
    fn report(&self, sequence: u64, frame: PresentedFrame) {
        let sink = self.sink.lock().clone();
        if let Some(sink) = sink {
            sink(sequence, frame);
        }
    }
}

/// What the layer's thread draws with.
struct VideoRenderer {
    renderer: MetalRenderer,
    scene: Scene,
    drawable_size: (usize, usize),
}

// SAFETY: the renderer holds Metal objects (device, queue, pipelines, buffers), which Metal
// documents as safe to use from any thread, a `CVMetalTextureCache`, and the layer, whose
// drawables and `drawableSize` Core Animation lets any one thread at a time drive. It moves
// once, to the layer's thread, and is used there alone.
unsafe impl Send for VideoRenderer {}

impl VideoLayer {
    /// Creates a video layer and attaches it to `host`, whose element places it. Main thread.
    pub fn attach(host: &NativeHost, options: VideoLayerOptions) -> Result<Self> {
        let mut renderer =
            MetalRenderer::new(Arc::new(Mutex::new(InstanceBufferPool::default())), false);
        let Some(layer) = renderer.layer() else {
            bail!("the video renderer has no layer");
        };
        layer.set_maximum_drawable_count(options.maximum_drawable_count.clamp(2, 3));
        let layer = renderer.layer_ptr().cast::<objc::runtime::Object>();
        // SAFETY: a new, live CAMetalLayer, configured and attached on the main thread; the
        // host's container retains it and the renderer keeps its own reference.
        unsafe {
            // Stretched to the layer: the element places the layer where the picture goes.
            let () = msg_send![layer, setNeedsDisplayOnBoundsChange: objc::runtime::NO];
            host.attach_layer(
                std::ptr::NonNull::new(layer.cast()).context("the video renderer's layer")?,
            )?;
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            sink: Mutex::new(None),
        });
        renderer.set_presented_frame_sink(Some(Arc::new({
            let shared = Arc::downgrade(&shared);
            move |frame| {
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                let sequence = shared.state.lock().in_flight.pop_front();
                shared.wake.notify_all();
                if let Some((sequence, _)) = sequence {
                    shared.report(sequence, frame);
                }
            }
        })));
        let video = VideoRenderer {
            renderer,
            scene: Scene::default(),
            drawable_size: (0, 0),
        };
        std::thread::Builder::new()
            .name("gpui video layer".into())
            .spawn({
                let shared = shared.clone();
                move || draw_pictures(&shared, video)
            })
            .context("start the video layer's thread")?;
        Ok(Self {
            handle: Arc::new(Handle { shared }),
        })
    }

    /// Hands `picture` to the layer, from any thread, and returns its sequence number for
    /// [`Self::on_presented`]. Never blocks on the display: a picture not drawn yet is
    /// replaced by this one.
    pub fn present(&self, picture: &CVPixelBuffer) -> Result<u64> {
        if picture.get_width() == 0 || picture.get_height() == 0 {
            bail!("an empty picture");
        }
        let shared = &self.handle.shared;
        let (sequence, replaced) = {
            let mut state = shared.state.lock();
            let sequence = state.next_sequence;
            state.next_sequence += 1;
            let replaced = state.mailbox.replace((sequence, Picture(picture.clone())));
            (sequence, replaced)
        };
        shared.wake.notify_all();
        if let Some((replaced, _)) = replaced {
            shared.report(
                replaced,
                PresentedFrame {
                    submitted_at: Instant::now(),
                    presented_at: None,
                },
            );
        }
        Ok(sequence)
    }

    /// Reports what became of every picture to `sink`; `None` stops. The iOS simulator's
    /// drawables report nothing.
    pub fn on_presented(&self, sink: Option<VideoPresentedSink>) {
        *self.handle.shared.sink.lock() = sink;
    }
}

/// The layer's thread: draws the mailbox's picture whenever no drawn picture is still on
/// its way to the glass.
fn draw_pictures(shared: &Shared, mut video: VideoRenderer) {
    loop {
        let (sequence, picture) = {
            let mut state = shared.state.lock();
            loop {
                if state.stopped {
                    return;
                }
                if let Some(&(_, drawn_at)) = state.in_flight.front()
                    && drawn_at.elapsed() >= UNREPORTED_AFTER
                {
                    state.in_flight.clear();
                }
                if state.mailbox.is_some() && state.in_flight.is_empty() {
                    break;
                }
                if state.in_flight.is_empty() {
                    shared.wake.wait(&mut state);
                } else {
                    shared.wake.wait_for(&mut state, UNREPORTED_AFTER);
                }
            }
            let Some((sequence, picture)) = state.mailbox.take() else {
                continue;
            };
            state.in_flight.push_back((sequence, Instant::now()));
            (sequence, picture)
        };
        let drawn = objc2::rc::autoreleasepool(|_| video.draw(&picture.0));
        if drawn.is_err() {
            shared
                .state
                .lock()
                .in_flight
                .retain(|(drawn, _)| *drawn != sequence);
        }
    }
}

impl VideoRenderer {
    fn draw(&mut self, picture: &CVPixelBuffer) -> Result<()> {
        let (width, height) = (picture.get_width(), picture.get_height());
        if self.drawable_size != (width, height) {
            let Some(layer) = self.renderer.layer() else {
                bail!("the video renderer has no layer");
            };
            // SAFETY: an explicit transaction on this thread, as Core Animation requires of
            // a thread without a run loop; `drawableSize` is not animatable, and actions are
            // off.
            unsafe {
                let () = msg_send![class!(CATransaction), begin];
                let () = msg_send![class!(CATransaction), setDisableActions: YES];
            }
            layer.set_drawable_size(core_graphics::geometry::CGSize::new(
                width as f64,
                height as f64,
            ));
            // SAFETY: closes the transaction begun above.
            unsafe {
                let () = msg_send![class!(CATransaction), commit];
            }
            self.drawable_size = (width, height);
        }
        // Takes in the renderer's own presentation records so they do not pile up.
        self.renderer.ready_for_vsync_frame();
        let bounds = Bounds::new(
            point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(width as f32), ScaledPixels(height as f32)),
        );
        self.scene.clear();
        self.scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            image_buffer: picture.clone(),
        });
        self.scene.finish();
        self.renderer.draw(&self.scene);
        Ok(())
    }
}
