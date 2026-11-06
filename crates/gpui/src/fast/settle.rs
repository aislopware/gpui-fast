//! A frame settles before it is shown.
//!
//! An element whose size is known only once it is laid out (wrapped text, a stack another
//! element scrolls under, a card's natural height) measures itself in prepaint, keeps the
//! value, and defers a notify, since a view cannot be notified while it draws. The value then
//! reached layout in the next frame, so the frame shown first was laid out with the old one: a
//! jump the person sees, and a frame the retained check finds stale.
//!
//! A test window never showed that frame: in test mode the effect loop draws a dirty window
//! again until nothing is dirty before a test looks. A platform frame drew once and presented.
//! Now a platform frame does as a test does, within bounds: when the window is dirty again once
//! its draw's update has ended (what the draw deferred has run), the frame it drew is stale, and
//! it is drawn again before it is presented, up to [`PASSES`] more times. A frame that left
//! nothing dirty costs nothing more.

use gpui_util::ResultExt as _;

use crate::{AnyWindowHandle, AsyncApp};

/// How many more times a frame is drawn, at most, before it is shown. A measurement that has
/// not settled after these is shown as it stands and settles in the frames that follow.
pub(crate) const PASSES: usize = 2;

thread_local! {
    /// A test draws its windows only when it asks for a platform frame.
    #[cfg(any(test, feature = "test-support"))]
    static PLATFORM_FRAMES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The bounds of the quads of each frame a platform frame presented, oldest first.
    #[cfg(test)]
    static PRESENTED: std::cell::RefCell<Vec<Vec<crate::Bounds<crate::ScaledPixels>>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// After a platform frame's draw: settle `handle`'s window ([`settle`]), then present it.
pub(crate) fn settle_and_present(handle: AnyWindowHandle, cx: &mut AsyncApp) {
    settle(handle, cx);
    handle
        .update(cx, |_, window, _| {
            window.present();
            #[cfg(test)]
            note_presented(window);
        })
        .log_err();
}

/// Draw `handle`'s window again while what its last draw deferred, run as the draw's update
/// ended, left it dirty: up to [`PASSES`] times. How many more draws it took.
pub(crate) fn settle(handle: AnyWindowHandle, cx: &mut AsyncApp) -> usize {
    let mut passes = 0;
    while passes < PASSES {
        let drew = handle
            .update(cx, |_, window, cx| {
                if !window.invalidator.is_dirty() {
                    return false;
                }
                window.draw(cx).clear(cx);
                true
            })
            .unwrap_or(false);
        if !drew {
            break;
        }
        passes += 1;
    }
    passes
}

/// Whether test windows draw only when a test asks for a platform frame
/// ([`crate::platform::TestWindow::simulate_frame_request`]), as a platform's do, instead of
/// whenever an update leaves them dirty.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn platform_frames() -> bool {
    PLATFORM_FRAMES.with(std::cell::Cell::get)
}

/// Draw test windows only on platform frames, while `on` (this thread's tests).
#[cfg(test)]
pub(crate) fn set_platform_frames(on: bool) {
    PLATFORM_FRAMES.with(|frames| frames.set(on));
}

/// Keep the quads of the frame `window` presented.
#[cfg(test)]
fn note_presented(window: &crate::Window) {
    let quads = window
        .rendered_frame
        .scene
        .quads
        .iter()
        .map(|quad| quad.bounds)
        .collect();
    PRESENTED.with(|presented| presented.borrow_mut().push(quads));
}

/// The bounds of the quads of each frame platform frames presented on this thread, oldest
/// first.
#[cfg(test)]
pub(crate) fn presented() -> Vec<Vec<crate::Bounds<crate::ScaledPixels>>> {
    PRESENTED.with(|presented| presented.borrow().clone())
}
