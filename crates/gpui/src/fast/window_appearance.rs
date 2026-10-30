//! A window's own appearance, apart from the application's.
//!
//! [`App::set_window_appearance`] sets the appearance of every window at
//! once, and each window follows it. A terminal needs one window to differ:
//! a window showing a program that paints a dark background wants the dark
//! appearance for what the platform draws over that background (on macOS the
//! traffic lights, menus, popovers and scrollers) while the application, and
//! a theme that follows it, stays light. [`Window::set_appearance`] gives one
//! window its own appearance until it is cleared.
//!
//! [`Window::appearance`] and [`Window::observe_window_appearance`] report
//! the window's appearance, its own where it has one. The application's is
//! [`App::window_appearance`], told by [`App::on_window_appearance_change`],
//! and no window's own appearance changes it: a window with its own does not
//! follow the system's light and dark, so a theme reading that window would
//! stop following the system too.

use std::rc::Rc;

use crate::{App, AppCell, Platform, Subscription, Window, WindowAppearance};

type Handler = Box<dyn FnMut(&mut App) -> bool + 'static>;

/// Who [`App::on_window_appearance_change`] tells.
pub(crate) struct AppAppearanceObservers(crate::subscription::SingleSubscriberSet<Handler>);

impl Default for AppAppearanceObservers {
    fn default() -> Self {
        Self(crate::subscription::SingleSubscriberSet::new())
    }
}

/// Has `platform` tell the application's observers when its appearance
/// changes. They are told after the moment the platform says: AppKit says it
/// while the appearance is being set, and [`App::set_window_appearance`] sets
/// it from inside an update, with the application borrowed.
pub(crate) fn observe_platform(platform: &dyn Platform, app: &Rc<AppCell>) {
    let app = Rc::downgrade(app);
    let executor = platform.foreground_executor();
    platform.on_window_appearance_change(Box::new(move || {
        let app = app.clone();
        executor
            .spawn(async move {
                if let Some(app) = app.upgrade() {
                    let cx = &mut app.borrow_mut();
                    cx.fast_appearance_observers
                        .0
                        .clone()
                        .retain(|callback| callback(cx));
                }
            })
            .detach();
    }));
}

impl App {
    /// Invokes `callback` when the application's appearance changes, the one
    /// [`App::window_appearance`] reports: the system's light or dark, or
    /// what [`App::set_window_appearance`] set in its place. A window's own
    /// appearance ([`Window::set_appearance`]) never changes it, and a window
    /// with its own is not told of the system's changes, so code that follows
    /// the application, a theme, observes it here rather than through a
    /// window.
    pub fn on_window_appearance_change(
        &self,
        mut callback: impl FnMut(&mut App) + 'static,
    ) -> Subscription {
        self.fast_appearance_observers.0.insert(Box::new(move |cx| {
            callback(cx);
            true
        }))
    }
}

impl Window {
    /// Gives this window its own appearance, apart from the application's,
    /// or with `None` has it follow the application's again.
    ///
    /// On macOS this sets the window's `NSWindow.appearance`, which decides
    /// what AppKit draws in the window (the traffic lights, menus, popovers
    /// and scrollers) and stops the window following the system's light and
    /// dark; on iOS the window's `overrideUserInterfaceStyle` (a vibrant
    /// appearance is its light or dark there). Other platforms keep the
    /// application's.
    ///
    /// [`Window::appearance`] and the window's
    /// [`Window::observe_window_appearance`] observers report the change once
    /// the platform tells the window, as with any change of its appearance.
    pub fn set_appearance(&self, appearance: Option<WindowAppearance>) {
        self.platform_window.set_appearance(appearance);
    }
}

/// A test window's appearance: its own where it has one, else the one it
/// was given (`TestWindow::simulate_appearance_change`).
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
pub(crate) fn of_test_window(window: &crate::platform::TestWindow) -> WindowAppearance {
    let state = window.0.lock();
    state.fast_appearance.unwrap_or(state.appearance)
}

/// Gives a test window its own appearance, and tells it as the platform
/// would, when that changes the appearance it reports.
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
pub(crate) fn set_test_window(
    window: &crate::platform::TestWindow,
    appearance: Option<WindowAppearance>,
) {
    let before = of_test_window(window);
    window.0.lock().fast_appearance = appearance;
    if of_test_window(window) != before {
        let callback = window.0.lock().appearance_change_callback.take();
        if let Some(mut callback) = callback {
            callback();
            window.0.lock().appearance_change_callback = Some(callback);
        }
    }
}

/// The test platform's application appearance: the system's, which a test
/// sets ([`crate::TestAppContext::simulate_system_appearance`]), the one
/// [`App::set_window_appearance`] set in its place, and who is told of a
/// change.
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
#[derive(Default)]
pub(crate) struct TestAppearance {
    system: WindowAppearance,
    set: Option<WindowAppearance>,
    callback: Option<Box<dyn FnMut()>>,
}

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
impl TestAppearance {
    pub(crate) fn current(this: &std::cell::RefCell<Self>) -> WindowAppearance {
        let this = this.borrow();
        this.set.unwrap_or(this.system)
    }

    pub(crate) fn set(this: &std::cell::RefCell<Self>, appearance: Option<WindowAppearance>) {
        Self::change(this, |this| this.set = appearance);
    }

    pub(crate) fn observe(this: &std::cell::RefCell<Self>, callback: Box<dyn FnMut()>) {
        this.borrow_mut().callback = Some(callback);
    }

    /// Changes the appearance and tells the platform's observer, as AppKit
    /// does, when the application's appearance moved.
    fn change(this: &std::cell::RefCell<Self>, change: impl FnOnce(&mut Self)) {
        let before = Self::current(this);
        change(&mut this.borrow_mut());
        if Self::current(this) == before {
            return;
        }
        let callback = this.borrow_mut().callback.take();
        if let Some(mut callback) = callback {
            callback();
            this.borrow_mut().callback.get_or_insert(callback);
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl crate::TestAppContext {
    /// Turns the test platform's system appearance to `appearance`, as the
    /// person switching the system between light and dark does. The
    /// application's appearance follows it unless
    /// [`App::set_window_appearance`] set one. Test windows keep theirs: a
    /// test window's appearance is its own, not the application's.
    pub fn simulate_system_appearance(&self, appearance: WindowAppearance) {
        TestAppearance::change(&self.test_platform.fast_appearance, |this| {
            this.system = appearance;
        });
    }
}
