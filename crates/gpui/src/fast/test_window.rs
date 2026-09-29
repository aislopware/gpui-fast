//! What a test window records of what GPUI asked the platform to do, for
//! tests to assert on.

use crate::{AnyWindowHandle, Bounds, Pixels, TestAppContext};

impl TestAppContext {
    /// Every place the platform was asked to put the input method's
    /// candidate window at, in the window's coordinates, oldest first: the
    /// selected bounds of the focused input, each time
    /// [`Window::invalidate_character_coordinates`](crate::Window::invalidate_character_coordinates)
    /// had it look them up again.
    pub fn ime_positions(&self, window: AnyWindowHandle) -> Vec<Bounds<Pixels>> {
        self.test_window(window).0.lock().fast_ime_positions.clone()
    }
}
