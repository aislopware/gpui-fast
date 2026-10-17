//! A finger over a native, checked inside the simulator app: what the native's own scroll
//! views do not take scrolls GPUI under it, momentum included, and what they take stays
//! theirs. The native is a `WKWebView` whose page lives in a frame, its own scroll view off,
//! as an inline page is shown; the drags are real touches sent through `UIApplication`,
//! so UIKit's hit testing and gesture recognisers decide where they go. Each check prints
//! `ok` or `FAIL`, and the app exits with the number of failures.
//!
//! ```text
//! crates/gpui_ios/examples/ios/build-simulator.sh
//! xcrun simctl boot 'iPhone 18 Pro'
//! xcrun simctl install booted target/gpui-ios-example-xcode/Build/Products/Debug-iphonesimulator/GPUIIosExample.app
//! SIMCTL_CHILD_GPUI_IOS_NATIVE_SCROLL_TEST=1 xcrun simctl launch --console-pty booted dev.zed.gpui-ios-example
//! ```

use std::{cell::RefCell, ffi::c_void, ptr::NonNull, time::Duration};

use gpui::{
    App, AsyncApp, Context, ScrollHandle, Window, WindowHandle, WindowOptions,
    composition::{NativeHost, NativeHostOptions, native_view},
    div, point,
    prelude::*,
    px, rgb,
};
use objc2::{class, msg_send, runtime::AnyObject, sel};

use crate::composition_test::{CGPoint, CGRect};

/// Where the page sits in the scroller, and how tall it is.
const PAGE_TOP: f32 = 200.;
const PAGE_HEIGHT: f32 = 300.;

struct Stage {
    host: NativeHost,
    scroll: ScrollHandle,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("chat")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(rgb(0x202020))
            .child(div().h(px(PAGE_TOP)))
            .child(
                native_view(&self.host)
                    .id("page")
                    .block_mouse_except_scroll()
                    .w_full()
                    .h(px(PAGE_HEIGHT)),
            )
            .child(div().h(px(4000.)).bg(rgb(0x303040)))
    }
}

struct Failures(RefCell<usize>);

impl Failures {
    fn check(&self, ok: bool, what: impl AsRef<str>) {
        println!("{} {}", if ok { "ok  " } else { "FAIL" }, what.as_ref());
        if !ok {
            *self.0.borrow_mut() += 1;
        }
    }
}

async fn wait(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

// `IOHIDEvent`'s creators (IOKit's `IOHIDEvent.h`, exported though not in the SDK), the
// digitizer events UIKit's touch handling reads a touch's position and contact from.
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDEventCreateDigitizerEvent(
        allocator: *const c_void,
        time: u64,
        transducer: u32,
        index: u32,
        identity: u32,
        mask: u32,
        buttons: u32,
        x: f64,
        y: f64,
        z: f64,
        tip: f64,
        barrel: f64,
        range: u8,
        touch: u8,
        options: u32,
    ) -> *mut c_void;
    fn IOHIDEventCreateDigitizerFingerEventWithQuality(
        allocator: *const c_void,
        time: u64,
        index: u32,
        identity: u32,
        mask: u32,
        x: f64,
        y: f64,
        z: f64,
        tip: f64,
        twist: f64,
        minor: f64,
        major: f64,
        quality: f64,
        density: f64,
        irregularity: f64,
        range: u8,
        touch: u8,
        options: u32,
    ) -> *mut c_void;
    fn IOHIDEventAppendEvent(event: *mut c_void, child: *mut c_void, options: u32);
    fn IOHIDEventSetIntegerValue(event: *mut c_void, field: u32, value: i32);
    fn mach_absolute_time() -> u64;
    fn CFRelease(object: *const c_void);
}

/// An `IOHIDEventRef`, typed as UIKit's selectors declare it.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct HidEvent(*mut c_void);

// SAFETY: a pointer to the opaque `struct __IOHIDEvent`, as `IOHIDEventRef` is declared.
unsafe impl objc2::encode::Encode for HidEvent {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("__IOHIDEvent", &[]));
}

