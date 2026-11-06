//! Tests of a mouse wheel's notches gliding through a `list`
//! (`fast::wheel_glide`, monocode's `WheelGlide`), with scroll layers on and
//! off. Each frame's position is checked against monocode's step, written
//! out here with its constants rather than read from the module.

use std::time::Duration;

use crate::fast::layers::policy::{Decision, last_decision};
use crate::{
    self as gpui, Context, IntoElement, ListAlignment, ListState, ParentElement as _, Render,
    ScrollDelta, ScrollEvent, Styled as _, TestAppContext, VisualTestContext, Window, div, list,
    point, px, rgb,
};

/// How tall a row is, in pixels.
const ROW: f32 = 20.;
/// How tall the list is, in pixels.
const VIEWPORT: f32 = 100.;
/// How many rows the list holds: 900 px of scrolling.
const ROWS: usize = 50;
/// How long a frame takes, in seconds.
const FRAME: f32 = 0.016;

/// monocode's glide step (its `wheel_glide_frame`): how far a frame `dt`
/// seconds after the last moves a glide with `remaining` still to go.
fn monocode_step(remaining: f32, dt: f32) -> f32 {
    let dt = dt.min(0.05);
    let step = remaining * (1. - (-dt / 0.07).exp());
    if (remaining - step).abs() < 0.5 {
        remaining
    } else {
        step
    }
}

struct Rows {
    state: ListState,
}

impl Render for Rows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0xffffff)).child(
            list(self.state.clone(), |row, _, _| {
                div()
                    .w(px(200.))
                    .h(px(ROW))
                    .bg(crate::hsla(row as f32 / ROWS as f32, 0.5, 0.5, 1.))
                    .into_any_element()
            })
            .w(px(200.))
            .h(px(VIEWPORT)),
        )
    }
}

struct Harness<'a> {
    cx: &'a mut VisualTestContext,
    state: ListState,
    layers: bool,
    /// The frames whose list layer composited, rather than painted afresh.
    composited: usize,
}

fn harness(cx: &mut TestAppContext, alignment: ListAlignment, layers: bool) -> Harness<'_> {
    // Know the full scroll range before asserting an exact pixel distance.
    let state = ListState::new(ROWS, alignment, px(0.)).measure_all();
    let view_state = state.clone();
    let (_, cx) = cx.add_window_view(move |_, _| Rows { state: view_state });
    cx.update(|window, _| {
        window.activate_window();
        window.set_scroll_layers(layers);
    });
    cx.run_until_parked();
    let mut harness = Harness {
        cx,
        state,
        layers,
        composited: 0,
    };
    harness.frame();
    harness
}

impl Harness<'_> {
    /// Turns the wheel by `lines` notch lines, down for a negative count.
    fn notch(&mut self, lines: f32) {
        self.cx.simulate_event(ScrollEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Lines(point(0., lines)),
            ..Default::default()
        });
    }

    /// A trackpad's scroll by `dy` pixels.
    fn pixels(&mut self, dy: f32) {
        self.cx.simulate_event(ScrollEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            ..Default::default()
        });
    }

    /// Lets `seconds` go by and the frame after them, running the glide's
    /// step before it draws.
    fn frame_after(&mut self, seconds: f32) {
        self.cx
            .executor()
            .advance_clock(Duration::from_secs_f32(seconds));
        let window = self.cx.update(|window, _| window.window_handle());
        self.cx.test_window(window).simulate_scheduled_frame();
        self.cx.run_until_parked();
        if self.layers && crate::fast::layers::COMPILED {
            let composited = self.cx.update(|window, _| {
                window
                    .fast_layers
                    .layers
                    .keys()
                    .any(|id| last_decision(window, id) == Some(Decision::Composite))
            });
            self.composited += usize::from(composited);
        }
    }

    fn frame(&mut self) {
        self.frame_after(FRAME);
    }

    /// How far the list is scrolled, in pixels: a bottom-aligned list with
    /// no position of its own is at its end.
    fn top(&self) -> f32 {
        let inner = self.state.0.borrow();
        match inner.scroll_anchor {
            Some(anchor) => f32::from(inner.scroll_top(&anchor.position)),
            None if inner.alignment == ListAlignment::Bottom => ROWS as f32 * ROW - VIEWPORT,
            None => 0.,
        }
    }

    fn gliding(&self) -> bool {
        self.state.0.borrow().fast_glide.is_some()
    }

    /// Runs frames until the glide ends, checking each against monocode's
    /// step from `remaining` (in scroll direction, so positive scrolls down);
    /// returns how many it took.
    fn glide_out(&mut self, mut remaining: f32) -> usize {
        let mut frames = 0;
        while remaining != 0. {
            let before = self.top();
            self.frame();
            let step = monocode_step(remaining, FRAME);
            remaining -= step;
            frames += 1;
            let moved = self.top() - before;
            assert!(
                (moved - step).abs() < 1e-3,
                "frame {frames}: moved {moved}, monocode steps {step}"
            );
            assert!(frames < 100, "the glide ends");
        }
        assert!(!self.gliding(), "the glide is over once it lands");
        frames
    }
}

/// Runs `test` with scroll layers off, then on.
fn both_ways(cx: &mut TestAppContext, test: impl Fn(&mut TestAppContext, bool)) {
    test(cx, false);
    test(cx, true);
}

