//! What composing native views into a GPUI window costs on the way to the glass
//! (macOS). It opens one non-activating panel over the other windows, without taking
//! focus, prints one line of numbers and quits.
//!
//! ```text
//! cargo run -p gpui_perf --example composition_latency --release -- <variant> <mode>
//! ```
//!
//! Variants, each with a solid-colour layer-backed `NSView` under the GPUI view:
//!
//! - `today`: GPUI's layer as it is, opaque; the native is hidden under it.
//! - `nonopaque`: GPUI's layer marked non-opaque, so the window server blends it over
//!   what is under it (the cost of the underlay, measured without a hole).
//! - `band`: a second full-window transparent `CAMetalLayer` above GPUI's, cleared and
//!   presented with every GPUI frame (the cost of zed#62379's overlay band).
//! - `holes`: the native placed with `native_view`, GPUI cutting its hole.
//! - `moving`: the same native moving by a point every frame, so every frame is
//!   presented inside a Core Animation transaction with the native's new frame.
//!
//! Modes:
//!
//! - `idle`: a single frame after a quarter second of idle, from another thread's
//!   notify, as a keystroke in a quiet terminal. `wake_to_glass` is from the notify to
//!   the glass.
//! - `continuous`: a frame every refresh; `submit_to_glass` is from the frame's
//!   submission to the GPU to the glass. `COMPOSITION_FRAMES` sets how many frames are
//!   measured (900 by default), long enough to record the window server meanwhile.

extern crate gpui_fast as gpui;
extern crate gpui_platform_fast as gpui_platform;

