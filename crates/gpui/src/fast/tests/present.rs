//! Callbacks run as the frame reaches the screen (`Window::on_present`):
//! only on present, in paint order, again for a view drawn from last frame,
//! with the bounds it is drawn at now. See [`crate::fast::present`].

use std::{cell::RefCell, rc::Rc};

use crate::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, PlatformInput, Render, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, StatefulInteractiveElement as _, Styled as _, TestAppContext, TouchPhase,
    Window, canvas, div, hsla, point, px,
};

type Log = Rc<RefCell<Vec<(&'static str, Bounds<Pixels>)>>>;

fn with_window<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| f(window, cx))
        .unwrap()
}

/// A canvas logging, as each frame holding it is presented, its name and
/// the bounds it was painted at.
fn synced(name: &'static str, log: &Log) -> impl IntoElement {
    let log = log.clone();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let log = log.clone();
            window.on_present(move || log.borrow_mut().push((name, bounds)));
        },
    )
    .w(px(40.))
    .h(px(20.))
}

struct Page {
    spacer: f32,
    pane: Entity<Pane>,
    other: Entity<Other>,
}

struct Pane {
    log: Log,
}

struct Other {
    count: usize,
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .child(synced("first", &self.log))
            .child(synced("second", &self.log))
    }
}

impl Render for Other {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().h(px(10.)).w(px(10. + self.count as f32))
    }
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.other.clone())
            .child(div().flex_none().h(px(self.spacer)))
            .child(self.pane.clone())
    }
}

/// Drawing alone runs nothing; presenting runs each callback once, in paint
/// order; a frame drawing the pane from last frame runs them again; one
/// drawing it moved runs them with where it is now.
#[test]
fn callbacks_run_on_present_in_order_and_again_when_drawn_from_last_frame() {
    let mut cx = TestAppContext::single();
    let log: Log = Rc::default();
    let page_log = log.clone();
    let window = cx.add_window(move |_, cx| Page {
        spacer: 0.,
        pane: cx.new(|_| Pane { log: page_log }),
        other: cx.new(|_| Other { count: 0 }),
    });
    let take = |log: &Log| std::mem::take(&mut *log.borrow_mut());
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx)
    });
    assert!(take(&log).is_empty(), "drawn, not presented");
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw_and_present(cx)
    });
    let first = take(&log);
    assert_eq!(
        first.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["first", "second"]
    );

    // Another view changes; the pane is drawn from last frame.
    window
        .update(&mut cx, |page, _, cx| {
            page.other.update(cx, |other, cx| {
                other.count += 1;
                cx.notify();
            })
        })
        .unwrap();
    let reused = with_window(&mut cx, window.into(), |window, cx| {
        window.reset_layout_stats();
        window.draw_and_present(cx);
        window.layout_stats().views_reused
    });
    assert!(reused > 0, "the pane is drawn from last frame");
    assert_eq!(take(&log), first, "its callbacks run again");

    // The pane moves down: its callbacks run with where it is now.
    window
        .update(&mut cx, |page, _, cx| {
            page.spacer = 15.;
            cx.notify();
        })
        .unwrap();
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw_and_present(cx)
    });
    let moved = take(&log);
    assert_eq!(moved.len(), 2);
    for ((_, before), (_, after)) in first.iter().zip(&moved) {
        assert_eq!(after.origin.y, before.origin.y + px(15.));
    }
}

struct Scrolled {
    log: Log,
    scroll: ScrollHandle,
}

impl Render for Scrolled {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let log = self.log.clone();
        div().size_full().bg(hsla(0., 0., 1., 1.)).child(
            div()
                .id("scrolled")
                .w(px(200.))
                .h(px(100.))
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .children((0..30).map(move |ix| {
                    let row = div()
                        .h(px(20.))
                        .flex_none()
                        .bg(hsla(ix as f32 / 30., 0.5, 0.5, 1.));
                    if ix == 2 {
                        row.child(synced("row", &log)).into_any_element()
                    } else {
                        row.into_any_element()
                    }
                })),
        )
    }
}

/// A scroll container whose content hands the present a callback is drawn
/// without its tiles, so the callback runs with where the content is now.
#[test]
fn content_handing_the_present_a_callback_scrolls_without_tiles() {
    let mut cx = TestAppContext::single();
    let log: Log = Rc::default();
    let page_log = log.clone();
    let window = cx.add_window(move |_, _| Scrolled {
        log: page_log,
        scroll: ScrollHandle::new(),
    });
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw_and_present(cx)
    });
    let mut last = log.borrow_mut().pop().expect("presented").1;
    for _ in 0..12 {
        with_window(&mut cx, window.into(), |window, cx| {
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: point(px(20.), px(20.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-3.))),
                    modifiers: Default::default(),
                    touch_phase: TouchPhase::Moved,
                    momentum_phase: None,
                }),
                cx,
            );
            window.draw_and_present(cx);
        });
        let now = log.borrow_mut().pop().expect("presented").1;
        assert_eq!(now.origin.y, last.origin.y - px(3.));
        last = now;
    }
    let stats = with_window(&mut cx, window.into(), |window, _| window.layout_stats());
    assert_eq!(stats.layer_frames_composited, 0, "{stats:?}");
}
