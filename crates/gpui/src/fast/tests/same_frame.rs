//! A frame that draws what the screen shows is not presented
//! ([`crate::fast::same_frame`]): a notify that changes nothing draws but
//! hands the platform nothing, while a change, a platform that can't say what
//! it draws with, an epoch that moved, a resize or a frame that requires a
//! presentation still present. Next-frame callbacks run on frames left
//! unpresented.

use std::cell::Cell;
use std::rc::Rc;

use crate::{
    AnyWindowHandle, AppContext as _, Context, IntoElement, ParentElement as _, PresentCounts,
    Render, RequestFrameOptions, Styled as _, TestAppContext, Window, WindowHandle, div, hsla, px,
    size,
};

/// A column of bars, the first in `hue`.
struct Bars {
    hue: Rc<Cell<f32>>,
    renders: Rc<Cell<usize>>,
}

impl Render for Bars {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().h(px(20.)).bg(hsla(self.hue.get(), 0.8, 0.5, 1.)))
            .children((0..4).map(|row| {
                div()
                    .h(px(10.))
                    .w(px(40. + row as f32 * 10.))
                    .bg(hsla(0.3, 0.5, 0.4, 1.))
            }))
    }
}

struct Fixture {
    window: WindowHandle<Bars>,
    hue: Rc<Cell<f32>>,
    renders: Rc<Cell<usize>>,
}

impl Fixture {
    /// A window drawn and presented once, whose platform says `epoch`.
    fn new(cx: &mut TestAppContext, epoch: Option<u64>) -> Self {
        crate::fast::settle::set_platform_frames(true);
        let hue = Rc::new(Cell::new(0.1));
        let renders = Rc::new(Cell::new(0));
        let window = cx.add_window({
            let (hue, renders) = (hue.clone(), renders.clone());
            move |_, _| Bars { hue, renders }
        });
        cx.set_presentation_epoch(window.into(), epoch);
        let test_window = cx.test_window(window.into());
        // The frames of an active window, which the inactive window's frame limit leaves alone.
        test_window.simulate_active_status_change(true);
        test_window.simulate_frame_request(RequestFrameOptions::default());
        Fixture {
            window,
            hue,
            renders,
        }
    }

    fn handle(&self) -> AnyWindowHandle {
        self.window.into()
    }

    fn notify(&self, cx: &mut TestAppContext) {
        self.window.update(cx, |_, _, cx| cx.notify()).unwrap();
    }

    fn frame(&self, cx: &mut TestAppContext, options: RequestFrameOptions) {
        cx.test_window(self.handle())
            .simulate_frame_request(options);
    }

    fn counts(&self, cx: &mut TestAppContext) -> PresentCounts {
        self.window
            .update(cx, |_, window, _| window.present_counts())
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        crate::fast::settle::set_platform_frames(false);
    }
}

#[crate::test]
fn a_notify_that_changes_nothing_draws_but_is_not_presented(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, Some(0));
    let start = f.counts(cx);
    assert_eq!(start.presented, 1, "the first frame is presented");
    let renders = f.renders.get();
    for _ in 0..5 {
        f.notify(cx);
        f.frame(cx, RequestFrameOptions::default());
    }
    assert_eq!(f.renders.get(), renders + 5, "each notify draws");
    assert_eq!(
        f.counts(cx),
        PresentCounts {
            presented: 1,
            skipped: 5
        },
        "no frame drawn the same is presented"
    );

    f.hue.set(0.7);
    f.notify(cx);
    f.frame(cx, RequestFrameOptions::default());
    assert_eq!(f.counts(cx).presented, 2, "a change is presented");

    f.notify(cx);
    f.frame(cx, RequestFrameOptions::default());
    assert_eq!(
        f.counts(cx),
        PresentCounts {
            presented: 2,
            skipped: 6
        },
        "and the frame after it, drawn the same, is not"
    );
}

#[crate::test]
fn a_platform_that_cannot_say_what_it_draws_with_presents_every_frame(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, None);
    for _ in 0..3 {
        f.notify(cx);
        f.frame(cx, RequestFrameOptions::default());
    }
    assert_eq!(
        f.counts(cx),
        PresentCounts {
            presented: 4,
            skipped: 0
        }
    );
}

#[crate::test]
fn a_moved_epoch_presents_the_same_frame(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, Some(0));
    cx.set_presentation_epoch(f.handle(), Some(1));
    f.notify(cx);
    f.frame(cx, RequestFrameOptions::default());
    assert_eq!(
        f.counts(cx).presented,
        2,
        "the atlas or the drawable changed"
    );
    f.notify(cx);
    f.frame(cx, RequestFrameOptions::default());
    assert_eq!(f.counts(cx).skipped, 1, "and stayed so");
}

#[crate::test]
fn a_frame_that_requires_a_presentation_or_forces_a_render_presents(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, Some(0));
    f.notify(cx);
    f.frame(
        cx,
        RequestFrameOptions {
            require_presentation: true,
            ..Default::default()
        },
    );
    f.frame(
        cx,
        RequestFrameOptions {
            force_render: true,
            ..Default::default()
        },
    );
    assert_eq!(
        f.counts(cx),
        PresentCounts {
            presented: 3,
            skipped: 0
        }
    );
}

#[crate::test]
fn a_resize_presents(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, Some(0));
    let mut test_window = cx.test_window(f.handle());
    test_window.simulate_resize(size(px(320.), px(200.)));
    f.frame(cx, RequestFrameOptions::default());
    assert_eq!(f.counts(cx).presented, 2);
}

#[crate::test]
fn next_frame_callbacks_run_on_a_frame_left_unpresented(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, Some(0));
    let ran = Rc::new(Cell::new(0));
    f.window
        .update(cx, |_, window, cx| {
            let ran = ran.clone();
            window.on_next_frame(move |window, _| {
                ran.set(ran.get() + 1);
                window.refresh();
            });
            cx.notify();
        })
        .unwrap();
    f.frame(cx, RequestFrameOptions::default());
    assert_eq!(ran.get(), 1, "the callback ran");
    assert_eq!(
        f.counts(cx),
        PresentCounts {
            presented: 1,
            skipped: 1
        },
        "on a frame that drew the same and was not presented"
    );
}

#[crate::test]
fn scenes_compare_by_what_they_draw(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, Some(0));
    let quads = |cx: &mut TestAppContext| {
        // Through the window rather than its root view, which drawing updates.
        cx.update_window(f.handle(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            window.rendered_frame.scene.quads.clone()
        })
        .unwrap()
    };
    let a = quads(cx);
    let b = quads(cx);
    f.hue.set(0.9);
    let c = quads(cx);
    let same = |x: &[crate::Quad], y: &[crate::Quad]| {
        x.len() == y.len()
            && x.iter().zip(y).all(|(x, y)| {
                let mut one = crate::Scene::default();
                let mut other = crate::Scene::default();
                one.quads.push(*x);
                other.quads.push(*y);
                crate::fast::same_frame::same_scene(&one, &other)
            })
    };
    assert!(same(&a, &b), "the same view drawn twice draws the same");
    assert!(!same(&a, &c), "a colour that changed draws otherwise");
}