#[cfg(target_os = "macos")]
fn main() {
    macos::main();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("composition_latency measures macOS composition");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        cell::RefCell,
        collections::VecDeque,
        rc::Rc,
        time::{Duration, Instant},
    };

    use gpui::{
        App, Bounds, Context, PresentedFrame, Subscription, Window, WindowBounds, WindowOptions,
        composition::{NativeHost, NativeHostOptions, native_view},
        div,
        prelude::*,
        px, rgb, size,
    };
    use gpui_platform::application;
    use objc::{
        class, msg_send,
        runtime::{NO, Object, YES},
        sel, sel_impl,
    };
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

    const WARMUP_FRAMES: usize = 60;
    const IDLE_SAMPLES: usize = 40;
    const IDLE_GAP: Duration = Duration::from_millis(250);

    #[derive(Clone, Copy, PartialEq)]
    enum Variant {
        Today,
        NonOpaque,
        Band,
        Holes,
        Moving,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Idle,
        Continuous,
    }

    #[derive(Default)]
    struct Stats {
        seen: usize,
        submit_to_glass: Vec<f64>,
        wake_to_glass: Vec<f64>,
        presented_at: Vec<Instant>,
        dropped: usize,
        wake: Option<Instant>,
        started: VecDeque<Instant>,
        /// The main thread's CPU time at the start of each frame's render.
        cpu_marks: Vec<Duration>,
    }

    /// A second full-window transparent Metal layer, presented with every GPUI frame.
    struct Band {
        layer: metal::MetalLayer,
        queue: metal::CommandQueue,
    }

    impl Band {
        fn present(&self) {
            objc2::rc::autoreleasepool(|_| {
                let Some(drawable) = self.layer.next_drawable() else {
                    return;
                };
                let descriptor = metal::RenderPassDescriptor::new();
                let attachment = descriptor.color_attachments().object_at(0).unwrap();
                attachment.set_texture(Some(drawable.texture()));
                attachment.set_load_action(metal::MTLLoadAction::Clear);
                attachment.set_clear_color(metal::MTLClearColor::new(0., 0., 0., 0.));
                attachment.set_store_action(metal::MTLStoreAction::Store);
                let command_buffer = self.queue.new_command_buffer();
                command_buffer
                    .new_render_command_encoder(descriptor)
                    .end_encoding();
                command_buffer.present_drawable(drawable);
                command_buffer.commit();
            });
        }
    }

    struct Probe {
        variant: Variant,
        mode: Mode,
        tick: u64,
        host: Option<NativeHost>,
        band: Option<Band>,
        stats: Rc<RefCell<Stats>>,
        _presented: Subscription,
    }

    /// Main-thread CPU time used so far.
    fn thread_cpu() -> Duration {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `clock_gettime` writes the current thread's CPU clock into `time`.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
        Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
    }

    /// An `NSView` whose layer is a solid colour, sized to `frame`.
    ///
    /// # Safety
    ///
    /// Main thread only, as AppKit requires of views.
    unsafe fn solid_view(frame: cocoa::foundation::NSRect) -> *mut Object {
        // SAFETY: AppKit view creation and layer configuration on the main thread; the
        // colour is autoreleased and retained by the layer.
        unsafe {
            let view: *mut Object = msg_send![class!(NSView), alloc];
            let view: *mut Object = msg_send![view, initWithFrame: frame];
            let () = msg_send![view, setWantsLayer: YES];
            let layer: *mut Object = msg_send![view, layer];
            let color: *mut Object = msg_send![class!(NSColor), systemTealColor];
            let cg_color: *mut Object = msg_send![color, CGColor];
            let () = msg_send![layer, setBackgroundColor: cg_color];
            view
        }
    }

    impl Probe {
        fn new(variant: Variant, mode: Mode, window: &mut Window, cx: &mut Context<Self>) -> Self {
            let stats = Rc::new(RefCell::new(Stats::default()));
            let frames = std::env::var("COMPOSITION_FRAMES")
                .ok()
                .and_then(|frames| frames.parse().ok())
                .unwrap_or(900);
            let presented = window.on_frame_presented({
                let stats = stats.clone();
                move |frame, _, cx| {
                    if record(&mut stats.borrow_mut(), mode, frames, frame) {
                        report(variant, mode, &stats.borrow());
                        cx.quit();
                    }
                }
            });
            let mut band = None;
            let mut host = None;
            let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else {
                unreachable!("an AppKit window")
            };
            let gpui_view = handle.ns_view.as_ptr().cast::<Object>();
            // SAFETY: the GPUI view is live, and this is the main thread, where AppKit and
            // Core Animation objects of a window are used.
            unsafe {
                let content: *mut Object = msg_send![gpui_view, superview];
                let bounds: cocoa::foundation::NSRect = msg_send![content, bounds];
                match variant {
                    Variant::Today | Variant::NonOpaque | Variant::Band => {
                        let native = solid_view(cocoa::foundation::NSRect::new(
                            cocoa::foundation::NSPoint::new(200., 150.),
                            cocoa::foundation::NSSize::new(400., 300.),
                        ));
                        let () = msg_send![content, addSubview: native positioned: -1isize relativeTo: gpui_view];
                        let () = msg_send![native, release];
                    }
                    Variant::Holes | Variant::Moving => {
                        let native_host = window
                            .create_native_host(NativeHostOptions::default(), cx)
                            .expect("macOS composes natives");
                        let native = solid_view(bounds);
                        native_host
                            .attach_view(std::ptr::NonNull::new(native.cast()).unwrap())
                            .expect("attach the native");
                        let () = msg_send![native, release];
                        host = Some(native_host);
                    }
                }
                if variant == Variant::NonOpaque {
                    let layer: *mut Object = msg_send![gpui_view, layer];
                    let () = msg_send![layer, setOpaque: NO];
                }
                if variant == Variant::Band {
                    let device = metal::Device::system_default().expect("a Metal device");
                    let layer = metal::MetalLayer::new();
                    layer.set_device(&device);
                    layer.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
                    layer.set_opaque(false);
                    layer.set_maximum_drawable_count(3);
                    let backing: f64 = {
                        let window: *mut Object = msg_send![gpui_view, window];
                        msg_send![window, backingScaleFactor]
                    };
                    layer.set_contents_scale(backing);
                    layer.set_drawable_size(core_graphics::geometry::CGSize::new(
                        bounds.size.width * backing,
                        bounds.size.height * backing,
                    ));
                    let view: *mut Object = msg_send![class!(NSView), alloc];
                    let view: *mut Object = msg_send![view, initWithFrame: bounds];
                    let () = msg_send![view, setLayer: layer.as_ref()];
                    let () = msg_send![view, setWantsLayer: YES];
                    let () = msg_send![content, addSubview: view positioned: 1isize relativeTo: gpui_view];
                    let () = msg_send![view, release];
                    band = Some(Band {
                        layer,
                        queue: device.new_command_queue(),
                    });
                }
            }
            if std::env::var_os("COMPOSITION_DEBUG").is_some() {
                let view = gpui_view as usize;
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    // SAFETY: a debug probe of the live window, on the main thread.
                    unsafe {
                        let view = view as *mut Object;
                        let window: *mut Object = msg_send![view, window];
                        let occlusion: usize = msg_send![window, occlusionState];
                        let visible: bool = msg_send![window, isVisible];
                        let active_space: bool = msg_send![window, isOnActiveSpace];
                        let frame: cocoa::foundation::NSRect = msg_send![window, frame];
                        let level: isize = msg_send![window, level];
                        eprintln!(
                            "occlusion={occlusion:#x} visible={visible} active_space={active_space} level={level} frame={:?}",
                            (frame.origin.x, frame.origin.y, frame.size.width, frame.size.height)
                        );
                    }
                })
                .detach();
            }
            if mode == Mode::Idle {
                // Wakes come from another thread at a phase that walks across the refresh,
                // as keystrokes do.
                let (wakes, mut woken) = futures::channel::mpsc::unbounded();
                std::thread::spawn(move || {
                    for step in 0u32.. {
                        std::thread::sleep(
                            IDLE_GAP + Duration::from_micros(u64::from(step % 16) * 830),
                        );
                        if wakes.unbounded_send(Instant::now()).is_err() {
                            break;
                        }
                    }
                });
                let stats = stats.clone();
                cx.spawn(async move |this, cx| {
                    while let Some(wake) = futures::StreamExt::next(&mut woken).await {
                        stats.borrow_mut().wake = Some(wake);
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                })
                .detach();
            }
            Self {
                variant,
                mode,
                tick: 0,
                host,
                band,
                stats,
                _presented: presented,
            }
        }
    }

    /// Returns whether the run has all its samples.
    fn record(stats: &mut Stats, mode: Mode, frames: usize, frame: PresentedFrame) -> bool {
        stats.seen += 1;
        if std::env::var_os("COMPOSITION_DEBUG").is_some() {
            eprintln!("frame {} {:?}", stats.seen, frame.presented_at.is_some());
        }
        while stats
            .started
            .front()
            .is_some_and(|started| *started <= frame.submitted_at)
        {
            stats.started.pop_front();
        }
        let warmup = if mode == Mode::Idle { 2 } else { WARMUP_FRAMES };
        if stats.seen <= warmup {
            return false;
        }
        let Some(presented_at) = frame.presented_at else {
            stats.dropped += 1;
            return false;
        };
        let ms = |from: Instant| presented_at.saturating_duration_since(from).as_secs_f64() * 1e3;
        match mode {
            Mode::Continuous => {
                stats.submit_to_glass.push(ms(frame.submitted_at));
                stats.presented_at.push(presented_at);
                stats.submit_to_glass.len() >= frames
            }
            Mode::Idle => {
                if let Some(wake) = stats.wake.take_if(|wake| *wake <= frame.submitted_at) {
                    stats.submit_to_glass.push(ms(frame.submitted_at));
                    stats.wake_to_glass.push(ms(wake));
                }
                stats.wake_to_glass.len() >= IDLE_SAMPLES
            }
        }
    }

    fn summary(label: &str, samples: &[f64]) -> String {
        if samples.is_empty() {
            return format!("{label}_ms none");
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        format!(
            "{label}_ms p50={:.2} p95={:.2} max={:.2}",
            at(0.5),
            at(0.95),
            at(1.0)
        )
    }

    fn report(variant: Variant, mode: Mode, stats: &Stats) {
        let variant = match variant {
            Variant::Today => "today",
            Variant::NonOpaque => "nonopaque",
            Variant::Band => "band",
            Variant::Holes => "holes",
            Variant::Moving => "moving",
        };
        let mode_name = match mode {
            Mode::Idle => "idle",
            Mode::Continuous => "continuous",
        };
        let mut line = format!(
            "variant={variant} mode={mode_name} frames={} dropped={}",
            stats.submit_to_glass.len(),
            stats.dropped
        );
        if mode == Mode::Continuous {
            let mut gaps: Vec<f64> = stats
                .presented_at
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).as_secs_f64() * 1e3)
                .collect();
            gaps.sort_by(f64::total_cmp);
            let refresh = gaps[gaps.len() / 2];
            let missed: u64 = gaps
                .iter()
                .map(|gap| ((gap / refresh).round() as u64).saturating_sub(1))
                .sum();
            line += &format!(" refresh_ms={refresh:.2} missed={missed} ");
            let main_cpu: Vec<f64> = stats
                .cpu_marks
                .windows(2)
                .skip(WARMUP_FRAMES)
                .map(|pair| (pair[1] - pair[0]).as_secs_f64() * 1e3)
                .collect();
            line += &summary("main_cpu_per_frame", &main_cpu);
        }
        line += " ";
        line += &summary("submit_to_glass", &stats.submit_to_glass);
        if mode == Mode::Idle {
            line += " ";
            line += &summary("wake_to_glass", &stats.wake_to_glass);
        }
        println!("{line}");
    }

    impl Render for Probe {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.tick += 1;
            {
                let mut stats = self.stats.borrow_mut();
                stats.started.push_back(Instant::now());
                stats.cpu_marks.push(thread_cpu());
            }
            if self.mode == Mode::Continuous {
                window.request_animation_frame();
            }
            if let Some(band) = &self.band {
                band.present();
            }
            let offset = px((self.tick % 400) as f32);
            let moving = if self.variant == Variant::Moving {
                px((self.tick % 200) as f32)
            } else {
                px(0.)
            };
            div()
                .relative()
                .size_full()
                .bg(rgb(0x202020))
                .when_some(self.host.as_ref(), |this, host| {
                    this.child(
                        native_view(host)
                            .absolute()
                            .top(px(150.) + moving / 4.)
                            .left(px(200.) + moving)
                            .w(px(400.))
                            .h(px(300.))
                            .rounded(px(12.)),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .top(px(8.))
                        .left(offset)
                        .size(px(40.))
                        .bg(rgb(0xff8000)),
                )
        }
    }

    pub fn main() {
        let mut args = std::env::args().skip(1);
        let variant = match args.next().as_deref() {
            Some("today") | None => Variant::Today,
            Some("nonopaque") => Variant::NonOpaque,
            Some("band") => Variant::Band,
            Some("holes") => Variant::Holes,
            Some("moving") => Variant::Moving,
            Some(other) => {
                eprintln!("unknown variant {other}: today, nonopaque, band, holes or moving");
                std::process::exit(2);
            }
        };
        let mode = match args.next().as_deref() {
            Some("idle") | None => Mode::Idle,
            Some("continuous") => Mode::Continuous,
            Some(other) => {
                eprintln!("unknown mode {other}: idle or continuous");
                std::process::exit(2);
            }
        };
        application().run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(900.), px(700.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Full rate while another app keeps focus; the probe never takes it. A
                    // non-activating panel above other windows on every Space is on the glass
                    // without taking the keyboard from whatever the user is doing.
                    inactive_frame_interval: None,
                    focus: false,
                    kind: gpui::WindowKind::PopUp,
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Probe::new(variant, mode, window, cx)),
            )
            .expect("open the probe window");
        });
    }
}
