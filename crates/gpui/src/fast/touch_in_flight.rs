//! Whether a touch is in flight in a window: a finger on it, or the fling it
//! threw still coasting.
//!
//! Work an app can put off, a web view to make or a page to lay out, stalls
//! the main thread wherever it runs; under a finger that stall is a scroll
//! that stops dead, a drag that jumps and a fling that freezes mid-coast.
//! `Window::touch_in_flight` says when a touch is in flight, so such work can
//! wait for it to end. A view that reads it is built again when it changes,
//! as a view that reads the pointer is when the pointer moves, so the view
//! that put work off is built again, and does it, once the touch is over.

use crate::fast::dependencies::ambient;
use crate::{
    App, Window,
    gestures::{TouchGestureRecognizer, TouchGestureState},
};

impl Window {
    /// Whether a touch is in flight in this window: from the moment a finger
    /// lands until it lifts, whatever it becomes (a tap, a scroll, a long
    /// press, a drag), and after a scroll until the fling it threw has
    /// stopped. A view that reads it is built again when it changes.
    pub fn touch_in_flight(&self) -> bool {
        self.retained_state
            .ambient_reads
            .note::<ambient::TouchInFlight>();
        in_flight(&self.touch_gestures)
    }
}

/// Whether `recognizer` follows a finger or steps a fling.
fn in_flight(recognizer: &TouchGestureRecognizer) -> bool {
    recognizer.momentum.is_some() || !matches!(recognizer.state, TouchGestureState::Idle)
}

/// After the window handled a touch or stepped a fling: when that changed
/// whether a touch is in flight, the views that read it are built again,
/// in a frame asked for here, since a lift that moves nothing and a fling's
/// last step that scrolls nothing ask for none.
pub(crate) fn stamp(window: &mut Window, cx: &mut App) {
    let now = in_flight(&window.touch_gestures);
    if std::mem::replace(&mut window.touch_gestures.fast_fling.in_flight, now) != now {
        cx.ambient_changed::<ambient::TouchInFlight>();
        window.invalidator.set_dirty(true);
    }
}
