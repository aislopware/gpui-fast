//! Tests of a fling ending once it moves nothing, and of a touch outside what
//! it scrolls staying a tap.

use std::{cell::Cell, rc::Rc, time::Duration};

use crate::{
    self as gpui, Context, InteractiveElement, IntoElement, ListAlignment, ListOffset, ListState,
    ParentElement, Pixels, Point, Render, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement, Styled, TestAppContext, TouchEvent, TouchId, TouchPhase,
    VisualTestContext, Window, canvas, div, list, point, px,
};

/// How the scrolling part of [`Pane`] is drawn.
#[derive(Clone, Copy)]
enum Scroller {
    /// A `div` scrolling twenty rows of 50 px in its 100 px.
    Div,
    /// A `list` of the same rows.
    List,
    /// An element scrolling by a handler of its own, which reports nothing.
    Custom,
}

/// A scroller 100 px tall at the top, and a button below it.
struct Pane {
    scroller: Scroller,
    handle: ScrollHandle,
    list: ListState,
    /// The button's clicks.
    clicks: Rc<Cell<usize>>,
    /// The momentum steps that reached the pane, by their momentum phase.
    momentum: Rc<Cell<[usize; 2]>>,
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = || (0..20).map(|row| div().id(row).h(px(50.)).w_full());
        let scroller = match self.scroller {
            Scroller::Div => div()
                .id("scroller")
                .h(px(100.))
                .w(px(200.))
                .overflow_y_scroll()
                .track_scroll(&self.handle)
                .children(rows())
                .into_any_element(),
            Scroller::List => list(self.list.clone(), |row, _, _| {
                div().id(row).h(px(50.)).w_full().into_any_element()
            })
            .h(px(100.))
            .w(px(200.))
            .into_any_element(),
            Scroller::Custom => canvas(
                |_, _, _| {},
                |_, _, window, _| {
                    window.on_mouse_event(|_: &ScrollWheelEvent, _, _, cx| {
                        cx.stop_propagation();
                    });
                },
            )
            .h(px(100.))
            .w(px(200.))
            .into_any_element(),
        };
        let clicks = self.clicks.clone();
        let momentum = self.momentum.clone();
        div()
            .id("pane")
            .size_full()
            .on_scroll_wheel(move |event, _, _| {
                let mut seen = momentum.get();
                match event.momentum_phase {
                    Some(TouchPhase::Ended) => seen[1] += 1,
                    Some(_) => seen[0] += 1,
                    None => {}
                }
                momentum.set(seen);
            })
            .child(scroller)
            .child(
                div()
                    .id("button")
                    .h(px(50.))
                    .w(px(200.))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
    }
}

struct Harness<'a> {
    cx: &'a mut VisualTestContext,
    pane: gpui::Entity<Pane>,
}

impl Harness<'_> {
    fn touch(&mut self, id: u64, phase: TouchPhase, at: Point<Pixels>) {
        self.cx.simulate_event(TouchEvent {
            id: TouchId(id),
            phase,
            position: at,
            predicted_position: None,
            force: None,
        });
    }

    /// Swipes up through the scroller from y = 90 and lets go at speed.
    fn fling(&mut self) {
        for step in 0..=4u8 {
            let phase = match step {
                0 => TouchPhase::Started,
                4 => TouchPhase::Ended,
                _ => TouchPhase::Moved,
            };
            self.touch(1, phase, point(px(100.), px(90. - f32::from(step) * 20.)));
            self.cx.executor().advance_clock(Duration::from_millis(16));
        }
    }

    /// Lets a frame go by, running the momentum tick it brings.
    fn frame(&mut self) {
        self.cx.executor().advance_clock(Duration::from_millis(34));
        let window = self.cx.update(|window, _| window.window_handle());
        self.cx.test_window(window).simulate_scheduled_frame();
        self.cx.run_until_parked();
    }

    fn has_momentum(&mut self) -> bool {
        self.cx
            .update(|window, _| window.touch_gestures.has_momentum())
    }

    fn momentum(&mut self) -> [usize; 2] {
        self.pane.read_with(self.cx, |pane, _| pane.momentum.get())
    }

    fn clicks(&mut self) -> usize {
        self.pane.read_with(self.cx, |pane, _| pane.clicks.get())
    }

    fn div_offset(&mut self) -> Pixels {
        self.pane
            .read_with(self.cx, |pane, _| pane.handle.offset().y)
    }

    fn list_top(&mut self) -> ListOffset {
        self.pane
            .read_with(self.cx, |pane, _| pane.list.logical_scroll_top())
    }
}

