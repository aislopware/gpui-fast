//! A mouse wheel's notches glide through a `list` (monocode's `WheelGlide`,
//! its GPUI's `elements/list.rs`; FORK.md, "Onicode").
//!
//! A mouse wheel moves in notches, each a jump of several lines. Rather than
//! teleporting, the list glides through a notch's distance, and further
//! notches add to the distance still to go. Trackpads report exact pixels
//! with their own momentum and are applied as they arrive.
//!
//! The constants and the step are monocode's verbatim: a notch is 20 px a
//! line, the glide closes on what is left with a time constant of 70 ms, a
//! step within half a pixel of the end lands on it, a notch the other way
//! drops what was left of the old one, and with reduced motion a notch jumps
//! as upstream's does. The clock is the executor's (`std::time::Instant`
//! outside tests), so a test steps it.
//!
//! Each glide step is the list's own scroll (`StateInner::scroll`), run from
//! a next-frame callback before the frame draws, so the frame it brings is a
//! scroll like a wheel's: scroll layers composite it (`fast::layers::lists`).
//!
//! monocode lands where its steps' sum takes it, which in `f32` is a hair
//! off the notches' distance (199.99998 px for ten lines). Here the glide
//! keeps the scroll top its notches aim at, summed from where it began, and
//! the landing step puts the list there exactly.

use scheduler::Instant;

use crate::{
    App, EntityId, ListAlignment, ListOffset, ListState, Pixels, ScrollDelta, ScrollEvent, Window,
    point, px,
};

/// Time constant of the glide: about 95% of a notch lands within 3τ.
pub(crate) const WHEEL_GLIDE_TAU: f32 = 0.07;

/// How far one line of a wheel's notch scrolls a list.
pub(crate) const NOTCH_LINE: Pixels = px(20.);

/// The longest step of the clock one glide frame takes: a frame that comes
/// late moves the list no further than one 50 ms after the last.
const MAX_STEP_SECONDS: f32 = 0.05;

/// A glide in progress, in the list's state.
pub(crate) struct WheelGlide {
    /// The distance still to travel, signed as the wheel's delta.
    remaining: Pixels,
    /// The scroll top, in pixels, the glide lands on: where it began, or
    /// where a notch the other way found the list, less every notch since.
    target: Pixels,
    /// When the glide last stepped, or began.
    last: Instant,
    /// The list's viewport height, as its last notch's paint saw it.
    height: Pixels,
    /// The view the list notifies when it scrolls.
    view: EntityId,
}

/// The list's wheel listener, before it scrolls: takes a vertical notch into
/// the glide, starting it if none runs, and returns true. Any other scroll
/// ends the glide and returns false, and the listener scrolls as upstream's.
/// `(scroll_top, height, view)` are the scroll top the list's prepaint laid
/// out, its viewport height and the view that painted it.
pub(crate) fn glides(
    state: &ListState,
    event: &ScrollEvent,
    (scroll_top, height, view): (ListOffset, Pixels, EntityId),
    window: &mut Window,
    cx: &App,
) -> bool {
    let ScrollDelta::Lines(lines) = event.delta else {
        state.0.borrow_mut().fast_glide = None;
        return false;
    };
    if lines.x != 0. || cx.reduce_motion() {
        state.0.borrow_mut().fast_glide = None;
        return false;
    }
    let distance = NOTCH_LINE * lines.y;
    let mut inner = state.0.borrow_mut();
    let base = inner.scroll_anchor.map(|anchor| anchor.position);
    let top = inner.scroll_top(&base.unwrap_or(scroll_top));
    let start = inner.fast_glide.is_none();
    let glide = inner.fast_glide.get_or_insert(WheelGlide {
        remaining: px(0.),
        target: top,
        last: cx.background_executor().now(),
        height,
        view,
    });
    // Reversing direction drops what was left of the old notch.
    if glide.remaining.signum() != distance.signum() {
        glide.remaining = px(0.);
        glide.target = top;
    }
    glide.remaining += distance;
    // A notch's distance is signed as the wheel's delta: down is negative,
    // and moves the scroll top down the content.
    glide.target -= distance;
    glide.height = height;
    drop(inner);
    if start {
        frame(state.clone(), scroll_top, window);
    }
    true
}

/// Advances a wheel glide by one display frame.
fn frame(state: ListState, scroll_top: ListOffset, window: &mut Window) {
    window.on_next_frame(move |window, cx| {
        let mut inner = state.0.borrow_mut();
        let Some(glide) = inner.fast_glide.as_mut() else {
            return;
        };
        let now = cx.background_executor().now();
        let dt = now
            .saturating_duration_since(glide.last)
            .as_secs_f32()
            .min(MAX_STEP_SECONDS);
        glide.last = now;
        let mut step = glide.remaining * (1. - (-dt / WHEEL_GLIDE_TAU).exp());
        if (glide.remaining - step).abs() < px(0.5) {
            step = glide.remaining;
        }
        glide.remaining -= step;
        let (height, view, target) = (glide.height, glide.view, glide.target);
        let done = glide.remaining == px(0.);
        if done {
            inner.fast_glide = None;
        }
        // After the first step the list's own logical position is current.
        // A bottom-aligned list at its end has none, and the glide is over.
        let anchor = inner.scroll_anchor.map(|anchor| anchor.position);
        let base = anchor.unwrap_or(scroll_top);
        let at_end = inner.alignment == ListAlignment::Bottom && anchor.is_none() && step < px(0.);
        if at_end {
            inner.fast_glide = None;
        } else if step != px(0.) {
            inner.scroll(&base, height, point(px(0.), step), view, window, cx);
            if done {
                land(&mut inner, target, height);
            }
        }
        let running = inner.fast_glide.is_some();
        drop(inner);
        if running {
            frame(state, scroll_top, window);
        }
    });
}

/// Puts `inner`, just scrolled by a glide's landing step, on the glide's
/// `target` scroll top exactly, within the range the scroll clamps it to.
/// A bottom-aligned list at its end has no position to put.
fn land(inner: &mut crate::StateInner, target: Pixels, height: Pixels) {
    let Some(anchor) = inner.scroll_anchor else {
        return;
    };
    let padding = inner.last_padding.unwrap_or_default();
    let summary = inner.items.summary();
    let max = (summary.height + padding.top + padding.bottom - height).max(px(0.));
    let target = target.max(px(0.)).min(max);
    let count = summary.count;
    let item_top = |item_ix| {
        let offset_in_item = px(0.);
        inner.scroll_top(&ListOffset {
            item_ix,
            offset_in_item,
        })
    };
    // The step's sum lands a hair off the target, in the item it lands in
    // or the one beside it.
    let mut item_ix = anchor.position.item_ix.min(count);
    while item_ix > 0 && target < item_top(item_ix) {
        item_ix -= 1;
    }
    while item_ix + 1 < count && target >= item_top(item_ix + 1) {
        item_ix += 1;
    }
    let offset_in_item = target - item_top(item_ix);
    if let Some(anchor) = inner.scroll_anchor.as_mut() {
        anchor.position = ListOffset {
            item_ix,
            offset_in_item,
        };
        if let Some(crate::PendingScroll::Absolute(offset)) = anchor.pending.as_mut() {
            *offset = offset_in_item;
        }
    }
}
