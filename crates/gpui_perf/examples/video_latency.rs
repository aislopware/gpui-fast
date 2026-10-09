//! Decoded picture to glass (macOS): a video shown through GPUI's `surface` element, which
//! costs a GPUI frame per picture, against a `VideoLayer` handed pictures on the decoding
//! thread, which costs none. It opens one non-activating panel over the other windows,
//! without taking focus, prints its numbers and quits.
//!
//! ```text
//! GPUI_PRESENTED_AT_CALLBACK=1 cargo run -p gpui_perf --example video_latency --release -- <surface|layer|both>
//! ```
//!
//! A producer thread stands in for the decoder: every 1/60 s it writes a 1920×1080 4:2:0
//! picture into the next buffer of a small pool and hands it on. `decoded_to_glass` is from
//! the hand-off to the glass; `dropped` counts pictures never shown (replaced by a newer one
//! first). `gpui_frames_per_s` is how many frames the GPUI window drew and
//! `main_cpu_per_picture` the main thread's CPU time per picture. `both` shows the same
//! pictures both ways side by side, so the two latencies are compared picture by picture
//! (`layer_minus_surface`) rather than across runs, which the producer's clock drifting
//! against the display's makes noisy. `VIDEO_PICTURES` sets how many pictures are measured
//! (600 by default); `VIDEO_BUILD_MS` makes every GPUI frame spend that long on the main
//! thread first, as a window with more in it than one picture does.

extern crate gpui_fast as gpui;
extern crate gpui_platform_fast as gpui_platform;