/// `kIOHIDDigitizerTransducerTypeHand`, `kIOHIDDigitizerEvent{Range,Touch,Position}` and
/// `kIOHIDEventFieldDigitizerIsDisplayIntegrated` (IOKit's `IOHIDEventTypes.h`).
const TRANSDUCER_HAND: u32 = 3;
const EVENT_RANGE: u32 = 1 << 0;
const EVENT_TOUCH: u32 = 1 << 1;
const EVENT_POSITION: u32 = 1 << 2;
const FIELD_DISPLAY_INTEGRATED: u32 = (11 << 16) | 25;

/// `UITouchPhase`.
const BEGAN: isize = 0;
const MOVED: isize = 1;
const ENDED: isize = 3;

/// The digitizer event of one finger at `at` (window points) in `phase`.
fn hid_event(at: CGPoint, phase: isize) -> *mut c_void {
    let touching = u8::from(phase != ENDED);
    let mask = if phase == MOVED {
        EVENT_POSITION
    } else {
        EVENT_RANGE | EVENT_TOUCH
    };
    // SAFETY: IOKit's creators make new events the caller releases; the finger is appended
    // to the hand, which retains it.
    unsafe {
        let time = mach_absolute_time();
        let hand = IOHIDEventCreateDigitizerEvent(
            std::ptr::null(),
            time,
            TRANSDUCER_HAND,
            0,
            0,
            EVENT_TOUCH,
            0,
            0.,
            0.,
            0.,
            0.,
            0.,
            0,
            touching,
            0,
        );
        IOHIDEventSetIntegerValue(hand, FIELD_DISPLAY_INTEGRATED, 1);
        let finger = IOHIDEventCreateDigitizerFingerEventWithQuality(
            std::ptr::null(),
            time,
            1,
            2,
            mask,
            at.x,
            at.y,
            0.,
            0.,
            0.,
            5.,
            5.,
            1.,
            1.,
            1.,
            touching,
            touching,
            0,
        );
        IOHIDEventSetIntegerValue(finger, FIELD_DISPLAY_INTEGRATED, 1);
        IOHIDEventAppendEvent(hand, finger, 0);
        CFRelease(finger);
        hand
    }
}

/// One finger, as UIKit's own touch delivery makes it: hit-tested where it lands, carrying
/// its digitizer event, sent through `UIApplication` in its touches event.
struct Finger {
    touch: *mut AnyObject,
    window: *mut AnyObject,
}

fn uptime() -> f64 {
    // SAFETY: `NSProcessInfo` is thread-safe; `systemUptime` is a plain getter.
    unsafe {
        let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
        msg_send![info, systemUptime]
    }
}

impl Finger {
    fn down(window: *mut AnyObject, at: CGPoint) -> Self {
        // SAFETY: `UITouch`'s own setters (private, as KIF and UIKit's tests use them) on a
        // new touch, on the main thread.
        unsafe {
            let touch: *mut AnyObject = msg_send![class!(UITouch), new];
            let _: () = msg_send![touch, setWindow: window];
            let _: () = msg_send![touch, setTapCount: 1usize];
            let _: () = msg_send![touch, _setLocationInWindow: at, resetPrevious: true];
            let view: *mut AnyObject =
                msg_send![window, hitTest: at, withEvent: std::ptr::null::<AnyObject>()];
            let _: () = msg_send![touch, setView: view];
            let gesture_view: bool = msg_send![touch, respondsToSelector: sel!(setGestureView:)];
            if gesture_view {
                let _: () = msg_send![touch, setGestureView: view];
            }
            let finger = Self { touch, window };
            finger.send(at, BEGAN);
            finger
        }
    }

    fn send(&self, at: CGPoint, phase: isize) {
        let hid = hid_event(at, phase);
        // SAFETY: as in `down`; `_touchesEvent` is the application's one touches event,
        // emptied and given this touch and its digitizer event before it is sent.
        unsafe {
            let touch = self.touch;
            let _: () = msg_send![touch, setTimestamp: uptime()];
            if phase != BEGAN {
                let _: () = msg_send![touch, _setLocationInWindow: at, resetPrevious: false];
            }
            let _: () = msg_send![touch, setPhase: phase];
            let _: () = msg_send![touch, _setHidEvent: HidEvent(hid)];
            let application: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
            let event: *mut AnyObject = msg_send![application, _touchesEvent];
            let _: () = msg_send![event, _clearTouches];
            let _: () = msg_send![event, _setHIDEvent: HidEvent(hid)];
            let _: () = msg_send![event, _addTouch: touch, forDelayedDelivery: false];
            let _: () = msg_send![application, sendEvent: event];
            CFRelease(hid);
        }
        let _ = self.window;
    }
}

