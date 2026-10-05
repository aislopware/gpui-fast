//! A touch's fling ends once it moves nothing, and a touch outside what it
//! scrolls stays a tap.
//!
//! Upstream, a fling runs its whole curve whatever it scrolls: a list flung
//! against its end sits still for the second or two the curve has left, and
//! every touch in that time catches the fling, anywhere in the window, so it
//! can never be a tap. A tap on a text field beside the list goes nowhere.
//! On iOS and Android a fling stops at the end of its content, and a touch
//! outside the scrolling view is that view's business only.
//!
//! So the scroll containers a touch's scroll steps reach report back: `div`'s
//! scroll listener ([`scrolled_by`]) and `list`'s ([`scrolled_list`]) say
//! whether the step moved their content, and where they are on screen.
//!
//! - A step of momentum that some container heard and none moved ends the
//!   fling, with the momentum's closing step, as a touch catching it would.
//!   A step no container reported keeps the fling: a custom element's own
//!   scroll handler (a terminal's, say) is not one of the two, and how far it
//!   scrolled is its own.
//! - The containers a fling's last step reached are its container. A touch
//!   that starts outside them leaves the fling running and is a touch like
//!   any other, so it may be a tap ([`catch`]). Until a step has reported,
//!   any touch catches the fling, as upstream.
//!
//! The ticks that drive momentum (`Window::schedule_touch_momentum_tick`) are
//! kept to one chain ([`schedule_tick`]): upstream schedules a tick after any
//! touch while momentum runs, which a touch outside the fling now does.

use std::mem;

use smallvec::SmallVec;

use crate::{
    App, Bounds, Hitbox, Interactivity, ListOffset, Pixels, Point, ScrollWheelEvent, Style,
    TouchEvent, TouchPhase, Window,
    gestures::{Momentum, RecognizedTouchGesture, TouchGestureRecognizer, scroll_event},
    size,
};

/// What the fling in progress has learnt of what it scrolls.
#[derive(Default)]
pub(crate) struct Fling {
    /// Where the containers the fling's last step reached are on screen.
    container: Option<Bounds<Pixels>>,
    /// While a touch's scroll step is dispatched, what the containers it
    /// reached reported.
    heard: Option<Heard>,
    /// A momentum tick is scheduled.
    ticking: bool,
}

/// What the containers a scroll step reached reported.
#[derive(Default)]
struct Heard {
    /// Where they are on screen, together.
    containers: Option<Bounds<Pixels>>,
    /// Whether any of them moved its content.
    moved: bool,
}

/// A touch starts: the fling it catches, if it starts where the fling
/// scrolls or before the fling knows where that is.
pub(crate) fn catch(
    recognizer: &mut TouchGestureRecognizer,
    touch: &TouchEvent,
) -> Option<Momentum> {
    if recognizer
        .fast_fling
        .container
        .is_some_and(|container| !container.contains(&touch.position))
    {
        return None;
    }
    recognizer.fast_fling.container = None;
    recognizer.momentum.take()
}

/// A pan is released into a fling of its own: the one it leaves running,
/// started by a touch elsewhere, ends with its closing step.
pub(crate) fn replace(
    recognizer: &mut TouchGestureRecognizer,
    recognized: &mut SmallVec<[RecognizedTouchGesture; 2]>,
) {
    recognizer.fast_fling.container = None;
    if let Some(momentum) = recognizer.momentum.take() {
        recognized.push(RecognizedTouchGesture::Scroll(closing_step(&momentum)));
    }
}