fn harness(cx: &mut TestAppContext, scroller: Scroller) -> Harness<'_> {
    let (pane, cx) = cx.add_window_view(|_, _| Pane {
        scroller,
        handle: ScrollHandle::new(),
        list: ListState::new(20, ListAlignment::Top, px(100.)),
        clicks: Rc::default(),
        momentum: Rc::default(),
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    Harness { cx, pane }
}

#[gpui::test]
fn a_fling_into_a_divs_end_stops_there(cx: &mut TestAppContext) {
    let mut harness = harness(cx, Scroller::Div);
    harness.pane.update(harness.cx, |pane, _| {
        pane.handle.set_offset(point(px(0.), px(-900.)))
    });
    harness.cx.run_until_parked();
    harness.frame();
    assert_eq!(harness.div_offset(), px(-900.));

    harness.fling();
    assert!(harness.has_momentum(), "the swipe flings");
    harness.frame();
    assert!(!harness.has_momentum(), "a step that moves nothing ends it");
    assert_eq!(harness.momentum(), [1, 1], "and the fling closes");
    assert_eq!(harness.div_offset(), px(-900.));
}

#[gpui::test]
fn a_fling_runs_on_while_it_moves_a_div(cx: &mut TestAppContext) {
    let mut harness = harness(cx, Scroller::Div);
    harness.fling();
    let mut offsets = vec![harness.div_offset()];
    for _ in 0..3 {
        harness.frame();
        assert!(harness.has_momentum());
        offsets.push(harness.div_offset());
    }
    assert!(
        offsets.windows(2).all(|pair| pair[1] < pair[0]),
        "{offsets:?}"
    );
    assert_eq!(harness.momentum()[1], 0);
}

#[gpui::test]
fn a_fling_into_a_lists_end_stops_there(cx: &mut TestAppContext) {
    let mut harness = harness(cx, Scroller::List);
    let end = ListOffset {
        item_ix: 18,
        offset_in_item: px(0.),
    };
    harness
        .pane
        .update(harness.cx, |pane, _| pane.list.scroll_to(end));
    harness.frame();
    assert_eq!(harness.list_top().item_ix, 18);

    harness.fling();
    assert!(harness.has_momentum());
    harness.frame();
    assert!(!harness.has_momentum());
    assert_eq!(harness.momentum(), [1, 1]);
    assert_eq!(harness.list_top().item_ix, 18);
}

#[gpui::test]
fn a_fling_runs_on_while_it_moves_a_list(cx: &mut TestAppContext) {
    let mut harness = harness(cx, Scroller::List);
    harness.fling();
    let before = harness.list_top();
    harness.frame();
    let after = harness.list_top();
    assert!(harness.has_momentum());
    assert!(
        (after.item_ix, after.offset_in_item) > (before.item_ix, before.offset_in_item),
        "{before:?} {after:?}"
    );
}

/// A handler of its own reports nothing, so how far it scrolled is its own
/// business, and the fling runs its course.
#[gpui::test]
fn a_fling_a_custom_handler_takes_runs_its_course(cx: &mut TestAppContext) {
    let mut harness = harness(cx, Scroller::Custom);
    harness.fling();
    for _ in 0..3 {
        harness.frame();
        assert!(harness.has_momentum());
    }
}

#[gpui::test]
fn a_tap_beside_a_fling_is_a_tap(cx: &mut TestAppContext) {
    let mut harness = harness(cx, Scroller::Div);
    harness.fling();
    harness.frame();
    assert!(harness.has_momentum());

    let button = point(px(100.), px(125.));
    harness.touch(2, TouchPhase::Started, button);
    harness
        .cx
        .executor()
        .advance_clock(Duration::from_millis(50));
    harness.touch(2, TouchPhase::Ended, button);
    assert_eq!(harness.clicks(), 1, "the tap is a click");

    // The fling, not caught, carries on.
    assert!(harness.has_momentum());
    let before = harness.div_offset();
    harness.frame();
    assert!(harness.div_offset() < before);
}

#[gpui::test]
fn a_touch_on_a_fling_catches_it(cx: &mut TestAppContext) {
    let mut harness = harness(cx, Scroller::Div);
    harness.fling();
    harness.frame();
    assert!(harness.has_momentum());

    let scroller = point(px(100.), px(50.));
    harness.touch(2, TouchPhase::Started, scroller);
    assert!(!harness.has_momentum(), "the touch stops the fling");
    harness.touch(2, TouchPhase::Ended, scroller);
    assert_eq!(harness.momentum()[1], 1);
}

/// A touch beside a fling that flings in turn ends the first fling, which
/// closes, before the second one starts.
#[gpui::test]
fn a_second_fling_closes_the_first(cx: &mut TestAppContext) {
    let mut harness = harness(cx, Scroller::Div);
    harness.fling();
    harness.frame();
    for step in 0..=4u8 {
        let phase = match step {
            0 => TouchPhase::Started,
            4 => TouchPhase::Ended,
            _ => TouchPhase::Moved,
        };
        harness.touch(2, phase, point(px(100.), px(140. - f32::from(step) * 20.)));
        harness
            .cx
            .executor()
            .advance_clock(Duration::from_millis(16));
    }
    assert!(harness.has_momentum());
    assert_eq!(harness.momentum()[1], 1, "the first fling closed");
    assert_eq!(harness.clicks(), 0);
}
