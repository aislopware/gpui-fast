//! Work an element hands the frame to do when it reaches the screen
//! ([`Window::on_present`]): moving a native child view the platform does
//! not place, in step with the frame it was painted for.
//!
//! The callbacks are kept with the frame painted, in paint order, and run
//! as it is presented, before the platform is handed the scene and the
//! natives. A subtree drawn again from last frame hands them again, as it
//! does its listeners; a stretch holding one is painted again, never moved,
//! and a scroll container's content handing one is drawn without tiles,
//! since a callback holds the bounds it was painted at. A frame painted
//! only to hit test again is never presented, so its callbacks never run.
//!
//! The API is zui's (<https://github.com/zeronsh/zui>,
//! `crates/gpui/src/window.rs`; Apache-2.0, by Wing and the zui
//! contributors; zui is a fork of Zed's GPUI). Natives placed with
//! `native_view` need none of it: the frame places them itself
//! (`fast::composition`).

use std::{ops::Range, rc::Rc};

use crate::{PaintIndex, Window};

/// A callback [`Window::on_present`] was handed.
pub(crate) type OnPresent = Rc<dyn Fn()>;

impl Window {
    /// Runs `callback` when the frame being painted reaches the screen,
    /// each time it does: painting can run to hit test again without
    /// presenting, and a frame drawn again from this one presents it again.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn on_present(&mut self, callback: impl Fn() + 'static) {
        self.invalidator.debug_assert_paint();
        self.next_frame.retained.present.push(Rc::new(callback));
    }
}

/// Hands `range` of last frame's callbacks to the frame being drawn, for a
/// subtree drawn again from last frame.
pub(crate) fn reuse_paint(window: &mut Window, range: &Range<PaintIndex>) {
    let reused =
        &window.rendered_frame.retained.present[range.start.fast.present..range.end.fast.present];
    window
        .next_frame
        .retained
        .present
        .extend(reused.iter().cloned());
}

/// How many callbacks painting handed the present so far: what a scroll
/// layer's tiles can't carry as they move, since a callback holds where it
/// was painted, so content that adds to it is drawn into the frame
/// (`fast::layers`). Edge fades and rounded clips set inside the content
/// move with it ([`crate::fast::edge_fade::Relocation`]).
pub(crate) fn uncarried(window: &Window) -> u64 {
    window.next_frame.retained.present.len() as u64
}

/// Runs the callbacks of the frame being presented, in paint order.
pub(crate) fn run(window: &Window) {
    if window.rendered_frame.retained.present.is_empty() {
        return;
    }
    let callbacks = window.rendered_frame.retained.present.clone();
    for callback in callbacks {
        callback();
    }
}
