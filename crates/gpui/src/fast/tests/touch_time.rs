//! Tests of a touch timed by when it happened (`fast/touch_time.rs`): a main
//! thread stalled between a finger's last move and its lift neither stops the
//! fling, nor does a lift handled at once fling a finger that had stopped.

use std::time::Duration;

use scheduler::Instant;

use crate::{
    self as gpui, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render,
    ScrollHandle, StatefulInteractiveElement, Styled, TestAppContext, TouchEvent, TouchId,
    TouchPhase, VisualTestContext, Window, div, point, px,
};

/// Twenty rows of 50 px scrolling in 100 px.
struct Scroller {
    handle: ScrollHandle,
}

impl Render for Scroller {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("scroller")
            .h(px(100.))
            .w(px(200.))
            .overflow_y_scroll()
            .track_scroll(&self.handle)
            .children((0..20).map(|row| div().id(row).h(px(50.)).w_full()))
    }
}

fn scroller(cx: &mut TestAppContext) -> (ScrollHandle, &mut VisualTestContext) {
    let handle = ScrollHandle::new();
    let held = handle.clone();
    let (_, cx) = cx.add_window_view(move |_, _| Scroller { handle: held });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (handle, cx)
}

/// The finger's touch in `phase` at `y`, which happened at `at`.
fn touch(cx: &mut VisualTestContext, phase: TouchPhase, y: f32, at: Instant) {
    cx.simulate_event(TouchEvent {
        id: TouchId(1),
        phase,
        position: point(px(100.), px(y)),
        predicted_position: None,
        force: None,
        timestamp: Some(at),
    });
}

/// A swipe up from y = 90 in four moves 16 ms apart, the first at `start`;
/// when its last move happened.
fn swipe(cx: &mut VisualTestContext, start: Instant) -> Instant {
    touch(cx, TouchPhase::Started, 90., start);
    let mut at = start;
    for step in 1..=4u8 {
        at = start + Duration::from_millis(16 * u64::from(step));
        touch(cx, TouchPhase::Moved, 90. - f32::from(step) * 15., at);
    }
    at
}

fn momentum_start(cx: &mut VisualTestContext) -> Option<Instant> {
    cx.update(|window, _| {
        window
            .touch_gestures
            .momentum
            .as_ref()
            .map(|momentum| momentum.started_at)
    })
}

fn offset(handle: &ScrollHandle) -> Pixels {
    handle.offset().y
}

#[gpui::test]
fn a_lift_handled_after_a_stall_still_flings(cx: &mut TestAppContext) {
    let (handle, cx) = scroller(cx);
    // The finger's moves happened 16 ms apart, a while ago, and its lift 16 ms
    // after the last; the main thread is busy for 100 ms between handling the
    // last move and handling the lift, two and a half times the gap after which
    // a lift reads as a stopped finger.
    let start = Instant::now() - Duration::from_millis(500);
    let last = swipe(cx, start);
    let dragged = offset(&handle);
    assert!(dragged < px(0.), "the swipe scrolls: {dragged:?}");
    std::thread::sleep(Duration::from_millis(100));
    let lifted_at = Instant::now();
    touch(cx, TouchPhase::Ended, 30., last + Duration::from_millis(16));
    let started = momentum_start(cx).expect("the lift flings");
    assert!(
        started >= lifted_at,
        "the fling runs from when its lift was handled, not from when the finger lifted"
    );
}

#[gpui::test]
fn a_finger_that_stopped_before_its_lift_does_not_fling(cx: &mut TestAppContext) {
    let (_, cx) = scroller(cx);
    // Handled at once, one after another, but the finger rested 100 ms before
    // it lifted.
    let start = Instant::now() - Duration::from_millis(500);
    let last = swipe(cx, start);
    touch(
        cx,
        TouchPhase::Ended,
        30.,
        last + Duration::from_millis(100),
    );
    assert_eq!(
        momentum_start(cx),
        None,
        "a finger that stopped does not fling"
    );
}

#[gpui::test]
fn a_touch_with_no_time_is_timed_as_it_is_handled(cx: &mut TestAppContext) {
    let (_, cx) = scroller(cx);
    let untimed = |cx: &mut VisualTestContext, phase, y| {
        cx.simulate_event(TouchEvent {
            id: TouchId(1),
            phase,
            position: point(px(100.), px(y)),
            ..TouchEvent::default()
        })
    };
    untimed(cx, TouchPhase::Started, 90.);
    for step in 1..=4u8 {
        untimed(cx, TouchPhase::Moved, 90. - f32::from(step) * 15.);
    }
    std::thread::sleep(Duration::from_millis(100));
    untimed(cx, TouchPhase::Ended, 30.);
    assert_eq!(
        momentum_start(cx),
        None,
        "a lift handled 100 ms after the last move, with no time of its own, reads as a stopped finger"
    );
}
