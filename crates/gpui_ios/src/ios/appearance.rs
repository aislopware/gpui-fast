//! A window's own appearance on iOS (`Window::set_appearance`), and the
//! application's, observed (`App::on_window_appearance_change`).
//!
//! A window's own appearance is its `UIWindow.overrideUserInterfaceStyle`,
//! which reaches the window's views and its view controller, so the status
//! bar, the keyboard and the menus UIKit shows over it take it too. iOS has
//! no vibrant appearances: a vibrant light is light there, a vibrant dark
//! dark. The override flows down from the window, so the window scene's own
//! `userInterfaceStyle` stays the system's: that is the application's
//! appearance, and the scene's trait changes tell when it moves.
//!
//! Both are observed with `registerForTraitChanges:withHandler:` (UIKit's
//! `UITraitChangeObservable`), which UIKit keeps for as long as the view or
//! scene it is registered on lives.

use std::cell::RefCell;

use gpui::WindowAppearance;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{class, msg_send};

/// `UIUserInterfaceStyle` (an `NSInteger`): what `appearance` overrides a
/// window's style with, `Unspecified` following the scene's.
fn style(appearance: Option<WindowAppearance>) -> isize {
    match appearance {
        None => 0,
        Some(WindowAppearance::Light | WindowAppearance::VibrantLight) => 1,
        Some(WindowAppearance::Dark | WindowAppearance::VibrantDark) => 2,
    }
}

/// The appearance of `environment`, a `UITraitEnvironment` (a view, a
/// window or a scene): its trait collection's `userInterfaceStyle`.
///
/// # Safety
///
/// `environment` is a live object conforming to `UITraitEnvironment`.
pub(crate) unsafe fn of(environment: *mut AnyObject) -> WindowAppearance {
    // SAFETY: the caller's; `traitCollection` is never nil.
    let style: isize = unsafe {
        let traits: *mut AnyObject = msg_send![environment, traitCollection];
        msg_send![traits, userInterfaceStyle]
    };
    if style == 2 {
        WindowAppearance::Dark
    } else {
        WindowAppearance::Light
    }
}

/// Gives `window`, a `UIWindow`, its own appearance, or with `None` has it
/// follow its scene's again.
///
/// # Safety
///
/// `window` is a live `UIWindow`, on the main thread.
pub(crate) unsafe fn set_window(window: *mut AnyObject, appearance: Option<WindowAppearance>) {
    // SAFETY: the caller's; `overrideUserInterfaceStyle` is a `UIView`
    // property taking a `UIUserInterfaceStyle`.
    unsafe {
        let _: () = msg_send![window, setOverrideUserInterfaceStyle: style(appearance)];
    }
}

/// Calls `handler` each time `observable`'s `userInterfaceStyle` changes.
///
/// # Safety
///
/// `observable` is a live object conforming to `UITraitChangeObservable` (a
/// view, a view controller, a window scene), on the main thread.
pub(crate) unsafe fn observe_style(observable: *mut AnyObject, handler: impl Fn() + 'static) {
    let block = block2::RcBlock::new(
        move |_environment: *mut AnyObject, _previous: *mut AnyObject| {
            handler();
        },
    );
    // SAFETY: the caller's; `UITraitUserInterfaceStyle` is UIKit's trait
    // class for the style, and UIKit copies the block and keeps it, with the
    // registration it returns, for as long as `observable` lives.
    unsafe {
        let trait_class: *const AnyObject =
            std::ptr::from_ref::<AnyClass>(class!(UITraitUserInterfaceStyle)).cast();
        let traits: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: trait_class];
        let _registration: *mut AnyObject = msg_send![
            observable,
            registerForTraitChanges: traits,
            withHandler: &*block
        ];
    }
}

/// Who is told of the application's appearance, and the scene observed for
/// it.
#[derive(Default)]
struct AppObservation {
    callback: Option<Box<dyn FnMut()>>,
    scene: Option<usize>,
}

thread_local! {
    static APP: RefCell<AppObservation> = RefCell::default();
}

/// The application's appearance: the window scene's, which no window's own
/// reaches, else, before a scene connects, the key window's.
pub(crate) fn app() -> WindowAppearance {
    let scene = super::ffi::window_scene();
    if !scene.is_null() {
        // SAFETY: the scene the delegate handed over, alive while connected;
        // `UIWindowScene` conforms to `UITraitEnvironment`.
        return unsafe { of(scene) };
    }
    // SAFETY: UIKit's shared application on the main thread; a non-nil key
    // window is a `UIWindow`, a `UITraitEnvironment`.
    unsafe {
        let application: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        let key_window: *mut AnyObject = msg_send![application, keyWindow];
        if key_window.is_null() {
            WindowAppearance::Light
        } else {
            of(key_window)
        }
    }
}

/// Calls `callback` each time the application's appearance changes, in
/// place of the callback given before.
pub(crate) fn observe_app(callback: Box<dyn FnMut()>) {
    APP.with(|app| app.borrow_mut().callback = Some(callback));
    observe_scene();
}

/// Observes the connected window scene's style for the application, once
/// per scene. Called when the callback is given and when a scene connects,
/// whichever comes last.
pub(crate) fn observe_scene() {
    let scene = super::ffi::window_scene();
    if scene.is_null() {
        return;
    }
    let first =
        APP.with(|app| app.borrow_mut().scene.replace(scene as usize) != Some(scene as usize));
    if !first {
        return;
    }
    // SAFETY: the connected `UIWindowScene`, a `UITraitChangeObservable`, on
    // the main thread where the delegate hands it over.
    unsafe { observe_style(scene, tell_app) };
}

fn tell_app() {
    // The callback is taken out while it runs, so one it installs or a
    // change it sets off cannot find it borrowed.
    let Some(mut callback) = APP.with(|app| app.borrow_mut().callback.take()) else {
        return;
    };
    callback();
    APP.with(|app| {
        app.borrow_mut().callback.get_or_insert(callback);
    });
}
