//! A touch is timed by when it happened, not by when it is handled.
//!
//! The recognizer reads a finger's speed off the times of its samples, and
//! takes a release that comes more than `VELOCITY_ASSUME_STOPPED_GAP` (40 ms)
//! after the last move for a finger that had stopped, which never flings.
//! Timed as handled, a main thread busy when the lift arrives (a page loading,
//! a frame drawn late) has every lift come late: the fling a person threw dies
//! on the spot, and moves that waited and then came all at once read as a
//! finger far faster than it was. With the platform's own time of each event
//! (`TouchEvent::timestamp`: `UITouch.timestamp` on iOS) the samples stand as
//! far apart as the finger's moves did, whenever they are handled.
//!
//! A fling the release starts runs from when it is handled, not from when the
//! finger lifted: the content stood still while the thread was busy, so its
//! curve starts where the content stands rather than jumping ahead by the
//! wait. A platform that says no time leaves the event timed as it is handled.

use smallvec::SmallVec;

use scheduler::Instant;

use crate::{
    TouchEvent, TouchPhase,
    gestures::{RecognizedTouchGesture, TouchGestureRecognizer},
};

/// `event` through `recognizer`, timed by when it happened (the body of
/// `TouchGestureRecognizer::handle_event`).
pub(crate) fn handle(
    recognizer: &mut TouchGestureRecognizer,
    event: &TouchEvent,
) -> SmallVec<[RecognizedTouchGesture; 2]> {
    let now = Instant::now();
    let at = happened(event, now);
    let recognized = recognizer.handle_event_at(event, at);
    // The release that flings starts its momentum at `at`; nothing else does.
    if event.phase == TouchPhase::Ended
        && let Some(momentum) = recognizer.momentum.as_mut()
        && momentum.started_at == at
    {
        momentum.started_at = now;
    }
    recognized
}

/// When `event` happened: its timestamp, or `now` when it has none. A
/// timestamp after `now` (the platform's clock read against ours a little off)
/// is `now`.
pub(crate) fn happened(event: &TouchEvent, now: Instant) -> Instant {
    event.timestamp.map_or(now, |timestamp| timestamp.min(now))
}
