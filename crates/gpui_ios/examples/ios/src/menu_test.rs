//! The main menu on iOS, checked inside the simulator app: the menus are handed to UIKit, a
//! command picked from them reaches the focused view's action through the responder chain, a
//! command whose action nothing handles is refused, and a key command pressed with its chord's
//! modifiers held goes to the window as that keystroke. Each check prints `ok` or `FAIL`, and
//! the app exits with the number of failures.
//!
//! ```text
//! IPHONEOS_DEPLOYMENT_TARGET=15.0 cargo build -p gpui_ios_example --features menu-test \
//!     --target aarch64-apple-ios-sim
//! (cd crates/gpui_ios/examples/ios/app && xcodegen generate)
//! xcodebuild -project crates/gpui_ios/examples/ios/app/GPUIIosExample.xcodeproj \
//!     -scheme GPUIIosExample -sdk iphonesimulator -configuration Debug \
//!     -derivedDataPath target/gpui-ios-example-xcode CODE_SIGNING_ALLOWED=NO build
//! xcrun simctl install <ipad> target/gpui-ios-example-xcode/Build/Products/Debug-iphonesimulator/GPUIIosExample.app
//! SIMCTL_CHILD_GPUI_IOS_MENU_TEST=1 xcrun simctl launch --console-pty <ipad> dev.zed.gpui-ios-example
//! ```

use std::{cell::RefCell, time::Duration};

use gpui::{
    App, AsyncApp, Context, FocusHandle, KeyBinding, Menu, MenuItem, Window, WindowHandle,
    WindowOptions, actions, div, prelude::*,
};
use gpui_ios::described::{DescribedInput, DescribedPress, PressPhase};
use gpui_ios::hardware_keyboard::{COMMAND, SHIFT};
use objc2::{class, msg_send, runtime::AnyObject, sel};

actions!(menu_test, [Ping, Nothing]);

struct Pad {
    focus: FocusHandle,
    pinged: usize,
}

impl Render for Pad {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context("Pad")
            .track_focus(&self.focus)
            .on_action(cx.listener(|pad, _: &Ping, _, _| pad.pinged += 1))
            .child(format!("Pinged {}", self.pinged))
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

pub fn open(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-shift-k", Ping, None)]);
    cx.set_menus([
        Menu::new("Example").items([MenuItem::action("Ping", Ping)]),
        Menu::new("Tools").items([MenuItem::action("Nothing", Nothing)]),
    ]);
    let window: WindowHandle<Pad> = cx
        .open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                Pad { focus, pinged: 0 }
            })
        })
        .expect("open the menu window");
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

/// A command as the menu system made it for the item at `index`: a key command for `Ping`'s
/// chord, a plain command otherwise.
fn command(index: usize, key: bool) -> *mut AnyObject {
    // SAFETY: UIKit constructors on the main thread; both return autoreleased commands.
    unsafe {
        let title: *mut AnyObject = msg_send![class!(NSString), string];
        let image = std::ptr::null_mut::<AnyObject>();
        let list: *mut AnyObject = msg_send![class!(NSNumber), numberWithUnsignedInteger: index];
        if key {
            let input: *mut AnyObject =
                msg_send![class!(NSString), stringWithUTF8String: c"k".as_ptr()];
            msg_send![
                class!(UIKeyCommand),
                commandWithTitle: title,
                image: image,
                action: sel!(gpuiMenuCommand:),
                input: input,
                modifierFlags: (COMMAND | SHIFT) as isize,
                propertyList: list
            ]
        } else {
            msg_send![
                class!(UICommand),
                commandWithTitle: title,
                image: image,
                action: sel!(gpuiMenuCommand:),
                propertyList: list
            ]
        }
    }
}

fn press(usage: u32, flags: u32, phase: PressPhase) {
    let input = DescribedInput::Press(DescribedPress {
        usage,
        flags,
        phase,
    });
    if let Err(error) = gpui_ios::inject(input) {
        println!("FAIL inject: {error}");
    }
}

async fn run(cx: &mut AsyncApp, window: WindowHandle<Pad>, failures: &Failures) {
    frames(cx, 5).await;
    let active = cx.update(|cx| cx.active_window());
    failures.check(
        active == Some(window.into()),
        "the active window is the one opened",
    );
    let menus = cx.update(|cx| cx.get_menus()).unwrap_or_default();
    failures.check(menus.len() == 2, "the menus are kept");
    let built = gpui_ios::built_menus();
    failures.check(
        built == ["dev.gpui.menu.0", "dev.gpui.menu.1"],
        format!("UIKit built the main menu with both: {built:?}"),
    );

    // SAFETY: the app's own UIApplication, asked on the main thread.
    let (ping_target, nothing_target, sent) = unsafe {
        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        let ping = command(0, false);
        let nothing = command(1, false);
        // Asked from the window's views, where the responder chain starts (UIApplication's
        // own `targetForAction:` starts at itself).
        let key_window: *mut AnyObject = msg_send![app, keyWindow];
        let controller: *mut AnyObject = msg_send![key_window, rootViewController];
        let view: *mut AnyObject = msg_send![controller, view];
        let ping_target: *mut AnyObject =
            msg_send![view, targetForAction: sel!(gpuiMenuCommand:), withSender: ping];
        let nothing_target: *mut AnyObject =
            msg_send![view, targetForAction: sel!(gpuiMenuCommand:), withSender: nothing];
        let sent: bool = msg_send![
            app,
            sendAction: sel!(gpuiMenuCommand:),
            to: std::ptr::null_mut::<AnyObject>(),
            from: ping,
            forEvent: std::ptr::null_mut::<AnyObject>()
        ];
        (ping_target, nothing_target, sent)
    };
    // SAFETY: `ping_target` is nil or an object UIKit returned.
    let is_controller: bool = !ping_target.is_null()
        && unsafe { msg_send![ping_target, isKindOfClass: class!(GPUIViewController)] };
    failures.check(
        is_controller,
        "the controller takes a command whose action is available",
    );
    failures.check(
        nothing_target.is_null(),
        "nobody takes a command whose action nothing handles",
    );
    failures.check(sent, "a picked command is sent");
    frames(cx, 2).await;
    let pinged = window.update(cx, |pad, _, _| pad.pinged).unwrap_or(0);
    failures.check(pinged == 1, format!("the pick ran the action: {pinged}"));

    // ⌘⇧ held, then the key command UIKit matched for ⌘⇧K.
    press(0xE3, COMMAND, PressPhase::Began);
    press(0xE1, COMMAND | SHIFT, PressPhase::Began);
    // SAFETY: as above.
    let sent: bool = unsafe {
        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        msg_send![
            app,
            sendAction: sel!(gpuiMenuCommand:),
            to: std::ptr::null_mut::<AnyObject>(),
            from: command(0, true),
            forEvent: std::ptr::null_mut::<AnyObject>()
        ]
    };
    press(0xE1, COMMAND, PressPhase::Ended);
    press(0xE3, 0, PressPhase::Ended);
    frames(cx, 2).await;
    let pinged = window.update(cx, |pad, _, _| pad.pinged).unwrap_or(0);
    failures.check(
        sent && pinged == 2,
        format!("a pressed chord goes through the keymap: {pinged}"),
    );
}