impl Drop for Finger {
    fn drop(&mut self) {
        // SAFETY: the touch was made by `new` in `down`.
        unsafe {
            let _: () = msg_send![self.touch, release];
        }
    }
}

/// A finger from `from` to `to` (window points) in `steps` moves 16 ms apart, then lifted.
async fn drag(cx: &mut AsyncApp, window: *mut AnyObject, from: CGPoint, to: CGPoint, steps: u32) {
    let finger = Finger::down(window, from);
    for step in 1..=steps {
        wait(cx, 16).await;
        let t = f64::from(step) / f64::from(steps);
        let at = CGPoint {
            x: from.x + (to.x - from.x) * t,
            y: from.y + (to.y - from.y) * t,
        };
        finger.send(at, MOVED);
    }
    wait(cx, 16).await;
    finger.send(to, ENDED);
}

/// A finger down and up at `at`.
async fn tap(cx: &mut AsyncApp, window: *mut AnyObject, at: CGPoint) {
    let finger = Finger::down(window, at);
    wait(cx, 60).await;
    finger.send(at, ENDED);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSizeE {
    width: f64,
    height: f64,
}

// SAFETY: CoreGraphics' `CGSize`.
unsafe impl objc2::encode::Encode for CGSizeE {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CGSize",
        &[
            objc2::encode::Encoding::Double,
            objc2::encode::Encoding::Double,
        ],
    );
}

fn nsstring(text: &str) -> *mut AnyObject {
    let text = std::ffi::CString::new(text).expect("no NUL");
    // SAFETY: `stringWithUTF8String:` copies the bytes into an autoreleased string.
    unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] }
}

/// A host document like an inline page's: the page in a frame sandboxed to scripts that
/// fills it, `inner` the frame's body. A tap posts `tapped`, which the host takes as its
/// title. Its root is not `overflow:
/// hidden`: WebKit would turn its scroll view off, and a scroll view that cannot scroll
/// between the frame's and the container stops UIKit passing an edge's pan on.
fn page(inner: &str) -> String {
    format!(
        "<!doctype html><html><head><meta name=viewport content='width=device-width'>\
         <style>html,body{{margin:0;height:100%}}\
         iframe{{display:block;border:0;width:100%;height:100%}}</style>\
         <script>addEventListener('message',e=>{{document.title=e.data}})</script></head>\
         <body><iframe id=view sandbox=allow-scripts srcdoc=\"<body style='margin:0' \
         onclick='parent.postMessage(&quot;tapped&quot;,&quot;*&quot;)'>{inner}</body>\">\
         </iframe></body></html>",
    )
}

/// The first scroll view under `view` other than `except`, depth first.
fn inner_scroll_view(view: *mut AnyObject, except: *mut AnyObject) -> Option<*mut AnyObject> {
    // SAFETY: walks a live view hierarchy on the main thread.
    unsafe {
        let subviews: *mut AnyObject = msg_send![view, subviews];
        let count: usize = msg_send![subviews, count];
        for index in 0..count {
            let child: *mut AnyObject = msg_send![subviews, objectAtIndex: index];
            let scrolls: bool = msg_send![child, isKindOfClass: class!(UIScrollView)];
            if scrolls && child != except {
                return Some(child);
            }
            if let Some(found) = inner_scroll_view(child, except) {
                return Some(found);
            }
        }
        None
    }
}

pub fn open(cx: &mut App) {
    let window: WindowHandle<Stage> = cx
        .open_window(WindowOptions::default(), |window, cx| {
            let host = window
                .create_native_host(NativeHostOptions::default(), cx)
                .expect("iOS composes natives");
            cx.new(|_| Stage {
                host,
                scroll: ScrollHandle::new(),
            })
        })
        .expect("open the scroll window");
    cx.activate(true);
    cx.spawn(async move |cx| {
        let failures = Failures(RefCell::new(0));
        run(cx, window, &failures).await;
        let failed = *failures.0.borrow();
        println!("{failed} checks failed");
        std::process::exit(i32::try_from(failed).unwrap_or(i32::MAX));
    })
    .detach();
}

