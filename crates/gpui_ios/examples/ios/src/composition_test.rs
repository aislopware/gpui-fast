//! Window composition on iOS, checked inside the simulator app: the root view's
//! structure, the containers natives live in, hit testing by direct calls, frames presented
//! inside a transaction, and the keyboard focus both ways. Each check prints `ok` or `FAIL`,
//! and the app exits with the number of failures.
//!
//! ```text
//! crates/gpui_ios/examples/ios/build-simulator.sh
//! xcrun simctl boot 'iPhone 18 Pro'
//! xcrun simctl install booted target/gpui-ios-example-xcode/Build/Products/Debug-iphonesimulator/GPUIIosExample.app
//! SIMCTL_CHILD_GPUI_IOS_COMPOSITION_TEST=1 xcrun simctl launch --console-pty booted dev.zed.gpui-ios-example
//! ```

use std::{cell::RefCell, ptr::NonNull, time::Duration};

use gpui::{
    App, AsyncApp, Context, FocusHandle, Window, WindowHandle, WindowOptions,
    composition::{NativeHost, NativeHostOptions, native_view},
    div,
    prelude::*,
    px, rgb,
};
use gpui_ios::ios::composition::IosNativeHost;
use objc2::{
    class, msg_send,
    runtime::{AnyObject, Bool},
};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

// SAFETY: the layout is CoreGraphics' `CGPoint`: two `CGFloat`s (double on 64-bit).
unsafe impl objc2::encode::Encode for CGPoint {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CGPoint",
        &[
            objc2::encode::Encoding::Double,
            objc2::encode::Encoding::Double,
        ],
    );
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

// SAFETY: the layout is CoreGraphics' `CGRect`: a `CGPoint` then a `CGSize`.
unsafe impl objc2::encode::Encode for CGRect {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CGRect",
        &[
            objc2::encode::Encoding::Struct(
                "CGPoint",
                &[
                    objc2::encode::Encoding::Double,
                    objc2::encode::Encoding::Double,
                ],
            ),
            objc2::encode::Encoding::Struct(
                "CGSize",
                &[
                    objc2::encode::Encoding::Double,
                    objc2::encode::Encoding::Double,
                ],
            ),
        ],
    );
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UIEdgeInsets {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
}

// SAFETY: the layout is UIKit's `UIEdgeInsets`: four `CGFloat`s, top, left, bottom, right.
unsafe impl objc2::encode::Encode for UIEdgeInsets {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "UIEdgeInsets",
        &[
            objc2::encode::Encoding::Double,
            objc2::encode::Encoding::Double,
            objc2::encode::Encoding::Double,
            objc2::encode::Encoding::Double,
        ],
    );
}

struct Stage {
    host: NativeHost,
    native_focus: FocusHandle,
    other_focus: FocusHandle,
    x: f32,
    shown: bool,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
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
                        .top(px(150.))
                        .w(px(240.))
                        .h(px(160.))
                        .track_focus(&self.native_focus),
                )
            })
            .child(
                div()
                    .absolute()
                    .left(px(150.))
                    .top(px(200.))
                    .w(px(60.))
                    .h(px(40.))
                    .bg(rgb(0xcc3030))
                    .occlude(),
            )
            .child(
                div()
                    .id("other")
                    .track_focus(&self.other_focus)
                    .absolute()
                    .left(px(40.))
                    .top(px(500.))
                    .size(px(40.))
                    .bg(rgb(0x3060cc)),
            )
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

async fn frames(cx: &mut AsyncApp, count: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(17 * count))
        .await;
}

fn ios_host(host: &NativeHost) -> &IosNativeHost {
    host.platform()
        .as_any()
        .downcast_ref::<IosNativeHost>()
        .expect("iOS natives")
}