/// Dispatches one step of a touch's scroll (the hook in
/// `Window::dispatch_recognized_touch_gesture`), and ends the fling when the
/// step was momentum that moved nothing.
pub(crate) fn dispatch_scroll(window: &mut Window, event: &ScrollWheelEvent, cx: &mut App) {
    let outer = window
        .touch_gestures
        .fast_fling
        .heard
        .replace(Heard::default());
    window.dispatch_mouse_event(event, cx);
    let heard =
        mem::replace(&mut window.touch_gestures.fast_fling.heard, outer).unwrap_or_default();
    let Some(phase) = event.momentum_phase else {
        return;
    };
    let fling = &mut window.touch_gestures.fast_fling;
    if phase == TouchPhase::Ended {
        fling.container = None;
        return;
    }
    let Some(containers) = heard.containers else {
        return;
    };
    fling.container = Some(containers);
    let delta = event.delta.pixel_delta(Pixels::ZERO);
    let stepped = delta.x != Pixels::ZERO || delta.y != Pixels::ZERO;
    if heard.moved || !stepped {
        return;
    }
    fling.container = None;
    if let Some(momentum) = window.touch_gestures.momentum.take() {
        window.dispatch_recognized_touch_gesture(
            RecognizedTouchGesture::Scroll(closing_step(&momentum)),
            cx,
        );
    }
}

/// Schedules the next momentum tick, unless one is (the body of
/// `Window::schedule_touch_momentum_tick`).
pub(crate) fn schedule_tick(window: &mut Window) {
    if mem::replace(&mut window.touch_gestures.fast_fling.ticking, true) {
        return;
    }
    window.on_next_frame(|window, cx| {
        window.touch_gestures.fast_fling.ticking = false;
        if let Some(gesture) = window.touch_gestures.tick_momentum() {
            window.dispatch_recognized_touch_gesture(gesture, cx);
        }
        if window.touch_gestures.has_momentum() {
            schedule_tick(window);
        }
    });
}

/// How far a `div` scrolls, as its prepaint clamps its offset
/// (`Interactivity::clamp_scroll_position`), taken where it paints its scroll
/// listener.
pub(crate) fn scroll_max(
    interactivity: &Interactivity,
    bounds: Bounds<Pixels>,
    style: &Style,
    window: &Window,
) -> Point<Pixels> {
    let padding = style
        .padding
        .to_pixels(bounds.size.into(), window.rem_size())
        .map(|edge| window.pixel_snap(*edge));
    let padded = interactivity.content_size
        + size(padding.left + padding.right, padding.top + padding.bottom);
    Point::from(padded - bounds.size)
        .map(|max| (max * 100.).round() / 100.)
        .max(&Point::default())
}

/// A `div`'s scroll listener moved its offset from `old` to `new`: whether its
/// content moved is whether the two differ once clamped as its prepaint will.
pub(crate) fn scrolled_by(
    window: &mut Window,
    hitbox: &Hitbox,
    max: Point<Pixels>,
    (old, new): (Point<Pixels>, Point<Pixels>),
) {
    let clamp = |offset: Point<Pixels>| {
        Point::new(
            offset.x.clamp(-max.x, Pixels::ZERO),
            offset.y.clamp(-max.y, Pixels::ZERO),
        )
    };
    heard(window, hitbox, clamp(old) != clamp(new));
}

/// A `list`'s scroll listener scrolled it from `old` to `new`, which the list
/// clamps as it scrolls.
pub(crate) fn scrolled_list(
    window: &mut Window,
    hitbox: &Hitbox,
    old: ListOffset,
    new: ListOffset,
) {
    heard(
        window,
        hitbox,
        old.item_ix != new.item_ix || old.offset_in_item != new.offset_in_item,
    );
}

fn heard(window: &mut Window, hitbox: &Hitbox, moved: bool) {
    let Some(heard) = window.touch_gestures.fast_fling.heard.as_mut() else {
        return;
    };
    let visible = hitbox.bounds.intersect(&hitbox.content_mask.bounds);
    heard.containers = Some(
        heard
            .containers
            .map_or(visible, |containers| containers.union(&visible)),
    );
    heard.moved |= moved;
}

fn closing_step(momentum: &Momentum) -> ScrollWheelEvent {
    scroll_event(momentum.position, Point::default(), TouchPhase::Ended)
        .fast_momentum(TouchPhase::Ended)
}
