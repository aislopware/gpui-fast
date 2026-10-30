//! A window's own appearance on macOS (`Window::set_appearance`), and the
//! application's, observed (`App::on_window_appearance_change`).
//!
//! A window's own appearance is its `NSWindow.appearance`: AppKit draws the
//! window's traffic lights, menus, popovers and scrollers in it, and a window
//! that has one no longer follows the application's. The application's is
//! `NSApp.effectiveAppearance`, which moves with the system's light and dark
//! and with `NSApp.appearance`, never with a window's. AppKit documents it as
//! key-value observable, and Ghostty's `AppDelegate` follows it that way.

use std::cell::RefCell;
use std::ffi::c_void;

use gpui::WindowAppearance;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSAppearanceNameVibrantDark,
    NSAppearanceNameVibrantLight, NSApplication,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString, ns_string};

use crate::window::MacWindowState;

/// The `NSAppearance` AppKit names for `appearance`.
fn native(appearance: WindowAppearance) -> Option<Retained<NSAppearance>> {
    // SAFETY: AppKit's appearance names are immutable `NSString` constants
    // that live as long as the process.
    let name = unsafe {
        match appearance {
            WindowAppearance::Light => NSAppearanceNameAqua,
            WindowAppearance::Dark => NSAppearanceNameDarkAqua,
            WindowAppearance::VibrantLight => NSAppearanceNameVibrantLight,
            WindowAppearance::VibrantDark => NSAppearanceNameVibrantDark,
        }
    };
    NSAppearance::appearanceNamed(name)
}

/// Gives the window `state` holds its own appearance, or with `None` has it
/// follow the application's again (a nil `NSWindow.appearance`).
///
/// The state's lock is let go before AppKit is asked, since AppKit tells the
/// window's view of the change (`viewDidChangeEffectiveAppearance`) while it
/// sets it, and the view takes the lock.
pub(crate) fn set_window(
    state: &parking_lot::Mutex<MacWindowState>,
    appearance: Option<WindowAppearance>,
) {
    let native_window = state.lock().native_window;
    let appearance = appearance.and_then(native);
    // SAFETY: `native_window` is the live `NSWindow` this window owns, on the
    // main thread where GPUI drives its windows; `setAppearance:` takes an
    // `NSAppearance` or nil.
    unsafe {
        let _: () = msg_send![native_window, setAppearance: appearance.as_deref()];
    }
}

define_class!(
    /// The key-value observer of `NSApp.effectiveAppearance`.
    // SAFETY: `NSObject` has no subclassing requirements, and
    // `AppearanceObserver` holds nothing and does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct AppearanceObserver;

    unsafe impl NSObjectProtocol for AppearanceObserver {}

    impl AppearanceObserver {
        // NSKeyValueObserving: sent on the thread that changed the value,
        // the main thread for `NSApp`'s appearance.
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&AnyObject>,
            _context: *mut c_void,
        ) {
            tell_app();
        }
    }
);

/// `observer`'s registration for `NSApp.effectiveAppearance`, removed when
/// dropped.
struct AppAppearanceObserver {
    app: Retained<NSApplication>,
    observer: Retained<AppearanceObserver>,
}

impl AppAppearanceObserver {
    fn new(mtm: MainThreadMarker) -> Self {
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: `NSObject`'s designated initializer.
        let observer: Retained<AppearanceObserver> =
            unsafe { msg_send![AppearanceObserver::alloc(mtm), init] };
        // SAFETY: NSKeyValueObserving's `addObserver:forKeyPath:options:context:`
        // on the shared application, with an observer that answers
        // `observeValueForKeyPath:ofObject:change:context:`; AppKit keeps no
        // strong reference to it, so `Drop` removes it before it is freed.
        unsafe {
            let _: () = msg_send![
                &*app,
                addObserver: &*observer,
                forKeyPath: ns_string!("effectiveAppearance"),
                options: 0usize,
                context: std::ptr::null_mut::<c_void>()
            ];
        }
        Self { app, observer }
    }
}

impl Drop for AppAppearanceObserver {
    fn drop(&mut self) {
        // SAFETY: removes the registration `new` made, with the same observer
        // and key path.
        unsafe {
            let _: () = msg_send![
                &*self.app,
                removeObserver: &*self.observer,
                forKeyPath: ns_string!("effectiveAppearance")
            ];
        }
    }
}

/// Who is told of the application's appearance, and the observation that
/// tells, made once the shared application exists.
#[derive(Default)]
struct AppObservation {
    callback: Option<Box<dyn FnMut()>>,
    observer: Option<AppAppearanceObserver>,
}

thread_local! {
    static APP: RefCell<AppObservation> = RefCell::default();
}

/// Calls `callback` each time the application's appearance changes, in place
/// of the callback given before. GPUI gives it before the application runs,
/// so it is held until [`app_created`].
pub(crate) fn observe_app(callback: Box<dyn FnMut()>) {
    APP.with(|app| app.borrow_mut().callback = Some(callback));
}

/// Starts observing the application's appearance, once `run` made the shared
/// application: asking for it earlier would make a plain `NSApplication` in
/// place of GPUI's subclass, and a headless application never makes one.
pub(crate) fn app_created(mtm: MainThreadMarker) {
    if APP.with(|app| app.borrow().observer.is_none()) {
        let observer = AppAppearanceObserver::new(mtm);
        APP.with(|app| app.borrow_mut().observer = Some(observer));
    }
}

fn tell_app() {
    // The callback is taken out while it runs, so a change it sets off
    // cannot find it borrowed.
    let Some(mut callback) = APP.with(|app| app.borrow_mut().callback.take()) else {
        return;
    };
    callback();
    APP.with(|app| {
        app.borrow_mut().callback.get_or_insert(callback);
    });
}