/// onicode's `wheel_notches_glide_and_trackpad_pixels_land_at_once`.
#[gpui::test]
fn a_notch_glides_where_trackpad_pixels_land_at_once(cx: &mut TestAppContext) {
    both_ways(cx, |cx, layers| {
        let mut harness = harness(cx, ListAlignment::Top, layers);
        harness.notch(-3.);
        // The notch is distance still to travel, not a jump already made.
        assert_eq!(harness.top(), 0.);
        assert!(harness.gliding());
        harness.pixels(-40.);
        assert_eq!(harness.top(), 40.);
        assert!(!harness.gliding(), "trackpad pixels end the glide");
    });
}

#[gpui::test]
fn a_notch_lands_its_whole_distance_on_monocodes_curve(cx: &mut TestAppContext) {
    both_ways(cx, |cx, layers| {
        let mut harness = harness(cx, ListAlignment::Top, layers);
        harness.notch(-10.);
        let frames = harness.glide_out(200.);
        assert_eq!(harness.top(), 200.);
        // About 95% within 3τ: 13 frames of 16 ms; the last half pixel by 33.
        assert!((20..=40).contains(&frames), "{frames} frames");
        if layers && crate::fast::layers::COMPILED {
            assert!(
                harness.composited > frames / 2,
                "the glide's frames composite the list's layer ({} of {frames})",
                harness.composited
            );
        }
    });
}

#[gpui::test]
fn notches_add_to_the_distance_still_to_go(cx: &mut TestAppContext) {
    both_ways(cx, |cx, layers| {
        let mut harness = harness(cx, ListAlignment::Top, layers);
        harness.notch(-3.);
        let mut remaining = 60.;
        for _ in 0..2 {
            harness.frame();
            remaining -= monocode_step(remaining, FRAME);
        }
        harness.notch(-2.);
        harness.glide_out(remaining + 40.);
        assert_eq!(harness.top(), 100.);
    });
}

#[gpui::test]
fn a_notch_the_other_way_drops_what_was_left(cx: &mut TestAppContext) {
    both_ways(cx, |cx, layers| {
        let mut harness = harness(cx, ListAlignment::Top, layers);
        harness.notch(-5.);
        let mut remaining = 100.;
        for _ in 0..3 {
            harness.frame();
            remaining -= monocode_step(remaining, FRAME);
        }
        let travelled = harness.top();
        assert!((travelled - (100. - remaining)).abs() < 1e-3);
        harness.notch(1.);
        harness.glide_out(-20.);
        assert!((harness.top() - (travelled - 20.)).abs() < 1e-3);
    });
}

/// A frame that comes late steps the glide as one 50 ms after the last.
#[gpui::test]
fn a_late_frame_steps_at_most_fifty_milliseconds(cx: &mut TestAppContext) {
    let mut harness = harness(cx, ListAlignment::Top, false);
    harness.notch(-10.);
    harness.frame_after(0.5);
    let step = monocode_step(200., 0.05);
    assert!((harness.top() - step).abs() < 1e-3, "{}", harness.top());
    assert!(harness.gliding());
}

#[gpui::test]
fn with_reduced_motion_a_notch_jumps(cx: &mut TestAppContext) {
    let mut harness = harness(cx, ListAlignment::Top, false);
    harness.cx.update(|_, cx| cx.set_reduce_motion(true));
    harness.frame();
    harness.notch(-3.);
    assert_eq!(harness.top(), 60.);
    assert!(!harness.gliding());
}

/// A horizontal notch is no glide: the list scrolls as upstream's would.
#[gpui::test]
fn a_sideways_notch_does_not_glide(cx: &mut TestAppContext) {
    let mut harness = harness(cx, ListAlignment::Top, false);
    harness.cx.simulate_event(ScrollEvent {
        position: point(px(10.), px(10.)),
        delta: ScrollDelta::Lines(point(1., -1.)),
        ..Default::default()
    });
    assert!(!harness.gliding());
    assert_eq!(harness.top(), 20.);
}

/// A bottom-aligned list at its end has no logical position, and a notch
/// further down ends the glide without scrolling.
#[gpui::test]
fn a_bottom_aligned_list_at_its_end_ends_the_glide(cx: &mut TestAppContext) {
    both_ways(cx, |cx, layers| {
        let mut harness = harness(cx, ListAlignment::Bottom, layers);
        let end = harness.top();
        harness.notch(-3.);
        assert!(harness.gliding());
        harness.frame();
        assert!(!harness.gliding());
        assert_eq!(harness.top(), end);

        // Up from the end glides as anywhere else.
        harness.notch(2.);
        harness.glide_out(-40.);
        assert!((harness.top() - (end - 40.)).abs() < 1e-3);
    });
}

/// What moves the list to a place of its own ends the glide, as monocode's
/// `reset`, `scroll_to_end` and `set_follow_mode(Tail)` do.
#[gpui::test]
fn scrolling_to_the_end_or_resetting_ends_the_glide(cx: &mut TestAppContext) {
    let mut harness = harness(cx, ListAlignment::Top, false);
    harness.notch(-3.);
    harness.state.scroll_to_end();
    assert!(!harness.gliding());

    harness.frame();
    harness.notch(3.);
    harness.state.set_follow_mode(crate::FollowMode::Tail);
    assert!(!harness.gliding());

    harness.frame();
    harness.notch(3.);
    harness.state.reset(ROWS);
    assert!(!harness.gliding());
}
