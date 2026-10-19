//! A frame settles before it is shown ([`crate::fast::settle`]): a view that measures an element
//! as it prepaints and defers a notify with the new size shows that size in the very frame the
//! element changed in, not one frame later. Work deferred by a draw that leaves nothing to draw
//! costs no second draw.

use std::cell::Cell;
use std::rc::Rc;

use crate::{
    Context, EntityId, IntoElement, ParentElement as _, Render, RequestFrameOptions, Styled as _,
    TestAppContext, Window, canvas, div, hsla, px,
};

/// A bar as tall as its view last measured its block, beside the block, which is `tall` high.
struct Measured {
    tall: Rc<Cell<f32>>,
    measured: Rc<Cell<f32>>,
    renders: Rc<Cell<usize>>,
    /// Defers work on every draw that changes nothing.
    idle_defer: bool,
}

impl Render for Measured {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let view: EntityId = cx.entity_id();
        let measured = Rc::clone(&self.measured);
        let idle_defer = self.idle_defer;
        div()
            .flex()
            .items_start()
            .child(
                div()
                    .w(px(10.))
                    .h(px(self.measured.get()))
                    .bg(hsla(0.6, 0.8, 0.5, 1.)),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let height = f32::from(bounds.size.height);
                        if measured.get() != height {
                            measured.set(height);
                            cx.defer(move |cx| cx.notify(view));
                        } else if idle_defer {
                            cx.defer(|_| {});
                        }
                    },
                    |_, (), _, _| {},
                )
                .w(px(20.))
                .h(px(self.tall.get())),
            )
    }
}

/// The height of the bar in each frame shown, in pixels: the one quad, 10 px wide at the
/// window's scale.
fn bars() -> Vec<f32> {
    crate::fast::settle::presented()
        .iter()
        .map(|quads| {
            quads.first().map_or(0., |bounds| {
                bounds.size.height.as_f32() * 10. / bounds.size.width.as_f32()
            })
        })
        .collect()
}

/// The block grows from 30 to 60 px: the first frame shown after it grew has the bar at 60, as
/// the measurement the draw deferred is drawn before the frame is shown.
#[crate::test]
fn a_measured_size_shows_in_the_frame_it_changed_in(cx: &mut TestAppContext) {
    crate::fast::settle::set_platform_frames(true);
    let tall = Rc::new(Cell::new(30.));
    let measured = Rc::new(Cell::new(0.));
    let renders = Rc::new(Cell::new(0));
    let window = cx.add_window({
        let (tall, measured, renders) = (tall.clone(), measured, renders);
        move |_, _| Measured {
            tall,
            measured,
            renders,
            idle_defer: false,
        }
    });
    let test_window = cx.test_window(window.into());
    test_window.simulate_frame_request(RequestFrameOptions::default());
    assert_eq!(bars().last().copied(), Some(30.), "the first frame shown");

    tall.set(60.);
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    let shown = bars().len();
    test_window.simulate_frame_request(RequestFrameOptions::default());
    assert_eq!(&bars()[shown..], [60.], "one frame shown, already at 60");
    crate::fast::settle::set_platform_frames(false);
}

/// A draw that defers work which leaves the window clean is drawn once.
#[crate::test]
fn a_deferral_that_changes_nothing_costs_no_draw(cx: &mut TestAppContext) {
    crate::fast::settle::set_platform_frames(true);
    let tall = Rc::new(Cell::new(30.));
    let measured = Rc::new(Cell::new(30.));
    let renders = Rc::new(Cell::new(0));
    let window = cx.add_window({
        let (tall, measured, renders) = (tall, measured, renders.clone());
        move |_, _| Measured {
            tall,
            measured,
            renders,
            idle_defer: true,
        }
    });
    let test_window = cx.test_window(window.into());
    test_window.simulate_frame_request(RequestFrameOptions::default());
    let before = renders.get();
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    test_window.simulate_frame_request(RequestFrameOptions::default());
    assert_eq!(renders.get(), before + 1, "drawn once");
    crate::fast::settle::set_platform_frames(false);
}
