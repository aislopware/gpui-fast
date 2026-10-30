//! Tests of whether a touch is in flight (`fast/touch_in_flight.rs`): from a
//! finger's landing to its lift, and on until the fling it threw stops; and a
//! view drawn from its last frame that asks is built again as that changes,
//! and only then.

use std::{cell::RefCell, rc::Rc, time::Duration};

use scheduler::Instant;

use crate::{
    self as gpui, AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, StyleRefinement, Styled, TestAppContext, TouchEvent,
    TouchId, TouchPhase, VisualTestContext, Window, div, point, px,
};

/// Says what it read of the touch each time it is built.
struct Reader {
    seen: Rc<RefCell<Vec<bool>>>,
}

impl Render for Reader {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.seen.borrow_mut().push(window.touch_in_flight());
        div().size_full()
    }
}

/// Twenty rows of 50 px scrolling in 100 px at the top, and the reader,
/// cached, below.
struct Pane {
    reader: Entity<Reader>,
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                div()
                    .id("scroller")
                    .h(px(100.))
                    .w(px(200.))
                    .overflow_y_scroll()
                    .children((0..20).map(|row| div().id(row).h(px(50.)).w_full())),
            )
            .child(
                self.reader
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
    }
}

struct Harness<'a> {
    cx: &'a mut VisualTestContext,
    seen: Rc<RefCell<Vec<bool>>>,
}

/// The pane in a window, drawn and settled, with what its reader saw so far
/// taken.
fn harness(cx: &mut TestAppContext) -> Harness<'_> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let reader = cx.new({
        let seen = seen.clone();
        |_| Reader { seen }
    });
    let (_, cx) = cx.add_window_view(|_, _| Pane { reader });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let mut harness = Harness { cx, seen };
    harness.frame();
    assert_eq!(harness.seen().last(), Some(&false), "drawn with no finger");
    harness
}

impl Harness<'_> {
    /// The finger's touch in `phase` at `y`, which happened at `at`.
    fn touch(&mut self, phase: TouchPhase, y: f32, at: Instant) {
        self.cx.simulate_event(TouchEvent {
            id: TouchId(1),
            phase,
            position: point(px(100.), px(y)),
            predicted_position: None,
            force: None,
            timestamp: Some(at),
        });
    }

    /// A finger that landed at y = 90 at `start` and moved up 20 px each
    /// 16 ms, three times; when it last moved, at y = 30.
    fn swipe(&mut self, start: Instant) -> Instant {
        self.touch(TouchPhase::Started, 90., start);
        let mut at = start;
        for step in 1..=3u8 {
            at = start + Duration::from_millis(16 * u64::from(step));
            self.touch(TouchPhase::Moved, 90. - f32::from(step) * 20., at);
        }
        at
    }

    /// A swipe up that a finger let go of at speed, a while ago.
    fn fling(&mut self) {
        let last = self.swipe(Instant::now() - Duration::from_millis(200));
        self.touch(TouchPhase::Ended, 10., last + Duration::from_millis(16));
    }

    /// Lets a frame go by, running what it brings: a fling's step, which goes
    /// by the clock, and the draw of whatever changed.
    fn frame(&mut self) {
        std::thread::sleep(Duration::from_millis(16));
        self.cx.executor().advance_clock(Duration::from_millis(16));
        let window = self.cx.update(|window, _| window.window_handle());
        self.cx.test_window(window).simulate_scheduled_frame();
        self.cx.run_until_parked();
    }

    /// Frames until the fling stops, each seeing the touch in flight before
    /// it; how many.
    fn coast(&mut self) -> usize {
        let mut frames = 0;
        while self.flinging() {
            assert!(
                self.in_flight(),
                "in flight while it coasts, frame {frames}"
            );
            self.frame();
            frames += 1;
            assert!(frames < 300, "the fling stops");
        }
        frames
    }

    fn in_flight(&mut self) -> bool {
        self.cx.update(|window, _| window.touch_in_flight())
    }

    fn flinging(&mut self) -> bool {
        self.cx
            .update(|window, _| window.touch_gestures.has_momentum())
    }

    /// What the reader saw each time it was built, from the last call on.
    fn seen(&mut self) -> Vec<bool> {
        std::mem::take(&mut *self.seen.borrow_mut())
    }
}

#[gpui::test]
fn a_tap_is_in_flight_until_it_lifts(cx: &mut TestAppContext) {
    let mut harness = harness(cx);
    assert!(!harness.in_flight(), "no finger yet");
    let landed = Instant::now() - Duration::from_millis(100);
    harness.touch(TouchPhase::Started, 50., landed);
    assert!(harness.in_flight(), "a finger that landed");
    harness.touch(TouchPhase::Ended, 50., landed + Duration::from_millis(50));
    assert!(!harness.in_flight(), "a tap that lifted");
    assert!(!harness.flinging());
}

#[gpui::test]
fn a_scroll_that_stopped_before_its_lift_is_over_at_its_lift(cx: &mut TestAppContext) {
    let mut harness = harness(cx);
    let last = harness.swipe(Instant::now() - Duration::from_millis(500));
    assert!(harness.in_flight(), "a finger that scrolls");
    // The finger rested 200 ms, well past the gap after which it reads as
    // stopped, before it lifted.
    harness.touch(TouchPhase::Ended, 30., last + Duration::from_millis(200));
    assert!(!harness.flinging(), "a finger that stopped throws no fling");
    assert!(!harness.in_flight(), "and its scroll is over as it lifts");
}

#[gpui::test]
fn a_fling_keeps_the_touch_in_flight_until_it_stops(cx: &mut TestAppContext) {
    let mut harness = harness(cx);
    harness.fling();
    assert!(harness.flinging(), "the swipe flings");
    let frames = harness.coast();
    assert!(frames > 1, "it coasted for a while: {frames} frames");
    assert!(!harness.in_flight(), "over once it stopped");
}

#[gpui::test]
fn a_view_that_asks_is_built_again_as_the_touch_lands_and_as_its_fling_stops(
    cx: &mut TestAppContext,
) {
    let mut harness = harness(cx);
    harness.frame();
    assert!(
        harness.seen().is_empty(),
        "not built again while nothing changes"
    );
    let start = Instant::now() - Duration::from_millis(200);
    harness.touch(TouchPhase::Started, 90., start);
    harness.frame();
    assert_eq!(harness.seen(), [true], "built again as the finger lands");
    let mut at = start;
    for step in 1..=3u8 {
        at = start + Duration::from_millis(16 * u64::from(step));
        harness.touch(TouchPhase::Moved, 90. - f32::from(step) * 20., at);
        harness.frame();
    }
    harness.touch(TouchPhase::Ended, 10., at + Duration::from_millis(16));
    assert!(harness.flinging(), "the swipe flings");
    harness.coast();
    harness.frame();
    assert_eq!(
        harness.seen(),
        [false],
        "not built while the finger scrolls and its fling coasts, and built again once it stops"
    );
}
