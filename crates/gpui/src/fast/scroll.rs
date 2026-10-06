//! Momentum: the scrolling that carries on after the fingers lift, told apart
//! from the scrolling a finger or a wheel drives
//! ([`ScrollWheelEvent::momentum_phase`]), and a scroll handle's item brought into view
//! against the frame it is asked in.

use crate::{Bounds, Pixels, ScrollHandle, ScrollWheelEvent, Style, TouchPhase};

impl ScrollWheelEvent {
    /// This scroll step, as a step of momentum in `phase`.
    pub(crate) fn fast_momentum(mut self, phase: TouchPhase) -> Self {
        self.momentum_phase = Some(phase);
        self
    }
}

/// Brings the item `handle` was asked to show into view against this frame's `bounds` and
/// `style`'s overflow, before the scroll offset is clamped to them.
///
/// Upstream resolves the request at the start of `Div::prepaint`, before the frame's bounds
/// are recorded, so it judges the item against the frame before's: a container that narrowed
/// in the same frame (or one drawn for the first time, with no bounds yet) finds the item
/// visible at its old width, drops the request, and leaves the item past its edge.
pub(crate) fn into_view(handle: Option<&ScrollHandle>, bounds: Bounds<Pixels>, style: &Style) {
    let Some(handle) = handle else {
        return;
    };
    {
        let mut state = handle.0.borrow_mut();
        state.bounds = bounds;
        state.overflow = style.overflow;
    }
    handle.scroll_to_active_item();
}
