//! A oneshot animation's last frame, and the moment a frame shows
//! ([`crate::fast::frame_time`]).
//!
//! The frame after the one that drew a fade a hair short of its end draws it
//! at its end, wherever the faded element sits — in a cached view, in a scroll
//! container, on a row added later — when frames come as the platform
//! delivers them: the next-frame callbacks run, and the window is drawn only
//! if they left it dirty. And a frame drawn again from scratch at the moment
//! the last one showed paints the fade where that one did, a hair short of its
//! end included.

use std::{cell::Cell, rc::Rc, time::Duration};

use crate::{
    Animation, AnimationExt as _, AppContext as _, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, StyleRefinement,
    Styled as _, TestAppContext, Window, WindowHandle, div, hsla, prelude::FluentBuilder as _, px,
};

const FADE: Duration = Duration::from_millis(60);

/// Rows, the last faded in when it arrives.
struct Pane {
    rows: usize,
    scroll: bool,
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = (0..self.rows).map(|ix| {
            let row = div()
                .h(px(20.))
                .w(px(100. + ix as f32))
                .bg(hsla(0.3, 0.5, 0.5, 1.));
            if ix + 1 == self.rows && ix > 0 {
                div()
                    .child(row)
                    .with_animation(("row-in", ix), Animation::new(FADE), |el, t| el.opacity(t))
                    .into_any_element()
            } else {
                row.into_any_element()
            }
        });
        div()
            .id("list")
            .size_full()
            .flex()
            .flex_col()
            .when(self.scroll, |list| list.overflow_y_scroll())
            .children(rows)
    }
}

struct Root {
    pane: Entity<Pane>,
    cached: bool,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let pane = self.pane.clone();
        div().size(px(300.)).child(if self.cached {
            pane.cached(StyleRefinement::default().w(px(200.)).h(px(200.)))
                .into_any_element()
        } else {
            pane.into_any_element()
        })
    }
}

/// The alpha the newest row is painted at.
fn alpha(window: &Window, rows: usize) -> Option<f32> {
    let width = 100. + (rows - 1) as f32;
    window
        .rendered_frame
        .scene
        .quads
        .iter()
        .find(|quad| (quad.bounds.size.width.0 / window.scale_factor() - width).abs() < 0.25)
        .map(|quad| quad.background.solid.a)
}

/// Delivers frames as a platform would until none is asked for, giving the
/// newest row's alpha in each frame drawn.
fn run_frames(cx: &mut TestAppContext, window: WindowHandle<Root>, rows: usize) -> Vec<f32> {
    let mut alphas = Vec::new();
    for _ in 0..200 {
        cx.executor().advance_clock(Duration::from_millis(4));
        let (ran, drawn) = cx
            .update_window(window.into(), |_, window, cx| {
                let ran = window.simulate_next_frame(cx);
                let drawn = window.invalidator.is_dirty();
                if drawn {
                    window.draw(cx).clear(cx);
                }
                (ran, drawn.then(|| alpha(window, rows)).flatten())
            })
            .unwrap();
        alphas.extend(drawn);
        if ran == 0 && drawn.is_none() {
            return alphas;
        }
    }
    panic!("the fade never stopped asking for frames: {alphas:?}");
}

fn ends_at_one(cached: bool, scroll: bool) {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(move |_, cx| Root {
        pane: cx.new(|_| Pane { rows: 1, scroll }),
        cached,
    });
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    window
        .update(&mut cx, |root, _, cx| {
            root.pane.update(cx, |pane, cx| {
                pane.rows = 3;
                cx.notify();
            })
        })
        .unwrap();
    let alphas = run_frames(&mut cx, window, 3);
    assert!(alphas.len() > 5, "the fade is drawn as it goes: {alphas:?}");
    assert_eq!(
        alphas.last().copied(),
        Some(1.0),
        "cached {cached}, scroll {scroll}: the fade rests short of its end: {alphas:?}"
    );
}

#[test]
fn a_fade_ends_at_its_end() {
    ends_at_one(false, false);
}

#[test]
fn a_fade_in_a_cached_view_ends_at_its_end() {
    ends_at_one(true, false);
}

#[test]
fn a_fade_in_a_scroll_container_ends_at_its_end() {
    ends_at_one(false, true);
}

#[test]
fn a_fade_in_a_cached_scroll_container_ends_at_its_end() {
    ends_at_one(true, true);
}

/// The frame shown draws the fade a hair short of its end; a frame drawn from
/// scratch after the end, at the moment the shown one showed, paints the same.
#[test]
fn a_frame_drawn_again_at_the_last_frames_moment_paints_the_same_fade() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(move |_, cx| Root {
        pane: cx.new(|_| Pane {
            rows: 3,
            scroll: false,
        }),
        cached: true,
    });
    let shown = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            alpha(window, 3)
        })
        .unwrap()
    };
    assert_eq!(shown(&mut cx), Some(0.0), "the fade starts at nothing");
    cx.executor().advance_clock(FADE - Duration::from_millis(1));
    let short = shown(&mut cx).expect("the row is painted");
    assert!(
        short > 0.9 && short < 1.0,
        "a hair short of its end: {short}"
    );
    let moment = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, _| window.frame_time())
            .unwrap()
    };
    let shown_at = moment(&mut cx);

    cx.executor().advance_clock(Duration::from_millis(5));
    let again = cx
        .update_window(window.into(), |_, window, cx| {
            window.repeat_frame_time();
            window.refresh();
            window.draw(cx).clear(cx);
            alpha(window, 3)
        })
        .unwrap();
    assert_eq!(again, Some(short), "drawn again at the moment shown");
    assert_eq!(moment(&mut cx), shown_at, "the frame drawn again says so");

    let later = cx
        .update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            alpha(window, 3)
        })
        .unwrap();
    assert_eq!(later, Some(1.0), "the next frame shows its own moment");
    assert!(moment(&mut cx) > shown_at, "and says so");
}

