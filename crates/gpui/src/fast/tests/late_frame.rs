//! Motion a late frame starts begins from when that frame showed
//! ([`crate::fast::frame_time::shown`]).
//!
//! A frame that takes 50 ms to draw the fade and the spring it starts shows
//! them 50 ms after its moment. The frame after moves them as far as it would
//! have after a prompt frame, while a fade already running keeps real time.

use std::{cell::RefCell, rc::Rc, time::Duration};

use crate::{
    Animation, AnimationExt as _, AppContext as _, Context, IntoElement, ParentElement as _,
    Render, SpringAnimation, SpringConfig, Styled as _, TestAppContext, Window, div,
    prelude::FluentBuilder as _, px,
};

const SPAN: Duration = Duration::from_millis(200);
const FRAME: Duration = Duration::from_millis(8);
const OVERRUN: Duration = Duration::from_millis(50);

type Values = Rc<RefCell<Vec<f32>>>;

/// A fade that runs from the first frame, and a fade and a spring that a
/// later frame starts.
#[derive(Default)]
struct Motion {
    started: bool,
    /// How long the next frame's own work takes, on the test's clock.
    slow: Option<Duration>,
    running: Values,
    fade: Values,
    spring: Values,
}

impl Render for Motion {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(slow) = self.slow.take() {
            cx.background_executor().advance_clock(slow);
        }
        let record = |values: &Values| {
            let values = values.clone();
            move |element, t: f32| {
                values.borrow_mut().push(t);
                element
            }
        };
        let spring = self.spring.clone();
        div()
            .child(div().with_animation("running", Animation::new(SPAN), record(&self.running)))
            .when(self.started, |root| {
                root.child(div().with_animation("fade", Animation::new(SPAN), record(&self.fade)))
            })
            .child(div().with_spring(
                "spring",
                SpringAnimation::new(SpringConfig::new(100.0, 10.0, 1.0)).to(px(if self.started {
                    100.
                } else {
                    0.
                })),
                move |element, value| {
                    spring.borrow_mut().push(value.0);
                    element.left(value)
                },
            ))
    }
}

/// The running fade, the started fade and the spring on the frame after the
/// one that starts them, when that frame's own work takes `overrun`.
fn after_start(overrun: Duration) -> (f32, f32, f32) {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, _| Motion::default());
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    };
    draw(&mut cx);
    cx.executor().advance_clock(2 * FRAME);
    draw(&mut cx);
    cx.executor().advance_clock(FRAME);
    window
        .update(&mut cx, |motion, _, cx| {
            motion.started = true;
            motion.slow = Some(overrun);
            cx.notify();
        })
        .unwrap();
    // The update draws the frame that starts them.
    cx.executor().advance_clock(FRAME);
    draw(&mut cx);
    window
        .update(&mut cx, |motion, _, _| {
            let last = |values: &Values| *values.borrow().last().unwrap();
            assert_eq!(
                motion.fade.borrow().len(),
                2,
                "the started fade is drawn twice"
            );
            (
                last(&motion.running),
                last(&motion.fade),
                last(&motion.spring),
            )
        })
        .unwrap()
}

#[test]
fn motion_a_late_frame_starts_moves_one_frame_next_while_running_motion_keeps_time() {
    let (running_prompt, fade_prompt, spring_prompt) = after_start(Duration::ZERO);
    let (running_late, fade_late, spring_late) = after_start(OVERRUN);

    let one_frame = FRAME.as_secs_f32() / SPAN.as_secs_f32();
    assert!(
        (fade_prompt - one_frame).abs() < 1e-4,
        "{fade_prompt} vs {one_frame}"
    );
    assert_eq!(
        fade_late, fade_prompt,
        "the started fade moves one frame on"
    );
    assert!(spring_prompt > 0.0, "the spring moved: {spring_prompt}");
    assert_eq!(
        spring_late, spring_prompt,
        "the started spring moves one frame on"
    );

    let behind = OVERRUN.as_secs_f32() / SPAN.as_secs_f32();
    assert!(
        (running_late - running_prompt - behind).abs() < 1e-4,
        "the running fade keeps real time: {running_prompt} then {running_late}"
    );
}
