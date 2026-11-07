//! What a test window records of what GPUI asked the platform to do, for
//! tests to assert on.

use crate::{AnyWindowHandle, Bounds, Pixels, TestAppContext, Window};

impl TestAppContext {
    /// Sets what the test window says of what it draws with
    /// ([`crate::PlatformWindow::presentation_epoch`]): `None`, as by default,
    /// has every frame presented; a number lets a frame that draws what the
    /// screen shows go unpresented while it stays the same
    /// ([`crate::fast::same_frame`]).
    pub fn set_presentation_epoch(&self, window: AnyWindowHandle, epoch: Option<u64>) {
        self.test_window(window).0.lock().fast_presentation_epoch = epoch;
    }

    /// Every place the platform was asked to put the input method's
    /// candidate window at, in the window's coordinates, oldest first: the
    /// selected bounds of the focused input, each time
    /// [`Window::invalidate_character_coordinates`](crate::Window::invalidate_character_coordinates)
    /// had it look them up again.
    pub fn ime_positions(&self, window: AnyWindowHandle) -> Vec<Bounds<Pixels>> {
        self.test_window(window).0.lock().fast_ime_positions.clone()
    }
}

impl Window {
    /// The bounds the element marked with `selector` (`debug_selector`) took
    /// in the frame drawn last, as
    /// [`VisualTestContext::debug_bounds`](crate::VisualTestContext::debug_bounds)
    /// reads them, for a test that must look at a frame before the update it
    /// draws in ends and draws whatever that frame left dirty.
    pub fn debug_bounds(&self, selector: &str) -> Option<Bounds<Pixels>> {
        self.rendered_frame.debug_bounds.get(selector).copied()
    }
}

/// Records that the platform was asked to put the input method's candidate
/// window at `bounds`, for [`TestAppContext::ime_positions`].
pub(crate) fn note_ime_position(window: &crate::platform::TestWindow, bounds: Bounds<Pixels>) {
    window.0.lock().fast_ime_positions.push(bounds);
}