/// Records the app's moment each time it is built.
struct Clock {
    read: std::rc::Rc<std::cell::Cell<Option<scheduler::Instant>>>,
}

impl Render for Clock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.read.set(Some(cx.frame_time()));
        div().size_full()
    }
}

/// The app's moment is the frame's while a window draws, the frame drawn
/// again included, and now between frames.
#[test]
fn the_apps_moment_is_the_frames_while_it_draws() {
    let mut cx = TestAppContext::single();
    let read = std::rc::Rc::new(std::cell::Cell::new(None));
    let window = cx.add_window({
        let read = read.clone();
        move |_, _| Clock { read }
    });
    let frame = |cx: &mut TestAppContext, repeat: bool| {
        cx.update_window(window.into(), |_, window, cx| {
            if repeat {
                window.repeat_frame_time();
            }
            window.refresh();
            window.draw(cx).clear(cx);
            window.frame_time()
        })
        .unwrap()
    };
    let first = frame(&mut cx, false);
    assert_eq!(read.get(), Some(first), "read while drawing");
    cx.executor().advance_clock(Duration::from_millis(7));
    let between = cx.update(|cx| cx.frame_time());
    assert_eq!(
        between,
        first + Duration::from_millis(7),
        "now between frames"
    );
    assert_eq!(frame(&mut cx, true), first, "drawn again");
    assert_eq!(read.get(), Some(first), "read while drawn again");
    assert_eq!(frame(&mut cx, false), between, "the next frame's own");
}

/// Rows in a `list`, the last faded in when it arrives, as a chat's
/// transcript shows its rows.
struct ListPane {
    state: crate::ListState,
    rows: Rc<Cell<usize>>,
}

impl Render for ListPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        div().size_full().child(
            crate::list(self.state.clone(), move |ix, _, _| {
                let row = div()
                    .h(px(20.))
                    .w(px(100. + ix as f32))
                    .bg(hsla(0.3, 0.5, 0.5, 1.));
                if ix + 1 == rows.get() && ix > 0 {
                    div()
                        .child(row)
                        .with_animation(("row-in", ix), Animation::new(FADE), |el, t| el.opacity(t))
                        .into_any_element()
                } else {
                    row.into_any_element()
                }
            })
            .size_full(),
        )
    }
}

struct ListRoot {
    pane: Entity<ListPane>,
}

impl Render for ListRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(300.)).child(
            self.pane
                .clone()
                .cached(StyleRefinement::default().w(px(200.)).h(px(100.))),
        )
    }
}

fn list_fade_ends_at_one(rows_before: usize, scrolled: bool) {
    let mut cx = TestAppContext::single();
    let rows = Rc::new(Cell::new(rows_before));
    let state = crate::ListState::new(rows_before, crate::ListAlignment::Bottom, px(50.));
    let window = cx.add_window({
        let (rows, state) = (rows.clone(), state.clone());
        move |_, cx| ListRoot {
            pane: cx.new(|_| ListPane { state, rows }),
        }
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap()
    };
    draw(&mut cx);
    if scrolled {
        // Scrolled for a while, so the list's content may be on a layer.
        for step in 0..12 {
            state.scroll_by(px(if step % 2 == 0 { -5. } else { 5. }));
            window
                .update(&mut cx, |root, _, cx| {
                    root.pane.update(cx, |_, cx| cx.notify())
                })
                .unwrap();
            draw(&mut cx);
        }
        for _ in 0..4 {
            draw(&mut cx);
        }
    }
    let total = rows_before + 1;
    rows.set(total);
    state.splice(rows_before..rows_before, 1);
    window
        .update(&mut cx, |root, _, cx| {
            root.pane.update(cx, |_, cx| cx.notify())
        })
        .unwrap();
    let mut alphas = Vec::new();
    for _ in 0..200 {
        cx.executor().advance_clock(Duration::from_millis(4));
        let (ran, drawn) = cx
            .update_window(window.into(), |_, window, cx| {
                let ran = window.simulate_next_frame(cx);
                let drawn = window.invalidator.is_dirty();
                if drawn {
                    window.draw(cx).clear(cx);
                }
                (ran, drawn.then(|| alpha(window, total)).flatten())
            })
            .unwrap();
        alphas.extend(drawn);
        if ran == 0 && drawn.is_none() {
            break;
        }
    }
    let scratch = cx
        .update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            alpha(window, total)
        })
        .unwrap();
    assert_eq!(scratch, Some(1.0), "a frame from scratch shows the row");
    assert_eq!(
        alphas.last().copied(),
        Some(1.0),
        "rows {rows_before}, scrolled {scrolled}: the fade rests short of its end: {alphas:?}"
    );
}

#[test]
fn a_fade_in_a_lists_new_row_ends_at_its_end() {
    list_fade_ends_at_one(2, false);
}

#[test]
fn a_fade_in_a_long_lists_new_row_ends_at_its_end() {
    list_fade_ends_at_one(40, false);
}

#[test]
fn a_fade_in_a_scrolled_lists_new_row_ends_at_its_end() {
    list_fade_ends_at_one(40, true);
}