pub fn open(cx: &mut App) {
    let window: WindowHandle<Stage> = cx
        .open_window(WindowOptions::default(), |window, cx| {
            let host = window
                .create_native_host(NativeHostOptions::default(), cx)
                .expect("iOS composes natives");
            cx.new(|cx| Stage {
                host,
                native_focus: cx.focus_handle(),
                other_focus: cx.focus_handle(),
                x: 40.,
                shown: true,
            })
        })
        .expect("open the composition window");
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

async fn run(cx: &mut AsyncApp, window: WindowHandle<Stage>, failures: &Failures) {
    frames(cx, 5).await;
    let host = window
        .update(cx, |stage, _, _| stage.host.clone())
        .expect("the window is open");
    // SAFETY: UIKit calls on this app's own views, on the main thread.
    let (field, container, root, metal_view) = unsafe {
        let field: *mut AnyObject = msg_send![class!(UITextField), alloc];
        let field: *mut AnyObject = msg_send![field, init];
        let color: *mut AnyObject = msg_send![class!(UIColor), systemTealColor];
        let _: () = msg_send![field, setBackgroundColor: color];
        host.attach_view(NonNull::new(field.cast()).expect("a view"))
            .expect("attach the field");
        let _: () = msg_send![field, release];
        let container = ios_host(&host).container_view();
        let root: *mut AnyObject = msg_send![container, superview];
        let subviews: *mut AnyObject = msg_send![root, subviews];
        let count: usize = msg_send![subviews, count];
        let metal_view: *mut AnyObject = msg_send![subviews, objectAtIndex: count - 1];
        (field, container, root, metal_view)
    };
    // SAFETY: as above.
    unsafe {
        let window_view: *mut AnyObject = msg_send![root, window];
        let root_controller: *mut AnyObject = msg_send![window_view, rootViewController];
        let controller_view: *mut AnyObject = msg_send![root_controller, view];
        failures.check(
            controller_view == root,
            "the root view is the view controller's view",
        );
        let is_metal: bool = msg_send![metal_view, isKindOfClass: class!(GPUIMetalView)];
        failures.check(is_metal, "the Metal view is the root's topmost subview");
        let layer: *mut AnyObject = msg_send![container, layer];
        let z: f64 = msg_send![layer, zPosition];
        failures.check(
            z < 0.,
            format!("the container is stacked below GPUI's layer: {z}"),
        );
        let hidden: bool = msg_send![container, isHidden];
        failures.check(!hidden, "a placed native is shown");
        let metal_layer: *mut AnyObject = msg_send![metal_view, layer];
        let opaque: bool = msg_send![metal_layer, isOpaque];
        failures.check(!opaque, "GPUI's layer composites over the hole");
        let frame: CGRect = msg_send![container, frame];
        failures.check(
            (frame.x - 40.).abs() < 0.5
                && (frame.y - 150.).abs() < 0.5
                && (frame.width - 240.).abs() < 0.5,
            format!(
                "the container is where the element is: {} {} {} {}",
                frame.x, frame.y, frame.width, frame.height
            ),
        );
        let root_bounds: CGRect = msg_send![root, bounds];
        let metal_bounds: CGRect = msg_send![metal_view, bounds];
        failures.check(
            root_bounds.width == metal_bounds.width && root_bounds.height == metal_bounds.height,
            "the Metal view fills the root",
        );
    }
    // The safe area GPUI reports still comes through the Metal view under the root.
    // SAFETY: as above.
    let window_insets: UIEdgeInsets = unsafe {
        let window_view: *mut AnyObject = msg_send![root, window];
        msg_send![window_view, safeAreaInsets]
    };
    let gpui_insets = window
        .update(cx, |_, window, _| window.insets().safe_area)
        .expect("the window is open");
    failures.check(
        (f64::from(f32::from(gpui_insets.top)) - window_insets.top).abs() < 0.5
            && (f64::from(f32::from(gpui_insets.bottom)) - window_insets.bottom).abs() < 0.5
            && window_insets.top > 0.,
        format!(
            "the safe area is the window's: top {} bottom {} (window {} {})",
            f32::from(gpui_insets.top),
            f32::from(gpui_insets.bottom),
            window_insets.top,
            window_insets.bottom
        ),
    );

    // Hit testing by direct calls, in the root's coordinates (the Metal view's too).
    // SAFETY: as above.
    unsafe {
        let hit = |x: f64, y: f64| -> *mut AnyObject {
            msg_send![root, hitTest: CGPoint { x, y }, withEvent: std::ptr::null::<AnyObject>()]
        };
        let over_native = hit(60., 170.);
        let in_container: bool = !over_native.is_null() && {
            let inside: bool = msg_send![over_native, isDescendantOfView: container];
            inside
        };
        failures.check(in_container, "over the native, the native is hit");
        failures.check(
            hit(170., 210.) == metal_view,
            "over the overlay, GPUI is hit",
        );
        failures.check(
            hit(60., 520.) == metal_view,
            "beside the native, GPUI is hit",
        );
    }

    // Moving the native is a transactional frame; a frame that moves nothing is not.
    let (transactional_before, _) = ios_host(&host).presents();
    for _ in 0..10 {
        window
            .update(cx, |stage, _, cx| {
                stage.x += 1.;
                cx.notify();
            })
            .expect("the window is open");
        frames(cx, 2).await;
    }
    let (transactional_after, plain_after) = ios_host(&host).presents();
    failures.check(
        transactional_after - transactional_before >= 10,
        format!(
            "frames moving the native were transactional: {}",
            transactional_after - transactional_before
        ),
    );
    window
        .update(cx, |_, _, cx| cx.notify())
        .expect("the window is open");
    frames(cx, 3).await;
    let (transactional_still, plain_still) = ios_host(&host).presents();
    failures.check(
        transactional_still == transactional_after && plain_still > plain_after,
        "a frame moving nothing was plain",
    );

    // GPUI to platform: focusing the native's element gives it the keyboard.
    window
        .update(cx, |stage, window, cx| {
            window.focus(&stage.native_focus, cx)
        })
        .expect("the window is open");
    frames(cx, 10).await;
    // SAFETY: as above.
    let field_focused: bool = unsafe { msg_send![field, isFirstResponder] };
    failures.check(field_focused, "GPUI gave the native the keyboard");
    window
        .update(cx, |stage, window, cx| window.focus(&stage.other_focus, cx))
        .expect("the window is open");
    frames(cx, 10).await;
    // SAFETY: as above.
    let (field_focused, metal_focused): (bool, bool) = unsafe {
        (
            msg_send![field, isFirstResponder],
            msg_send![metal_view, isFirstResponder],
        )
    };
    failures.check(
        !field_focused && metal_focused,
        "GPUI took the keyboard back from the native",
    );

    // Platform to GPUI: the native taking the keyboard focuses its element.
    // SAFETY: as above.
    let _: Bool = unsafe { msg_send![field, becomeFirstResponder] };
    window
        .update(cx, |_, _, cx| cx.notify())
        .expect("the window is open");
    frames(cx, 10).await;
    let native_focused = window
        .update(cx, |stage, window, _| stage.native_focus.is_focused(window))
        .expect("the window is open");
    failures.check(
        native_focused,
        "the native's first responder focused its element",
    );

    // A native no element places is hidden, and GPUI's layer is opaque again.
    window
        .update(cx, |stage, _, cx| {
            stage.shown = false;
            cx.notify();
        })
        .expect("the window is open");
    frames(cx, 5).await;
    // SAFETY: as above.
    unsafe {
        let hidden: bool = msg_send![container, isHidden];
        failures.check(hidden, "an unplaced native is hidden");
        let metal_layer: *mut AnyObject = msg_send![metal_view, layer];
        let opaque: bool = msg_send![metal_layer, isOpaque];
        failures.check(opaque, "with no hole, GPUI's layer is opaque again");
        let observers: *mut AnyObject = msg_send![root, gestureRecognizers];
        let count: usize = msg_send![observers, count];
        let observer: *mut AnyObject = msg_send![observers, objectAtIndex: 0usize];
        let cancels: bool = msg_send![observer, cancelsTouchesInView];
        failures.check(
            count == 1 && !cancels,
            "the root's touch observer never cancels a native's touches",
        );
    }
}
