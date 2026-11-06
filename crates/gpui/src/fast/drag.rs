//! When a press on a draggable element becomes a drag.
//!
//! Ported from zui (<https://github.com/zeronsh/zui> `0dc69aa`, "Drags need the button
//! held and 4px of travel on every platform"; Apache-2.0, by Wing).
//!
//! Upstream lifts a press into a drag after 2 px of travel on macOS and Linux and after
//! Windows' drag rectangle elsewhere. 2 px is less than a physical trackpad press rolls
//! the pointer, so a plain click on a draggable row (a file in a tree, a tab) was taken
//! for a drag mid-click and lost its click. And a press whose release the element never
//! saw, because it was not painted in the frame the release came in, stayed pending, so
//! the next plain hover across it became a drag with no button down. Here a press is a
//! drag only while its button is still held, after 4 px of travel, Windows' system drag
//! rectangle (`SM_CXDRAG`/`SM_CYDRAG`), on every platform.

use crate::{MouseDownEvent, MouseMoveEvent};

/// Pointer travel, in logical pixels, past which a held press on a draggable element
/// becomes a drag.
pub(crate) const DRAG_THRESHOLD: f64 = 4.;

/// Forgets a pending press whose button is no longer held as the pointer moves: its
/// release went to no listener of this element, so the move is a hover, not a drag.
/// Returns whether it did, for the listener to stop there.
pub(crate) fn forget_released_press(
    pending: &mut Option<MouseDownEvent>,
    event: &MouseMoveEvent,
) -> bool {
    if pending
        .as_ref()
        .is_some_and(|down| event.pressed_button != Some(down.button))
    {
        *pending = None;
        return true;
    }
    false
}
