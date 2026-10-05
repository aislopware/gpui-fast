//! Where assistive technology is told a node is: the part of it that is drawn.
//!
//! Upstream records a node's whole layout bounds. A node scrolled out of a scroll container,
//! or cut by an ancestor's `overflow_hidden`, then reports bounds where something else is
//! drawn: VoiceOver frames the wrong place, and its activate (`Action::Click`, which presses
//! the middle of the node's bounds) lands on whatever is drawn there. So a node's bounds are
//! cut to the content mask its ancestors set, the area it can be drawn in, and a node with
//! nothing left to show is not pressed.

use crate::{Bounds, Pixels, Window};

/// `bounds`, a node's layout bounds, cut to what the ancestors' content mask lets show.
pub(crate) fn visible(window: &Window, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    bounds.intersect(&window.content_mask().bounds)
}

/// Whether a node with these (cut) bounds can be pressed: some of it shows.
pub(crate) fn pressable(bounds: &Bounds<Pixels>) -> bool {
    !bounds.is_empty()
}
