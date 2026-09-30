//! Window composition on macOS, in a real window: the containers natives live in, the
//! hit testing that decides who gets the pointer, the frames presented inside a
//! transaction, focus both ways, a video layer presenting from another thread without a
//! GPUI frame, and (with `COMPOSITION_LIVE=1`) that a native and GPUI's hole around it
//! reach the glass in the same refresh.
//!
//! It runs on the main thread, as AppKit requires, so it has its own harness. It opens
//! one non-activating panel above the other windows and never takes the keyboard from
//! the user; its events are made in-process and handed to its own window, never posted
//! to another process.
//!
//! ```text
//! cargo test -p gpui_perf --test composition_macos
//! COMPOSITION_LIVE=1 GPUI_PRESENTED_AT_CALLBACK=1 cargo test -p gpui_perf --release --test composition_macos
//! COMPOSITION_LIVE=1 GPUI_PRESENTED_AT_CALLBACK=1 GPUI_COMPOSITION_TRANSACTIONS=0 \
//!     cargo test -p gpui_perf --release --test composition_macos   # must report seams
//! ```

extern crate gpui_fast as gpui;
extern crate gpui_platform_fast as gpui_platform;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    macos::main();
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        cell::{Cell, RefCell},
        ptr::NonNull,
        rc::Rc,
        sync::{
            Arc, Once,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use cocoa::{
        base::{id, nil},
        foundation::{NSPoint, NSRect, NSSize},
    };
    use foreign_types::ForeignType;
    use gpui::{
        App, AsyncApp, Bounds, Context, FocusHandle, MouseButton, PresentedFrame, Window,
        WindowBounds, WindowHandle, WindowKind, WindowOptions,
        composition::{NativeHost, NativeHostOptions, native_view},
        div,
        prelude::*,
        px, rgb, size,
    };
    use gpui_apple::fast::video_layer::{VideoLayer, VideoLayerOptions};
    use gpui_macos::MacNativeHost;
    use gpui_platform::application;
    use objc::{
        class,
        declare::ClassDecl,
        msg_send,
        runtime::{BOOL, Class, NO, Object, Sel, YES},
        sel, sel_impl,
    };

    static MOVED_TO_WINDOW: AtomicUsize = AtomicUsize::new(0);
    static MOUSE_DOWNS: AtomicUsize = AtomicUsize::new(0);
    static mut PROBE_CLASS: *const Class = std::ptr::null();

    /// An `NSView` that counts what AppKit does to it.
    fn probe_class() -> *const Class {
        static REGISTER: Once = Once::new();
        REGISTER.call_once(|| {
            let mut decl = ClassDecl::new("CompositionProbeView", class!(NSView)).unwrap();
            extern "C" fn moved_to_window(this: &Object, _: Sel) {
                MOVED_TO_WINDOW.fetch_add(1, Ordering::Relaxed);
                // SAFETY: NSView's own implementation.
                unsafe {
                    let () = msg_send![super(this, class!(NSView)), viewDidMoveToWindow];
                }
            }
            extern "C" fn mouse_down(_: &Object, _: Sel, _: id) {
                MOUSE_DOWNS.fetch_add(1, Ordering::Relaxed);
            }
            extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
                YES
            }
            // SAFETY: the signatures match the selectors.
            unsafe {
                decl.add_method(
                    sel!(viewDidMoveToWindow),
                    moved_to_window as extern "C" fn(&Object, Sel),
                );
                decl.add_method(
                    sel!(mouseDown:),
                    mouse_down as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    sel!(acceptsFirstResponder),
                    yes as extern "C" fn(&Object, Sel) -> BOOL,
                );
                PROBE_CLASS = decl.register();
            }
        });
        // SAFETY: registered above.
        unsafe { PROBE_CLASS }
    }

    struct Stage {
        host: NativeHost,
        native_focus: FocusHandle,
        other_focus: FocusHandle,
        x: f32,
        shown: bool,
        gpui_mouse_downs: Rc<Cell<usize>>,
        animate: bool,
        live: Option<Rc<LiveNative>>,
        video: Option<NativeHost>,
    }

    impl Render for Stage {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            if self.animate {
                self.x += 1.;
                window.request_animation_frame();
            }
            if let Some(live) = &self.live {
                live.present_frame();
            }
            let downs = self.gpui_mouse_downs.clone();
            div()
                .relative()
                .size_full()
                .bg(rgb(0x202020))
                .when(self.shown, |this| {
                    this.child(
                        native_view(&self.host)
                            .id("native")
                            .absolute()
                            .left(px(self.x))
                            .top(px(50.))
                            .w(px(300.))
                            .h(px(200.))
                            .rounded(px(8.))
                            .track_focus(&self.native_focus)
                            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                downs.set(downs.get() + 1)
                            }),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .left(px(200.))
                        .top(px(100.))
                        .w(px(80.))
                        .h(px(60.))
                        .bg(rgb(0xcc3030))
                        .occlude(),
                )
                .when_some(self.video.as_ref(), |this, video| {
                    this.child(
                        native_view(video)
                            .absolute()
                            .left(px(420.))
                            .top(px(40.))
                            .w(px(240.))
                            .h(px(135.)),
                    )
                })
                .child(
                    div()
                        .id("other")
                        .track_focus(&self.other_focus)
                        .absolute()
                        .left(px(500.))
                        .top(px(300.))
                        .size(px(40.))
                        .bg(rgb(0x3060cc)),
                )
        }
    }

    /// A native whose layer is a `CAMetalLayer` presented with the Core Animation
    /// transaction on every GPUI frame, recording when each drawable was shown.
    struct LiveNative {
        layer: metal::MetalLayer,
        queue: metal::CommandQueue,
        shown: Arc<parking_lot::Mutex<Vec<Option<Instant>>>>,
    }

    impl LiveNative {
        fn present_frame(&self) {
            objc2::rc::autoreleasepool(|_| {
                let Some(drawable) = self.layer.next_drawable() else {
                    return;
                };
                let slot = {
                    let mut shown = self.shown.lock();
                    shown.push(None);
                    shown.len() - 1
                };
                let shown = self.shown.clone();
                let handler =
                    block2::RcBlock::new(move |drawable: NonNull<objc2::runtime::AnyObject>| {
                        // SAFETY: the presented handler gets the drawable it was added to.
                        let drawable = unsafe { &*drawable.as_ptr().cast::<Object>() };
                        // SAFETY: `presentedTime` is a property of every CAMetalDrawable.
                        let time: f64 = unsafe { msg_send![drawable, presentedTime] };
                        shown.lock()[slot] = Some(host_time(time));
                    });
                // SAFETY: `addPresentedHandler:` copies the block, before presenting.
                unsafe {
                    let () = msg_send![drawable, addPresentedHandler: &*handler];
                }
                let descriptor = metal::RenderPassDescriptor::new();
                let attachment = descriptor.color_attachments().object_at(0).unwrap();
                attachment.set_texture(Some(drawable.texture()));
                attachment.set_load_action(metal::MTLLoadAction::Clear);
                attachment.set_clear_color(metal::MTLClearColor::new(0.1, 0.6, 0.6, 1.));
                attachment.set_store_action(metal::MTLStoreAction::Store);
                let command_buffer = self.queue.new_command_buffer();
                command_buffer
                    .new_render_command_encoder(descriptor)
                    .end_encoding();
                command_buffer.commit();
                command_buffer.wait_until_scheduled();
                drawable.present();
            });
        }
    }

    /// A Core Animation host time as an `Instant`, or now when the display gave none (the
    /// presented handler is running now).
    fn host_time(time: f64) -> Instant {
        unsafe extern "C" {
            fn CACurrentMediaTime() -> f64;
        }
        let now = Instant::now();
        if time <= 0. {
            return now;
        }
        // SAFETY: a pure read of the host clock.
        let age = unsafe { CACurrentMediaTime() } - time;
        if age >= 0. {
            now - Duration::from_secs_f64(age)
        } else {
            now + Duration::from_secs_f64(-age)
        }
    }

    struct Failures(RefCell<Vec<String>>);

    impl Failures {
        fn check(&self, ok: bool, what: impl Into<String>) {
            let what = what.into();
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
            if !ok {
                self.0.borrow_mut().push(what);
            }
        }
    }

    async fn frames(cx: &mut AsyncApp, count: u64) {
        cx.background_executor()
            .timer(Duration::from_millis(17 * count))
            .await;
    }

    fn mac_host(host: &NativeHost) -> &MacNativeHost {
        host.platform()
            .as_any()
            .downcast_ref::<MacNativeHost>()
            .expect("macOS natives")
    }

    /// Sends the window a mouse event made here, as AppKit would dispatch it.
    ///
    /// # Safety
    ///
    /// Main thread; `window` is live.
    unsafe fn send_mouse(window: id, event_type: u64, location: NSPoint) {
        // SAFETY: builds an NSEvent for this process's own window and dispatches it.
        unsafe {
            let number: isize = msg_send![window, windowNumber];
            let event: id = msg_send![class!(NSEvent),
                mouseEventWithType: event_type
                location: location
                modifierFlags: 0usize
                timestamp: 0f64
                windowNumber: number
                context: nil
                eventNumber: 0isize
                clickCount: 1isize
                pressure: 1f32];
            let () = msg_send![window, sendEvent: event];
        }
    }

    pub fn main() {
        let live = std::env::var_os("COMPOSITION_LIVE").is_some();
        application().run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(700.), px(450.)), cx);
            let presented: Rc<RefCell<Vec<PresentedFrame>>> = Rc::default();
            let window: WindowHandle<Stage> = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        focus: false,
                        kind: WindowKind::PopUp,
                        inactive_frame_interval: None,
                        ..Default::default()
                    },
                    |window, cx| {
                        let host = window
                            .create_native_host(NativeHostOptions::default(), cx)
                            .expect("macOS composes natives");
                        let presented = presented.clone();
                        window
                            .on_frame_presented(move |frame, _, _| {
                                presented.borrow_mut().push(frame)
                            })
                            .detach();
                        cx.new(|cx| Stage {
                            host,
                            native_focus: cx.focus_handle(),
                            other_focus: cx.focus_handle(),
                            x: 40.,
                            shown: true,
                            gpui_mouse_downs: Rc::default(),
                            animate: false,
                            live: None,
                            video: None,
                        })
                    },
                )
                .expect("open the composition window");
            cx.spawn(async move |cx| {
                let failures = Failures(RefCell::default());
                structure_and_input(cx, window, &failures).await;
                video_without_frames(cx, window, &presented, &failures).await;
                if live {
                    same_refresh(cx, window, &presented, &failures).await;
                } else {
                    println!("skip same refresh (set COMPOSITION_LIVE=1)");
                }
                let failed = failures.0.borrow().len();
                println!("{} checks failed", failed);
                std::process::exit(if failed == 0 { 0 } else { 1 });
            })
            .detach();
        });
    }

    async fn structure_and_input(
        cx: &mut AsyncApp,
        window: WindowHandle<Stage>,
        failures: &Failures,
    ) {
        frames(cx, 3).await;
        let host = window.update(cx, |stage, _, _| stage.host.clone()).unwrap();
        // SAFETY: AppKit calls on this window's own views, on the main thread.
        let (probe, container, content_view, ns_window, gpui_view) = unsafe {
            let probe: id = msg_send![probe_class(), alloc];
            let probe: id = msg_send![probe, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(10., 10.))];
            let () = msg_send![probe, setWantsLayer: YES];
            let layer: id = msg_send![probe, layer];
            let color: id = msg_send![class!(NSColor), systemTealColor];
            let cg: id = msg_send![color, CGColor];
            let () = msg_send![layer, setBackgroundColor: cg];
            host.attach_view(NonNull::new(probe.cast()).unwrap())
                .unwrap();
            let () = msg_send![probe, release];
            let container = mac_host(&host).container_view();
            let content_view: id = msg_send![container, superview];
            let ns_window: id = msg_send![container, window];
            let subviews: id = msg_send![content_view, subviews];
            let count: usize = msg_send![subviews, count];
            let gpui_view: id = msg_send![subviews, objectAtIndex: count - 1];
            (probe, container, content_view, ns_window, gpui_view)
        };
        let moved_at_attach = MOVED_TO_WINDOW.load(Ordering::Relaxed);
        failures.check(
            moved_at_attach >= 1,
            "the attached view moved into the window",
        );

        // A moving native: 60 frames, each presented with a transaction.
        let (transactional_before, _) = mac_host(&host).presents();
        window
            .update(cx, |stage, _, cx| {
                stage.animate = true;
                cx.notify();
            })
            .unwrap();
        let mut superview_stable = true;
        for _ in 0..30 {
            frames(cx, 2).await;
            // SAFETY: as above.
            let superview: id = unsafe { msg_send![container, superview] };
            superview_stable &= superview == content_view;
        }
        window
            .update(cx, |stage, _, _| stage.animate = false)
            .unwrap();
        frames(cx, 3).await;
        let (transactional_after, plain_after) = mac_host(&host).presents();
        failures.check(
            superview_stable,
            "the container stayed in its superview while moving",
        );
        failures.check(
            MOVED_TO_WINDOW.load(Ordering::Relaxed) == moved_at_attach,
            "the attached view never left the window while its native moved",
        );
        failures.check(
            transactional_after - transactional_before >= 20,
            format!(
                "frames moving the native were transactional: {}",
                transactional_after - transactional_before
            ),
        );
        // A frame that moves nothing is presented as any other.
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        frames(cx, 3).await;
        let (transactional_still, plain_still) = mac_host(&host).presents();
        failures.check(
            transactional_still == transactional_after && plain_still > plain_after,
            format!(
                "a frame moving nothing was plain: {transactional_after}→{transactional_still} transactional, {plain_after}→{plain_still} plain"
            ),
        );
        // SAFETY: as above.
        unsafe {
            let layer: id = msg_send![container, layer];
            let z: f64 = msg_send![layer, zPosition];
            failures.check(
                z < 0.,
                format!("the container is stacked below GPUI's layer: {z}"),
            );
            let hidden: BOOL = msg_send![container, isHidden];
            failures.check(hidden == NO, "a placed native is shown");
            let gpui_layer: id = msg_send![gpui_view, layer];
            let opaque: BOOL = msg_send![gpui_layer, isOpaque];
            failures.check(opaque == NO, "GPUI's layer composites over the hole");
        }

        // Hit testing, by direct calls. The native is at x ≥ its left, y 50..250 (GPUI
        // coordinates); the red overlay at 200..280 × 100..160 is above it.
        let x = window.update(cx, |stage, _, _| stage.x).unwrap() as f64;
        // SAFETY: as above.
        unsafe {
            let bounds: NSRect = msg_send![gpui_view, bounds];
            let at = |gx: f64, gy: f64| -> id {
                let in_view = NSPoint::new(gx, bounds.size.height - gy);
                let frame_view: id = msg_send![content_view, superview];
                let point: NSPoint = msg_send![gpui_view, convertPoint: in_view toView: frame_view];
                msg_send![content_view, hitTest: point]
            };
            let over_native = at(x + 20., 200.);
            failures.check(
                over_native == probe,
                "over the native, the native's view is hit",
            );
            failures.check(at(230., 130.) == gpui_view, "over the overlay, GPUI is hit");
            failures.check(
                at(650., 420.) == gpui_view,
                "beside the native, GPUI is hit",
            );
        }

        // A click on the native reaches GPUI first and then the native.
        let downs_before = window
            .update(cx, |stage, _, _| stage.gpui_mouse_downs.get())
            .unwrap();
        let native_downs_before = MOUSE_DOWNS.load(Ordering::Relaxed);
        // SAFETY: as above.
        unsafe {
            let bounds: NSRect = msg_send![gpui_view, bounds];
            let in_view = NSPoint::new(x + 20., bounds.size.height - 200.);
            let location: NSPoint = msg_send![gpui_view, convertPoint: in_view toView: nil];
            send_mouse(ns_window, 1, location);
            send_mouse(ns_window, 2, location);
        }
        frames(cx, 3).await;
        let downs_after = window
            .update(cx, |stage, _, _| stage.gpui_mouse_downs.get())
            .unwrap();
        failures.check(
            downs_after == downs_before + 1,
            "GPUI saw the click on the native",
        );
        failures.check(
            MOUSE_DOWNS.load(Ordering::Relaxed) == native_downs_before + 1,
            "the native got the click",
        );

        // Focus, platform to GPUI: the native becomes first responder.
        // SAFETY: as above.
        unsafe {
            let _: BOOL = msg_send![ns_window, makeFirstResponder: probe];
        }
        frames(cx, 3).await;
        let native_focused = window
            .update(cx, |stage, window, _| stage.native_focus.is_focused(window))
            .unwrap();
        failures.check(
            native_focused,
            "the native's first responder focused its element",
        );

        // GPUI to platform: GPUI's focus moves away, and the GPUI view takes the keyboard.
        window
            .update(cx, |stage, window, cx| window.focus(&stage.other_focus, cx))
            .unwrap();
        frames(cx, 3).await;
        // SAFETY: as above.
        let responder: id = unsafe { msg_send![ns_window, firstResponder] };
        failures.check(
            responder == gpui_view,
            "GPUI took the keyboard back from the native",
        );

        // And back: GPUI focuses the native's element, the native takes the keyboard.
        window
            .update(cx, |stage, window, cx| {
                window.focus(&stage.native_focus, cx)
            })
            .unwrap();
        frames(cx, 3).await;
        // SAFETY: as above.
        let responder: id = unsafe { msg_send![ns_window, firstResponder] };
        failures.check(responder == probe, "GPUI gave the native the keyboard");

        // A native no element places is hidden.
        window
            .update(cx, |stage, _, cx| {
                stage.shown = false;
                cx.notify();
            })
            .unwrap();
        frames(cx, 3).await;
        // SAFETY: as above.
        unsafe {
            let hidden: BOOL = msg_send![container, isHidden];
            failures.check(hidden == YES, "an unplaced native is hidden");
            let gpui_layer: id = msg_send![gpui_view, layer];
            let opaque: BOOL = msg_send![gpui_layer, isOpaque];
            failures.check(opaque == YES, "with no hole, GPUI's layer is opaque again");
        }
        window
            .update(cx, |stage, _, cx| {
                stage.shown = true;
                cx.notify();
            })
            .unwrap();
        frames(cx, 3).await;
    }

    /// Presents pictures to a video layer from another thread: each is reported, and the
    /// GPUI window draws no frame for them.
    async fn video_without_frames(
        cx: &mut AsyncApp,
        window: WindowHandle<Stage>,
        presented: &Rc<RefCell<Vec<PresentedFrame>>>,
        failures: &Failures,
    ) {
        const PICTURES: usize = 60;
        let host = window
            .update(cx, |stage, window, cx| {
                let host = window
                    .create_native_host(NativeHostOptions::default(), cx)
                    .expect("macOS composes natives");
                stage.video = Some(host.clone());
                cx.notify();
                host
            })
            .unwrap();
        let video = VideoLayer::attach(&host, VideoLayerOptions::default()).expect("a video layer");
        let reports = Arc::new(AtomicUsize::new(0));
        video.on_presented(Some(Arc::new({
            let reports = reports.clone();
            move |_, _| {
                reports.fetch_add(1, Ordering::Relaxed);
            }
        })));
        frames(cx, 5).await;
        presented.borrow_mut().clear();
        let producer = std::thread::spawn(move || {
            let buffer = picture();
            for _ in 0..PICTURES {
                video.present(&buffer.0).expect("present a picture");
                std::thread::sleep(Duration::from_micros(16_667));
            }
        });
        cx.background_executor()
            .timer(Duration::from_millis(1500))
            .await;
        producer.join().expect("the producer finished");
        // The last picture is reported once the display has shown it, a refresh or two
        // after it was presented; under load that can take longer than a fixed wait.
        let deadline = Instant::now() + Duration::from_secs(3);
        while reports.load(Ordering::Relaxed) < PICTURES && Instant::now() < deadline {
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
        }
        failures.check(
            reports.load(Ordering::Relaxed) == PICTURES,
            format!(
                "every picture presented from another thread was reported: {} of {PICTURES}",
                reports.load(Ordering::Relaxed)
            ),
        );
        failures.check(
            presented.borrow().is_empty(),
            format!(
                "the window drew no frame for the video: {}",
                presented.borrow().len()
            ),
        );
        window
            .update(cx, |stage, _, cx| {
                stage.video = None;
                cx.notify();
            })
            .unwrap();
        frames(cx, 3).await;
    }

    /// A 4:2:0 picture backed by an IOSurface, as a decoder hands over.
    struct Picture(core_video::pixel_buffer::CVPixelBuffer);

    // SAFETY: a CVPixelBuffer is a CoreFoundation object, whose retain and release are
    // thread-safe; nothing writes this one after it is made.
    unsafe impl Send for Picture {}

    fn picture() -> Picture {
        use core_foundation::{base::TCFType, dictionary::CFDictionary, string::CFString};
        use core_video::pixel_buffer::{
            CVPixelBuffer, kCVPixelBufferIOSurfacePropertiesKey,
            kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
        };
        // SAFETY: CoreVideo's constant key string lives forever.
        let key = unsafe { CFString::wrap_under_get_rule(kCVPixelBufferIOSurfacePropertiesKey) };
        let empty = CFDictionary::<CFString, CFString>::from_CFType_pairs(&[]);
        let options = CFDictionary::from_CFType_pairs(&[(key, empty.as_CFType())]);
        Picture(
            CVPixelBuffer::new(
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                640,
                360,
                Some(&options),
            )
            .expect("a 4:2:0 buffer"),
        )
    }

    /// Scrolls the native a point a frame for ten seconds; every frame's GPUI drawable
    /// and native drawable must reach the glass in the same refresh.
    async fn same_refresh(
        cx: &mut AsyncApp,
        window: WindowHandle<Stage>,
        presented: &Rc<RefCell<Vec<PresentedFrame>>>,
        failures: &Failures,
    ) {
        let host = window.update(cx, |stage, _, _| stage.host.clone()).unwrap();
        let live = {
            let device = metal::Device::system_default().expect("a Metal device");
            let layer = metal::MetalLayer::new();
            layer.set_device(&device);
            layer.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
            layer.set_presents_with_transaction(true);
            layer.set_maximum_drawable_count(3);
            layer.set_drawable_size(core_graphics::geometry::CGSize::new(600., 400.));
            // SAFETY: attaches a live CAMetalLayer on the main thread.
            unsafe {
                host.attach_layer(NonNull::new(layer.as_ptr().cast()).unwrap())
                    .unwrap();
            }
            Rc::new(LiveNative {
                layer,
                queue: device.new_command_queue(),
                shown: Arc::default(),
            })
        };
        frames(cx, 5).await;
        presented.borrow_mut().clear();
        window
            .update(cx, |stage, _, cx| {
                stage.live = Some(live.clone());
                stage.animate = true;
                stage.x = 0.;
                cx.notify();
            })
            .unwrap();
        cx.background_executor()
            .timer(Duration::from_secs(10))
            .await;
        window
            .update(cx, |stage, _, _| {
                stage.animate = false;
                stage.live = None;
            })
            .unwrap();
        frames(cx, 10).await;
        let gpui: Vec<Option<Instant>> = presented
            .borrow()
            .iter()
            .map(|frame| frame.presented_at)
            .collect();
        let native = live.shown.lock().clone();
        let pairs = gpui.len().min(native.len());
        let mut seams = 0;
        let mut compared = 0;
        let mut apart_ms = Vec::new();
        for (gpui, native) in gpui.iter().zip(&native).skip(2) {
            let (Some(gpui), Some(native)) = (gpui, native) else {
                continue;
            };
            compared += 1;
            let apart = if gpui > native {
                *gpui - *native
            } else {
                *native - *gpui
            };
            apart_ms.push(apart.as_secs_f64() * 1e3);
            if apart > Duration::from_millis(4) {
                seams += 1;
            }
        }
        apart_ms.sort_by(f64::total_cmp);
        let median = apart_ms
            .get(apart_ms.len() / 2)
            .copied()
            .unwrap_or_default();
        println!(
            "same refresh: {compared} of {pairs} frames compared ({} GPUI, {} native), {seams} seams, median apart {median:.2} ms",
            gpui.len(),
            native.len()
        );
        failures.check(compared > 300, "enough frames reached the glass to compare");
        failures.check(
            seams == 0,
            format!("the native and its hole moved together: {seams} seams"),
        );
    }
}