#[cfg(target_os = "macos")]
fn main() {
    macos::main();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("video_latency measures macOS video");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        cell::RefCell,
        collections::{HashMap, VecDeque},
        rc::Rc,
        sync::Arc,
        time::{Duration, Instant},
    };

    use core_foundation::{base::TCFType, dictionary::CFDictionary, string::CFString};
    use core_video::pixel_buffer::{
        CVPixelBuffer, kCVPixelBufferIOSurfacePropertiesKey,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    };
    use gpui::{
        App, Bounds, Context, PresentedFrame, Subscription, Window, WindowBounds, WindowKind,
        WindowOptions,
        composition::{NativeHost, NativeHostOptions, native_view},
        div,
        prelude::*,
        px, size, surface,
    };
    use gpui_apple::fast::video_layer::{VideoLayer, VideoLayerOptions};
    use gpui_platform::application;
    use parking_lot::Mutex;

    const WARMUP: usize = 60;
    const POOL: usize = 8;
    const WIDTH: usize = 1920;
    const HEIGHT: usize = 1080;
    const PERIOD: Duration = Duration::from_micros(16_667);

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Surface,
        Layer,
        Both,
    }

    impl Mode {
        fn surface(self) -> bool {
            self != Self::Layer
        }

        fn layer(self) -> bool {
            self != Self::Surface
        }
    }

    /// A picture on its way from the producer thread to the main thread.
    struct Picture {
        index: usize,
        buffer: CVPixelBuffer,
    }

    // SAFETY: a CVPixelBuffer is a CoreFoundation object, whose retain and release are
    // thread-safe; the producer no longer writes a buffer it handed on until the pool comes
    // round to it again, POOL pictures later.
    unsafe impl Send for Picture {}

    /// Decoded-to-glass per picture index, in milliseconds, `None` for a picture never shown.
    #[derive(Default)]
    struct Results {
        decoded_at: HashMap<usize, Instant>,
        surface: HashMap<usize, Option<f64>>,
        layer: HashMap<usize, Option<f64>>,
    }

    impl Results {
        fn record(&mut self, layer: bool, index: usize, frame: PresentedFrame) {
            let Some(decoded_at) = self.decoded_at.get(&index) else {
                return;
            };
            let latency = frame
                .presented_at
                .map(|presented_at| (presented_at - *decoded_at).as_secs_f64() * 1e3);
            if layer {
                self.layer.insert(index, latency);
            } else {
                self.surface.insert(index, latency);
            }
        }
    }

    struct Player {
        mode: Mode,
        host: Option<NativeHost>,
        _video: Option<VideoLayer>,
        picture: Option<CVPixelBuffer>,
        /// The index of each drawn frame's picture, oldest first.
        drawn: Rc<RefCell<VecDeque<usize>>>,
        pending: Option<usize>,
        /// Main-thread time every frame spends before it is built.
        build: Duration,
        _presented: Subscription,
    }

    fn thread_cpu() -> Duration {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `clock_gettime` writes the current thread's CPU clock into `time`.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
        Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
    }

    fn pool() -> Vec<CVPixelBuffer> {
        // SAFETY: CoreVideo's constant key string lives forever.
        let key = unsafe { CFString::wrap_under_get_rule(kCVPixelBufferIOSurfacePropertiesKey) };
        let empty = CFDictionary::<CFString, CFString>::from_CFType_pairs(&[]);
        let options = CFDictionary::from_CFType_pairs(&[(key, empty.as_CFType())]);
        (0..POOL)
            .map(|_| {
                CVPixelBuffer::new(
                    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                    WIDTH,
                    HEIGHT,
                    Some(&options),
                )
                .expect("an IOSurface-backed 4:2:0 buffer")
            })
            .collect()
    }

    /// Writes picture `index` into `buffer`: grey, with a bright bar that moves along.
    fn decode(buffer: &CVPixelBuffer, index: usize) {
        buffer.lock_base_address(0);
        // SAFETY: the base address is locked; plane 0 spans `bytes per row × height`, and
        // plane 1 (interleaved chroma) `bytes per row × height / 2`.
        unsafe {
            let raw = buffer.as_concrete_TypeRef();
            for plane in 0..2 {
                let base = core_video::pixel_buffer::CVPixelBufferGetBaseAddressOfPlane(raw, plane)
                    .cast::<u8>();
                let stride =
                    core_video::pixel_buffer::CVPixelBufferGetBytesPerRowOfPlane(raw, plane);
                let rows = buffer.get_height_of_plane(plane);
                let bytes = std::slice::from_raw_parts_mut(base, stride * rows);
                if plane == 0 {
                    bytes.fill(100);
                    let bar = (index * 8) % (WIDTH - 32);
                    for row in bytes.chunks_exact_mut(stride) {
                        row[bar..bar + 32].fill(235);
                    }
                } else {
                    bytes.fill(128);
                }
            }
        }
        buffer.unlock_base_address(0);
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

    fn path_line(name: &str, results: &HashMap<usize, Option<f64>>, measured: usize) -> String {
        let measured: Vec<Option<f64>> = (WARMUP..WARMUP + measured)
            .map(|index| results.get(&index).copied().flatten())
            .collect();
        let shown: Vec<f64> = measured.iter().flatten().copied().collect();
        format!(
            "path={name} shown={} dropped={} {}",
            shown.len(),
            measured.len() - shown.len(),
            summary("decoded_to_glass", &shown)
        )
    }

    struct Run {
        mode: Mode,
        measured: usize,
        results: Arc<Mutex<Results>>,
        gpui_frames: RefCell<usize>,
    }

    impl Run {
        fn report(&self, seconds: f64, main_cpu: Duration) {
            let results = self.results.lock();
            let mode = match self.mode {
                Mode::Surface => "surface",
                Mode::Layer => "layer",
                Mode::Both => "both",
            };
            let mut line = format!("mode={mode}");
            if self.mode.surface() {
                line += &format!(" {}", path_line("surface", &results.surface, self.measured));
            }
            if self.mode.layer() {
                line += &format!(" {}", path_line("layer", &results.layer, self.measured));
            }
            if self.mode == Mode::Both {
                let paired: Vec<f64> = (WARMUP..WARMUP + self.measured)
                    .filter_map(|index| {
                        Some(
                            results.layer.get(&index).copied().flatten()?
                                - results.surface.get(&index).copied().flatten()?,
                        )
                    })
                    .collect();
                line += &format!(" {}", summary("layer_minus_surface", &paired));
            }
            line += &format!(
                " gpui_frames_per_s={:.1} main_cpu_per_picture_ms={:.3}",
                *self.gpui_frames.borrow() as f64 / seconds,
                main_cpu.as_secs_f64() * 1e3 / self.measured as f64
            );
            println!("{line}");
        }
    }

    impl Player {
        fn new(mode: Mode, window: &mut Window, cx: &mut Context<Self>, run: Rc<Run>) -> Self {
            let drawn: Rc<RefCell<VecDeque<usize>>> = Rc::default();
            let presented = window.on_frame_presented({
                let run = run.clone();
                let drawn = drawn.clone();
                move |frame, _, _| {
                    *run.gpui_frames.borrow_mut() += 1;
                    if let Some(index) = drawn.borrow_mut().pop_front() {
                        run.results.lock().record(false, index, frame);
                    }
                }
            });
            let host = mode.layer().then(|| {
                window
                    .create_native_host(NativeHostOptions::default(), cx)
                    .expect("macOS composes natives")
            });
            let video = host.as_ref().map(|host| {
                let video =
                    VideoLayer::attach(host, VideoLayerOptions::default()).expect("a video layer");
                let sequences: Arc<Mutex<HashMap<u64, usize>>> = Arc::default();
                video.on_presented(Some(Arc::new({
                    let sequences = sequences.clone();
                    let results = run.results.clone();
                    move |sequence, frame| {
                        if let Some(index) = sequences.lock().remove(&sequence) {
                            results.lock().record(true, index, frame);
                        }
                    }
                })));
                (video, sequences)
            });
            let (pictures, mut received) = futures::channel::mpsc::unbounded::<Picture>();
            let total = WARMUP + run.measured;
            std::thread::spawn({
                let results = run.results.clone();
                let video = video.clone();
                move || {
                    let buffers = pool();
                    let mut next = Instant::now() + PERIOD * 30;
                    for index in 0..total {
                        std::thread::sleep(next.saturating_duration_since(Instant::now()));
                        next += PERIOD;
                        let buffer = &buffers[index % POOL];
                        decode(buffer, index);
                        results.lock().decoded_at.insert(index, Instant::now());
                        if let Some((video, sequences)) = &video {
                            // A picture is reported shown a refresh later at the soonest, or
                            // replaced by a later `present` on this thread: after this insert.
                            let sequence = video.present(buffer).expect("present a picture");
                            sequences.lock().insert(sequence, index);
                        }
                        if mode.surface() {
                            let picture = Picture {
                                index,
                                buffer: buffer.clone(),
                            };
                            if pictures.unbounded_send(picture).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            if mode.surface() {
                cx.spawn(async move |this, cx| {
                    while let Some(picture) = futures::StreamExt::next(&mut received).await {
                        let updated = this.update(cx, |player, cx| {
                            player.picture = Some(picture.buffer);
                            player.pending = Some(picture.index);
                            cx.notify();
                        });
                        if updated.is_err() {
                            break;
                        }
                    }
                })
                .detach();
            }
            // Frames and main-thread CPU are counted from the end of the warm-up to the end.
            cx.spawn(async move |_, cx| {
                let executor = cx.background_executor().clone();
                executor.timer(PERIOD * (30 + WARMUP as u32)).await;
                *run.gpui_frames.borrow_mut() = 0;
                let (start, start_cpu) = (Instant::now(), thread_cpu());
                executor
                    .timer(PERIOD * run.measured as u32 + Duration::from_millis(300))
                    .await;
                run.report(start.elapsed().as_secs_f64(), thread_cpu() - start_cpu);
                cx.update(|cx| cx.quit());
            })
            .detach();
            Self {
                mode,
                host,
                _video: video.map(|(video, _)| video),
                picture: None,
                drawn,
                pending: None,
                build: Duration::from_secs_f64(
                    std::env::var("VIDEO_BUILD_MS")
                        .ok()
                        .and_then(|ms| ms.parse::<f64>().ok())
                        .unwrap_or(0.)
                        / 1e3,
                ),
                _presented: presented,
            }
        }
    }

    impl Render for Player {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let busy_until = Instant::now() + self.build;
            while Instant::now() < busy_until {
                std::hint::spin_loop();
            }
            if let Some(index) = self.pending.take() {
                self.drawn.borrow_mut().push_back(index);
            }
            div()
                .size_full()
                .flex()
                .bg(gpui::black())
                .when_some(self.host.as_ref(), |this, host| {
                    this.child(native_view(host).flex_1().h_full())
                })
                .when(self.mode.surface(), |this| {
                    this.child(div().flex_1().h_full().when_some(
                        self.picture.clone(),
                        |this, picture| {
                            this.child(
                                surface(gpui_apple::fast::video_layer::objc2_buffer(&picture))
                                    .size_full(),
                            )
                        },
                    ))
                })
        }
    }

    pub fn main() {
        let mode = match std::env::args().nth(1).as_deref() {
            Some("surface") | None => Mode::Surface,
            Some("layer") => Mode::Layer,
            Some("both") => Mode::Both,
            Some(other) => {
                eprintln!("unknown mode {other}: surface, layer or both");
                std::process::exit(2);
            }
        };
        let measured = std::env::var("VIDEO_PICTURES")
            .ok()
            .and_then(|pictures| pictures.parse().ok())
            .unwrap_or(600);
        application().run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(960.), px(540.)), cx);
            let run = Rc::new(Run {
                mode,
                measured,
                results: Arc::default(),
                gpui_frames: RefCell::default(),
            });
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // A non-activating panel above other windows, on the glass without
                    // taking the keyboard from whatever the user is doing.
                    focus: false,
                    kind: WindowKind::PopUp,
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Player::new(mode, window, cx, run)),
            )
            .expect("open the video window");
        });
    }
}
