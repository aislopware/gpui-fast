//! How many callbacks the app holds, for tests that open and close things
//! many times and require nothing to be left subscribed.

use crate::{App, TestAppContext, Window};

/// How many callbacks of each kind the app holds. A callback is counted from
/// when it is registered until its [`crate::Subscription`] is dropped, its
/// entity or window is released, or it asks to be removed.
///
/// A view a window stops drawing is held by the frame it was last drawn in
/// until the window draws twice more, so its callbacks are counted until
/// then.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubscriptionCounts {
    /// `observe` callbacks, run when an entity is notified.
    pub observers: usize,
    /// `subscribe` callbacks, run when an entity emits an event.
    pub event_listeners: usize,
    /// `observe_release` and `on_release` callbacks.
    pub release_listeners: usize,
    /// `observe_new` callbacks, run when an entity of a type is created.
    pub new_entity_observers: usize,
    /// `observe_global` callbacks.
    pub global_observers: usize,
    /// `observe_keystrokes` and `intercept_keystrokes` callbacks.
    pub keystroke_observers: usize,
    /// The app's other callbacks: keyboard layout, thermal state, sleep,
    /// wake, quit, restart and window closed.
    pub other_app_observers: usize,
    /// The callbacks every open window holds: focus in and out, focus lost,
    /// bounds, appearance, button layout, visibility, presented frames,
    /// activation and pending input.
    pub window_observers: usize,
}

impl SubscriptionCounts {
    /// Every callback counted.
    pub fn total(&self) -> usize {
        self.observers
            + self.event_listeners
            + self.release_listeners
            + self.new_entity_observers
            + self.global_observers
            + self.keystroke_observers
            + self.other_app_observers
            + self.window_observers
    }
}

impl App {
    /// How many callbacks of each kind the app and its windows hold now. A
    /// window being updated at the time is not counted.
    pub fn subscription_counts(&self) -> SubscriptionCounts {
        SubscriptionCounts {
            observers: self.observers.len(),
            event_listeners: self.event_listeners.len(),
            release_listeners: self.release_listeners.len(),
            new_entity_observers: self.new_entity_observers.len(),
            global_observers: self.global_observers.len(),
            keystroke_observers: self.keystroke_observers.len() + self.keystroke_interceptors.len(),
            other_app_observers: self.keyboard_layout_observers.len()
                + self.thermal_state_observers.len()
                + self.system_sleep_observers.len()
                + self.system_wake_observers.len()
                + self.quit_observers.len()
                + self.restart_observers.len()
                + self.window_closed_observers.len(),
            window_observers: self
                .windows
                .values()
                .filter_map(|window| window.as_deref())
                .map(window_observers)
                .sum(),
        }
    }
}

impl TestAppContext {
    /// [`App::subscription_counts`].
    pub fn subscription_counts(&self) -> SubscriptionCounts {
        self.update(|cx| cx.subscription_counts())
    }
}

fn window_observers(window: &Window) -> usize {
    window.focus_listeners.len()
        + window.focus_lost_listeners.len()
        + window.bounds_observers.len()
        + window.appearance_observers.len()
        + window.button_layout_observers.len()
        + window.visibility_observers.len()
        + window.frame_presented_observers.len()
        + window.activation_observers.len()
        + window.pending_input_observers.len()
}
