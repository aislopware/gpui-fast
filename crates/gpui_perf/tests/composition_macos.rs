//! Window composition on macOS, in a real window: the containers natives live in, the
//! hit testing that decides who gets the pointer, the frames presented inside a
//! transaction, focus both ways, a video layer presenting from another thread without a
//! GPUI frame, the backdrop view a frosted menu over a native gets, (with
//! `COMPOSITION_LIVE=1`) that a native and GPUI's hole around it reach the glass in the
//! same refresh, and (with `GPUI_NATIVE_BLUR_PIXEL_TEST=1`) that the menu blurs the native
//! on the glass.
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
//! GPUI_NATIVE_BLUR_PIXEL_TEST=1 cargo test -p gpui_perf --test composition_macos
//! GPUI_NATIVE_BLUR_PIXEL_TEST=1 GPUI_NATIVE_BLUR=0 \
//!     cargo test -p gpui_perf --test composition_macos   # must fail: the page stays sharp
//! ```
//!
//! The pixel test reads back what the window server composited for its own window with
//! `CGWindowListCreateImage`: Core Animation's own renderers (`CARenderer`,
//! `-[CALayer renderInContext:]`) do not draw a `CABackdropLayer`, so no headless
//! readback shows the blur. A process may capture its own windows without the Screen
//! Recording permission, so it never prompts; where a macOS release refuses even that
//! (the function is deprecated since macOS 14), the check fails saying so, and granting
//! the terminal Screen Recording (System Settings › Privacy & Security) lets it run.

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
        ffi::c_void,
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
    use gpui::{
        App, AsyncApp, Bounds, Context, Corners, FocusHandle, MouseButton, Pixels, PresentedFrame,
        Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas,
        composition::{NativeHost, NativeHostOptions, native_view},
        div, hsla, point,
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
    use objc2::{rc::Retained, runtime::ProtocolObject};
    use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
    use objc2_core_video::CVPixelBuffer;
    use objc2_metal::{
        MTLClearColor, MTLCommandBuffer as _, MTLCommandEncoder as _, MTLCommandQueue,
        MTLDevice as _, MTLDrawable, MTLLoadAction, MTLPixelFormat, MTLRenderPassDescriptor,
        MTLStoreAction,
    };
    use objc2_quartz_core::{CAMetalDrawable as _, CAMetalLayer};

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
        /// A frosted menu drawn over everything, where it is.
        menu: Option<Bounds<Pixels>>,
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
                .when_some(self.menu, |this, menu| this.child(frosted_menu(menu)))
        }
    }

    /// A frosted menu at `bounds`: what is under it blurred by 12 points, rounded by 10,
    /// under a faint white fill.
    fn frosted_menu(bounds: Bounds<Pixels>) -> impl IntoElement {
        div()
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .child(
                canvas(
                    |_, _, _| {},
                    |bounds, _, window, _| {
                        window.paint_backdrop_blur(bounds, Corners::all(px(10.)), px(12.))
                    },
                )
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .size_full()
                    .rounded(px(10.))
                    .bg(hsla(0., 0., 1., 0.2)),
            )
    }

    /// A native whose layer is a `CAMetalLayer` presented with the Core Animation
    /// transaction on every GPUI frame, recording when each drawable was shown.
    struct LiveNative {
        layer: Retained<CAMetalLayer>,
        queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
        shown: Arc<parking_lot::Mutex<Vec<Option<Instant>>>>,
    }

    impl LiveNative {
        fn present_frame(&self) {
            objc2::rc::autoreleasepool(|_| {
                let Some(drawable) = self.layer.nextDrawable() else {
                    return;
                };
                let slot = {
                    let mut shown = self.shown.lock();
                    shown.push(None);
                    shown.len() - 1
                };
                let shown = self.shown.clone();
                let handler = block2::RcBlock::new(
                    move |drawable: NonNull<ProtocolObject<dyn MTLDrawable>>| {
                        // SAFETY: the presented handler gets the drawable it was added to,
                        // alive for the call.
                        let drawable = unsafe { drawable.as_ref() };
                        shown.lock()[slot] = Some(host_time(drawable.presentedTime()));
                    },
                );
                // SAFETY: `addPresentedHandler:` copies the block, before presenting.
                unsafe {
                    drawable.addPresentedHandler(block2::RcBlock::as_ptr(&handler));
                }
                let descriptor = MTLRenderPassDescriptor::new();
                // SAFETY: index zero is within the pass's colour attachments.
                let attachment =
                    unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
                attachment.setTexture(Some(&drawable.texture()));
                attachment.setLoadAction(MTLLoadAction::Clear);
                attachment.setClearColor(MTLClearColor {
                    red: 0.1,
                    green: 0.6,
                    blue: 0.6,
                    alpha: 1.,
                });
                attachment.setStoreAction(MTLStoreAction::Store);
                let Some(command_buffer) = self.queue.commandBuffer() else {
                    return;
                };
                if let Some(encoder) =
                    command_buffer.renderCommandEncoderWithDescriptor(&descriptor)
                {
                    encoder.endEncoding();
                }
                command_buffer.commit();
                command_buffer.waitUntilScheduled();
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
        let blur_pixels =
            std::env::var_os("GPUI_NATIVE_BLUR_PIXEL_TEST").is_some_and(|value| value != "0");
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
                            menu: None,
                        })
                    },
                )
                .expect("open the composition window");
            cx.spawn(async move |cx| {
                let failures = Failures(RefCell::default());
                structure_and_input(cx, window, &failures).await;
                video_without_frames(cx, window, &presented, &failures).await;
                native_blur_view(cx, window, &failures).await;
                if live {
                    same_refresh(cx, window, &presented, &failures).await;
                } else {
                    println!("skip same refresh (set COMPOSITION_LIVE=1)");
                }
                if blur_pixels {
                    native_blur_pixels(cx, window, &failures).await;
                } else {
                    println!("skip native blur pixels (set GPUI_NATIVE_BLUR_PIXEL_TEST=1)");
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
            let container: id = mac_host(&host).container_view().cast();
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
    struct Picture(CFRetained<CVPixelBuffer>);

    // SAFETY: a CVPixelBuffer is a CoreFoundation object, whose retain and release are
    // thread-safe; nothing writes this one after it is made.
    unsafe impl Send for Picture {}

    fn picture() -> Picture {
        use objc2_core_foundation::{CFDictionary, CFType};
        use objc2_core_video::{
            CVPixelBufferCreate, kCVPixelBufferIOSurfacePropertiesKey,
            kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVReturnSuccess,
        };
        // SAFETY: CoreVideo's constant key string lives forever.
        let key = unsafe { kCVPixelBufferIOSurfacePropertiesKey };
        let options =
            CFDictionary::from_slices(&[key], &[&*CFDictionary::<CFType, CFType>::empty()]);
        let mut buffer = std::ptr::null_mut();
        // SAFETY: a valid out-pointer, which CoreVideo fills with a +1 buffer on success.
        let status = unsafe {
            CVPixelBufferCreate(
                None,
                640,
                360,
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                Some(options.as_opaque()),
                NonNull::from(&mut buffer),
            )
        };
        assert_eq!(status, kCVReturnSuccess, "a 4:2:0 buffer");
        // SAFETY: the buffer CoreVideo created, owned from here.
        Picture(unsafe { CFRetained::from_raw(NonNull::new(buffer).expect("a 4:2:0 buffer")) })
    }

    /// The menu's bounds over the native when the native's left edge is at `x`: across its
    /// lower half, clear of the red overlay.
    fn menu_over_native(x: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x + 20.), px(165.)), size(px(160.), px(80.)))
    }

    /// A frosted menu over the native gets a backdrop view between the native's container
    /// and GPUI's layer, in a transactional frame; a frame that changes no blur leaves it
    /// alone and is plain; off the native, the view is hidden.
    async fn native_blur_view(cx: &mut AsyncApp, window: WindowHandle<Stage>, failures: &Failures) {
        let host = window.update(cx, |stage, _, _| stage.host.clone()).unwrap();
        let x = window.update(cx, |stage, _, _| stage.x).unwrap();
        let (transactional_before, _) = mac_host(&host).presents();
        window
            .update(cx, |stage, _, cx| {
                stage.menu = Some(menu_over_native(x));
                cx.notify();
            })
            .unwrap();
        frames(cx, 3).await;
        let (transactional_after, plain_after) = mac_host(&host).presents();
        failures.check(
            transactional_after > transactional_before,
            "the frame placing the blur was transactional",
        );
        let views = mac_host(&host).blur_views();
        failures.check(
            views.len() == 1,
            format!(
                "one backdrop view for the menu over the native: {}",
                views.len()
            ),
        );
        let Some(&view) = views.first() else {
            return;
        };
        let view: id = view.cast();
        let container: id = mac_host(&host).container_view().cast();
        // SAFETY: AppKit and Core Animation reads of this window's own views and layers, on
        // the main thread.
        unsafe {
            let hidden: BOOL = msg_send![view, isHidden];
            failures.check(hidden == NO, "the backdrop view shows");
            let layer: id = msg_send![view, layer];
            let backdrop = Class::get("CABackdropLayer").expect("CABackdropLayer");
            let is_backdrop: BOOL = msg_send![layer, isKindOfClass: backdrop];
            failures.check(is_backdrop == YES, "its layer is a backdrop layer");
            let z: f64 = msg_send![layer, zPosition];
            let container_layer: id = msg_send![container, layer];
            let container_z: f64 = msg_send![container_layer, zPosition];
            failures.check(
                container_z < z && z < 0.,
                format!("it lies over the native and under GPUI: {container_z} < {z} < 0"),
            );
            let frame: NSRect = msg_send![view, frame];
            failures.check(
                frame.size.width == 160. && frame.size.height == 80.,
                format!(
                    "it covers the menu: {} × {}",
                    frame.size.width, frame.size.height
                ),
            );
            let filters: id = msg_send![layer, filters];
            let count: usize = if filters == nil {
                0
            } else {
                msg_send![filters, count]
            };
            failures.check(count == 1, "it blurs with one filter");
        }

        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        frames(cx, 3).await;
        let (transactional_still, plain_still) = mac_host(&host).presents();
        failures.check(
            transactional_still == transactional_after && plain_still > plain_after,
            format!(
                "a frame changing no blur was plain: {transactional_after}→{transactional_still} transactional, {plain_after}→{plain_still} plain"
            ),
        );

        window
            .update(cx, |stage, _, cx| {
                stage.menu = Some(Bounds::new(
                    point(px(480.), px(300.)),
                    size(px(160.), px(80.)),
                ));
                cx.notify();
            })
            .unwrap();
        frames(cx, 3).await;
        // SAFETY: as above.
        let hidden: BOOL = unsafe { msg_send![view, isHidden] };
        failures.check(hidden == YES, "off the native, the backdrop view is hidden");

        window
            .update(cx, |stage, _, cx| {
                stage.menu = None;
                cx.notify();
            })
            .unwrap();
        frames(cx, 3).await;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCreateImage(
            bounds: CGRect,
            list_option: u32,
            window_id: u32,
            image_option: u32,
        ) -> *mut c_void;
        fn CGImageGetWidth(image: *mut c_void) -> usize;
        fn CGImageGetHeight(image: *mut c_void) -> usize;
        fn CGImageGetBytesPerRow(image: *mut c_void) -> usize;
        fn CGImageGetBitsPerPixel(image: *mut c_void) -> usize;
        fn CGImageGetBitmapInfo(image: *mut c_void) -> u32;
        fn CGImageGetDataProvider(image: *mut c_void) -> *mut c_void;
        fn CGDataProviderCopyData(provider: *mut c_void) -> *const c_void;
        fn CGImageRelease(image: *mut c_void);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
        fn CFDataGetLength(data: *const c_void) -> isize;
        fn CFRelease(object: *const c_void);
    }

    /// What the window server composited for one window: grey levels, row by row, with
    /// the image's pixels per point.
    struct Capture {
        width: usize,
        height: usize,
        scale: f64,
        grey: Vec<f64>,
    }

    impl Capture {
        /// The window `ns_window` as it is on the glass, or why it could not be read.
        ///
        /// # Safety
        ///
        /// Main thread; `ns_window` is live.
        unsafe fn window(ns_window: id) -> Result<Self, String> {
            const INCLUDING_WINDOW: u32 = 1 << 3;
            const IGNORE_FRAMING: u32 = 1 << 0;
            const BEST_RESOLUTION: u32 = 1 << 3;
            // SAFETY: Core Graphics reads of this process's own window; every object
            // created here is released here.
            unsafe {
                let number: isize = msg_send![ns_window, windowNumber];
                let frame: NSRect = msg_send![ns_window, frame];
                let null = CGRect::new(
                    CGPoint::new(f64::INFINITY, f64::INFINITY),
                    CGSize::new(0., 0.),
                );
                let image = CGWindowListCreateImage(
                    null,
                    INCLUDING_WINDOW,
                    number as u32,
                    IGNORE_FRAMING | BEST_RESOLUTION,
                );
                if image.is_null() {
                    return Err("CGWindowListCreateImage gave no image of the test's own \
                                window; grant the terminal Screen Recording to run this check"
                        .into());
                }
                let width = CGImageGetWidth(image);
                let height = CGImageGetHeight(image);
                let row = CGImageGetBytesPerRow(image);
                let bits = CGImageGetBitsPerPixel(image);
                let info = CGImageGetBitmapInfo(image);
                let data = CGDataProviderCopyData(CGImageGetDataProvider(image));
                CGImageRelease(image);
                if data.is_null() || bits != 32 {
                    if !data.is_null() {
                        CFRelease(data);
                    }
                    return Err(format!("an image of {bits} bits per pixel, not 32"));
                }
                let bytes = std::slice::from_raw_parts(
                    CFDataGetBytePtr(data),
                    CFDataGetLength(data) as usize,
                );
                // Which of a pixel's four bytes is alpha, from the alpha's place and the
                // byte order.
                let alpha_first = matches!(info & 0x1f, 2 | 4 | 6);
                let little = info & 0x7000 == 0x2000;
                let alpha = if alpha_first != little { 0 } else { 3 };
                let mut grey = Vec::with_capacity(width * height);
                for y in 0..height {
                    for x in 0..width {
                        let pixel = &bytes[y * row + x * 4..][..4];
                        let sum: u32 = (0..4)
                            .filter(|channel| *channel != alpha)
                            .map(|channel| u32::from(pixel[channel]))
                            .sum();
                        grey.push(f64::from(sum) / 3.);
                    }
                }
                CFRelease(data);
                Ok(Self {
                    width,
                    height,
                    scale: width as f64 / frame.size.width,
                    grey,
                })
            }
        }

        /// The standard deviation of the grey levels along the row at GPUI's `y`, from
        /// `left` to `right` (in points).
        fn deviation(&self, y: f32, left: f32, right: f32) -> f64 {
            let row = ((f64::from(y) * self.scale) as usize).min(self.height - 1);
            let from = ((f64::from(left) * self.scale) as usize).min(self.width);
            let to = ((f64::from(right) * self.scale) as usize).min(self.width);
            let samples = &self.grey[row * self.width + from..row * self.width + to.max(from)];
            if samples.is_empty() {
                return 0.;
            }
            let mean = samples.iter().sum::<f64>() / samples.len() as f64;
            let variance = samples
                .iter()
                .map(|grey| (grey - mean) * (grey - mean))
                .sum::<f64>()
                / samples.len() as f64;
            variance.sqrt()
        }
    }

    /// A layer of black and white stripes, 4 points each, for a native whose blur shows.
    fn stripes() -> id {
        // SAFETY: Core Animation layer creation on the main thread; the returned layer is
        // autoreleased, and its stripes are owned by it.
        unsafe {
            let layer: id = msg_send![class!(CALayer), layer];
            let black: id = msg_send![class!(NSColor), blackColor];
            let black: id = msg_send![black, CGColor];
            let () = msg_send![layer, setBackgroundColor: black];
            let white: id = msg_send![class!(NSColor), whiteColor];
            let white: id = msg_send![white, CGColor];
            for stripe in 0..200 {
                let sublayer: id = msg_send![class!(CALayer), layer];
                let frame = NSRect::new(
                    NSPoint::new(f64::from(stripe) * 8., 0.),
                    NSSize::new(4., 2000.),
                );
                let () = msg_send![sublayer, setFrame: frame];
                let () = msg_send![sublayer, setBackgroundColor: white];
                let () = msg_send![layer, addSublayer: sublayer];
            }
            layer
        }
    }

    /// The menu over a striped native, read back from the glass: across the menu the
    /// stripes are blurred away, beside it on the same native they are not.
    async fn native_blur_pixels(
        cx: &mut AsyncApp,
        window: WindowHandle<Stage>,
        failures: &Failures,
    ) {
        let host = window.update(cx, |stage, _, _| stage.host.clone()).unwrap();
        // SAFETY: attaches a live layer on the main thread; the host's view retains it.
        unsafe {
            host.attach_layer(NonNull::new(stripes().cast()).unwrap())
                .unwrap();
        }
        let x = 40.;
        let menu = menu_over_native(x);
        window
            .update(cx, |stage, _, cx| {
                stage.x = x;
                stage.shown = true;
                stage.menu = Some(menu);
                cx.notify();
            })
            .unwrap();
        frames(cx, 15).await;
        let container: id = mac_host(&host).container_view().cast();
        // SAFETY: the container's window is this test's live window; main thread.
        let capture = unsafe {
            let ns_window: id = msg_send![container, window];
            Capture::window(ns_window)
        };
        let capture = match capture {
            Ok(capture) => capture,
            Err(why) => {
                failures.check(false, format!("read the window back: {why}"));
                return;
            }
        };
        // One row across the menu's middle; inside it clear of its rounded corners, beside
        // it on the native (which spans x to x + 300) clear of its edge.
        let y = f32::from(menu.origin.y + menu.size.height / 2.);
        let menu_left = f32::from(menu.origin.x);
        let menu_right = f32::from(menu.origin.x + menu.size.width);
        let inside = capture.deviation(y, menu_left + 20., menu_right - 20.);
        let beside = capture.deviation(y, menu_right + 16., x + 290.);
        println!("native blur: grey deviation {inside:.1} under the menu, {beside:.1} beside it");
        failures.check(
            beside > 40.,
            format!("the stripes beside the menu are sharp on the glass: deviation {beside:.1}"),
        );
        failures.check(
            inside < beside * 0.1,
            format!(
                "the menu blurs the native under it: deviation {inside:.1} against {beside:.1} beside it"
            ),
        );
        window
            .update(cx, |stage, _, cx| {
                stage.menu = None;
                cx.notify();
            })
            .unwrap();
        frames(cx, 3).await;
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
            let device = objc2_metal::MTLCreateSystemDefaultDevice().expect("a Metal device");
            let layer = CAMetalLayer::new();
            layer.setDevice(Some(&device));
            layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            layer.setPresentsWithTransaction(true);
            layer.setMaximumDrawableCount(3);
            layer.setDrawableSize(CGSize::new(600., 400.));
            // SAFETY: attaches a live CAMetalLayer on the main thread.
            unsafe {
                host.attach_layer(
                    NonNull::new(Retained::as_ptr(&layer).cast_mut().cast()).unwrap(),
                )
                .unwrap();
            }
            Rc::new(LiveNative {
                layer,
                queue: device.newCommandQueue().expect("a command queue"),
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
