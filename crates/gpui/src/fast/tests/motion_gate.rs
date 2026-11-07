//! The motion gate ([`crate::fast::motion_gate`]) finds each artefact it is
//! for when one is injected, and nothing in the same scene without it:
//!
//! - a panel that drops out for one frame while nothing moves: `flicker`;
//! - a slide whose easing jumps halfway: `pop`;
//! - a block dragged on every frame, one move's notify lost: `stale`.
//!
//! The fade in `animation_end` is gated the same way.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use crate::motion_gate::{Finding, MotionRun, TrackKind};
use crate::{
    Animation, AnimationExt as _, AppContext as _, Context, Entity, IntoElement,
    ParentElement as _, Render, Styled as _, TestAppContext, Window, div, hsla, px,
};

/// A panel over a background, hidden while `hidden`, and a block `x` points
/// from the left.
struct Stage {
    hidden: Rc<Cell<bool>>,
    x: Rc<Cell<f32>>,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(hsla(0., 0., 0.1, 1.))
            .child(
                div()
                    .absolute()
                    .top(px(20.))
                    .left(px(20.))
                    .w(px(120.))
                    .h(px(60.))
                    .bg(hsla(
                        0.6,
                        0.6,
                        if self.hidden.get() { 0.1 } else { 0.5 },
                        1.,
                    )),
            )
            .child(
                div()
                    .absolute()
                    .top(px(120.))
                    .left(px(self.x.get()))
                    .w(px(40.))
                    .h(px(40.))
                    .bg(hsla(0.1, 0.8, 0.6, 1.)),
            )
    }
}

fn stage(cx: &mut TestAppContext) -> (MotionRun, Entity<Stage>, Rc<Cell<bool>>, Rc<Cell<f32>>) {
    let hidden = Rc::new(Cell::new(false));
    let x = Rc::new(Cell::new(10.));
    let view = cx.new({
        let (hidden, x) = (hidden.clone(), x.clone());
        |_| Stage { hidden, x }
    });
    let window = cx.add_window({
        let view = view.clone();
        move |_, _| Root { view }
    });
    let run = MotionRun::new(cx, window, 60.);
    (run, view, hidden, x)
}

struct Root {
    view: Entity<Stage>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(320.)).child(self.view.clone())
    }
}

fn blink(glitch: bool) -> Vec<Finding> {
    let mut cx = TestAppContext::single();
    let (mut run, view, hidden, _) = stage(&mut cx);
    run.play(&mut cx, 0.2);
    // Something redraws the scene twice, with no input and nothing animating.
    for drop_out in [glitch, false] {
        hidden.set(drop_out);
        view.update(&mut cx, |_, cx| cx.notify());
        run.step(&mut cx);
    }
    run.play(&mut cx, 0.3);
    run.findings()
}

#[test]
fn a_panel_that_drops_out_for_a_frame_flickers() {
    let found = blink(true);
    assert!(
        found.iter().any(|finding| finding.kind == "flicker"),
        "{found:#?}"
    );
    assert!(blink(false).is_empty(), "{:#?}", blink(false));
}

/// A slide of 200 points over 0.3 s, eased; `glitch` jumps it 60 points
/// halfway.
struct Slide {
    glitch: bool,
    started: bool,
}

impl Render for Slide {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let glitch = self.glitch;
        let block = div()
            .absolute()
            .top(px(40.))
            .w(px(40.))
            .h(px(40.))
            .bg(hsla(0.1, 0.8, 0.6, 1.));
        div()
            .size(px(320.))
            .bg(hsla(0., 0., 0.1, 1.))
            .child(if self.started {
                block
                    .with_animation(
                        "slide",
                        Animation::new(Duration::from_millis(300)),
                        move |el, t| {
                            let eased = t * t * (3. - 2. * t);
                            let jump = if glitch && t >= 0.5 { 60. } else { 0. };
                            el.left(px(10. + 200. * eased + jump))
                        },
                    )
                    .into_any_element()
            } else {
                block.left(px(10.)).into_any_element()
            })
    }
}

fn slide(glitch: bool) -> Vec<Finding> {
    let mut cx = TestAppContext::single();
    let view = cx.new(|_| Slide {
        glitch,
        started: false,
    });
    let window = cx.add_window({
        let view = view.clone();
        move |_, _| SlideRoot { view }
    });
    let mut run = MotionRun::new(&mut cx, window, 60.);
    run.track("block x", TrackKind::Position, |window| {
        let scale = window.scale_factor();
        window
            .rendered_frame
            .scene
            .quads
            .iter()
            .find(|quad| (quad.bounds.size.width.0 / scale - 40.).abs() < 0.5)
            .map(|quad| quad.bounds.origin.x.0 / scale)
    });
    run.play(&mut cx, 0.1);
    run.act(&mut cx, "start the slide", |cx| {
        view.update(cx, |slide, cx| {
            slide.started = true;
            cx.notify();
        });
    });
    run.play(&mut cx, 0.5);
    run.findings()
}

struct SlideRoot {
    view: Entity<Slide>,
}

impl Render for SlideRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(320.)).child(self.view.clone())
    }
}

#[test]
fn a_slide_whose_easing_jumps_pops() {
    let found = slide(true);
    assert!(
        found
            .iter()
            .any(|finding| finding.kind == "pop" && finding.place == "block x"),
        "{found:#?}"
    );
    let clean = slide(false);
    assert!(clean.is_empty(), "{clean:#?}");
}

fn walk(lost: Option<usize>) -> Vec<Finding> {
    let mut cx = TestAppContext::single();
    let (mut run, view, _, x) = stage(&mut cx);
    let model = x.clone();
    run.track("block x", TrackKind::Position, move |_| Some(model.get()));
    run.play(&mut cx, 0.1);
    // A drag moves it: each move is an input, so its edges may step at once.
    for frame in 0..30 {
        run.act(&mut cx, "drag", |cx| {
            x.set(10. + 5. * (frame + 1) as f32);
            // The notify that frame's move should have sent is lost.
            if lost != Some(frame) {
                view.update(cx, |_, cx| cx.notify());
            }
        });
    }
    run.play(&mut cx, 0.3);
    run.findings()
}

#[test]
fn a_frame_whose_update_is_lost_is_stale() {
    let found = walk(Some(12));
    assert!(
        found.iter().any(|finding| finding.kind == "stale"),
        "{found:#?}"
    );
    let clean = walk(None);
    assert!(clean.is_empty(), "{clean:#?}");
}