/// Scrolls the chat to `y` (negative: down the chat) and waits out any fling.
async fn chat_at(cx: &mut AsyncApp, window: WindowHandle<Stage>, y: f32) {
    wait(cx, 2500).await;
    window
        .update(cx, |stage, _, cx| {
            stage.scroll.set_offset(point(px(0.), px(y)));
            cx.notify();
        })
        .expect("the window is open");
    wait(cx, 200).await;
}

/// `scroll_view`'s vertical offset.
fn offset_of(scroll_view: *mut AnyObject) -> f64 {
    // SAFETY: a live scroll view of this app; main thread.
    let at: CGPoint = unsafe { msg_send![scroll_view, contentOffset] };
    at.y
}

async fn run(cx: &mut AsyncApp, window: WindowHandle<Stage>, failures: &Failures) {
    wait(cx, 100).await;
    let (host, scroll) = window
        .update(cx, |stage, _, _| (stage.host.clone(), stage.scroll.clone()))
        .expect("the window is open");
    // SAFETY: UIKit and WebKit calls on this app's own views, on the main thread.
    let (web, main_scroll, ui_window) = unsafe {
        let config: *mut AnyObject = msg_send![class!(WKWebViewConfiguration), new];
        let web: *mut AnyObject = msg_send![class!(WKWebView), alloc];
        let web: *mut AnyObject = msg_send![web,
            initWithFrame: CGRect { x: 0., y: 0., width: 300., height: 300. },
            configuration: config];
        let _: () = msg_send![config, release];
        host.attach_view(NonNull::new(web.cast()).expect("a view"))
            .expect("attach the page");
        let main_scroll: *mut AnyObject = msg_send![web, scrollView];
        // As an inline page's: its document never moves under the window's safe area.
        let _: () = msg_send![main_scroll, setContentInsetAdjustmentBehavior: 2isize];
        let container: *mut AnyObject = msg_send![web, superview];
        let ui_window: *mut AnyObject = msg_send![container, window];
        (web, main_scroll, ui_window)
    };
    let load = |html: &str| {
        // SAFETY: as above.
        unsafe {
            let _: *mut AnyObject = msg_send![web,
                loadHTMLString: nsstring(html),
                baseURL: std::ptr::null::<AnyObject>()];
        }
    };
    let offset = |cx: &mut AsyncApp| {
        window
            .update(cx, |_, _, _| f32::from(scroll.offset().y))
            .expect("the window is open")
    };
    let title = || -> String {
        // SAFETY: as above; `title` is a copied string or nil.
        unsafe {
            let title: *mut AnyObject = msg_send![web, title];
            if title.is_null() {
                return String::new();
            }
            let utf8: *const std::ffi::c_char = msg_send![title, UTF8String];
            std::ffi::CStr::from_ptr(utf8)
                .to_string_lossy()
                .into_owned()
        }
    };
    let x = 150.;
    // A vertical drag of 160 pt over the page's middle, with the chat at `chat`.
    let drag_over_page = |chat: f32, down: bool| {
        let middle = f64::from(PAGE_TOP + PAGE_HEIGHT / 2. + chat);
        let (from, to) = if down { (-80., 80.) } else { (80., -80.) };
        (
            CGPoint {
                x,
                y: middle + from,
            },
            CGPoint { x, y: middle + to },
        )
    };

    // A page with nothing to scroll: the drag scrolls the chat, the whole of it, and the
    // fling after the lift carries it further. Nothing of the page moves.
    load(&page("<div style='height:120px'>short</div>"));
    wait(cx, 1500).await;
    let rest = offset_of(main_scroll);
    let before = offset(cx);
    let (from, to) = drag_over_page(before, false);
    drag(cx, ui_window, from, to, 10).await;
    let lifted = offset(cx);
    let page_moved = (offset_of(main_scroll) - rest).abs();
    wait(cx, 600).await;
    let coasted = offset(cx);
    failures.check(
        before - lifted > 140.,
        format!("a drag over a page with nothing to scroll scrolls the chat: {before} -> {lifted}"),
    );
    failures.check(
        lifted - coasted > 20.,
        format!("the chat flings on after the lift: {lifted} -> {coasted}"),
    );
    failures.check(
        page_moved < 0.5,
        format!("the page's own view stays still: moved {page_moved}"),
    );

    // A tap on the page is the page's, and moves nothing.
    chat_at(cx, window, 0.).await;
    let before = offset(cx);
    tap(
        cx,
        ui_window,
        CGPoint {
            x,
            y: f64::from(PAGE_TOP) + 30.,
        },
    )
    .await;
    wait(cx, 400).await;
    failures.check(
        title() == "tapped",
        format!("a tap reaches the page: {:?}", title()),
    );
    failures.check(
        (offset(cx) - before).abs() < 0.5,
        "a tap on the page moves the chat nothing",
    );

    // A page that scrolls: a drag that scrolls it is its own, and the chat stays.
    chat_at(cx, window, 0.).await;
    load(&page(
        "<div style='height:3000px;background:linear-gradient(red,blue)'>tall</div>",
    ));
    wait(cx, 1500).await;
    let Some(inner) = inner_scroll_view(web, main_scroll) else {
        failures.check(
            false,
            "the page's frame scrolls in a scroll view of its own",
        );
        return;
    };
    let before = offset(cx);
    let (from, to) = drag_over_page(before, false);
    drag(cx, ui_window, from, to, 10).await;
    wait(cx, 600).await;
    failures.check(
        offset_of(inner) > 100.,
        format!(
            "a drag the page can take scrolls the page: {}",
            offset_of(inner)
        ),
    );
    failures.check(
        (offset(cx) - before).abs() < 0.5,
        format!("…and not the chat: {before} -> {}", offset(cx)),
    );

    // At its top, a drag down goes on to the chat, and the page stays at its top.
    let set_inner = |y: f64| {
        // SAFETY: as above.
        unsafe {
            let _: () = msg_send![inner, setContentOffset: CGPoint { x: 0., y }, animated: false];
        }
    };
    chat_at(cx, window, -150.).await;
    set_inner(0.);
    let before = offset(cx);
    let (from, to) = drag_over_page(before, true);
    drag(cx, ui_window, from, to, 10).await;
    wait(cx, 600).await;
    failures.check(
        offset(cx) - before > 100.,
        format!(
            "at the page's top, a drag down scrolls the chat: {before} -> {}",
            offset(cx)
        ),
    );
    failures.check(
        offset_of(inner).abs() < 0.5,
        format!("…and the page stays at its top: {}", offset_of(inner)),
    );

    // At its bottom, a drag up goes on to the chat.
    chat_at(cx, window, 0.).await;
    // SAFETY: as above.
    let bottom = unsafe {
        let size: CGSizeE = msg_send![inner, contentSize];
        let bounds: CGRect = msg_send![inner, bounds];
        size.height - bounds.height
    };
    set_inner(bottom);
    let before = offset(cx);
    let (from, to) = drag_over_page(before, false);
    drag(cx, ui_window, from, to, 10).await;
    wait(cx, 600).await;
    failures.check(
        before - offset(cx) > 100.,
        format!(
            "at the page's bottom, a drag up scrolls the chat: {before} -> {}",
            offset(cx)
        ),
    );
    failures.check(
        (offset_of(inner) - bottom).abs() < 0.5,
        format!(
            "…and the page stays at its bottom: {} of {bottom}",
            offset_of(inner)
        ),
    );

    // Near its top, a drag down scrolls the page to its top and no further: UIKit keeps a
    // drag with the scroll view it began in, as Safari keeps one with the frame it began in.
    chat_at(cx, window, -150.).await;
    set_inner(40.);
    let before = offset(cx);
    let (from, to) = drag_over_page(before, true);
    drag(cx, ui_window, from, to, 10).await;
    wait(cx, 1500).await;
    failures.check(
        offset_of(inner).abs() < 0.5,
        format!(
            "a drag down near the page's top scrolls it to its top: {}",
            offset_of(inner)
        ),
    );
    failures.check(
        (offset(cx) - before).abs() < 0.5,
        format!("…and the drag stays the page's: {before} -> {}", offset(cx)),
    );
}
