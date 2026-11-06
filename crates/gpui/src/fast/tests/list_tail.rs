//! Tests of a `list`'s tail reservation (`fast/list_tail.rs`): the space a
//! list keeps below its rows from one row on, how rows appended take it up,
//! and a list with one on a scroll layer.

use crate::fast::layers::policy::{Decision, last_decision};
use crate::{
    AnyWindowHandle, App, AppContext as _, Context, IntoElement, ListAlignment, ListOffset,
    ListState, ParentElement as _, Pixels, Render, ScrollDelta, ScrollEvent, Styled as _,
    TestAppContext, TouchPhase, Window, WindowHandle, div, point, px, rgb,
};
use std::{cell::RefCell, rc::Rc};

/// How tall the list is, in pixels.
const VIEWPORT_HEIGHT: f32 = 100.;

/// How tall row `row` is: 20 px, or 20, 30 or 40 px when `varied`.
fn row_height(row: usize, varied: bool) -> f32 {
    if varied {
        20. + (row % 3) as f32 * 10.
    } else {
        20.
    }
}

/// A white panel holding a list 100 px tall at the top left of the window.
/// Every row the list renders is logged.
struct Page {
    state: ListState,
    varied: bool,
    rendered: Rc<RefCell<Vec<usize>>>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rendered = self.rendered.clone();
        let varied = self.varied;
        div().size_full().bg(rgb(0xffffff)).child(
            crate::list(self.state.clone(), move |row, _, _| {
                rendered.borrow_mut().push(row);
                div()
                    .w(px(200.))
                    .h(px(row_height(row, varied)))
                    .bg(crate::hsla((row % 97) as f32 / 97., 0.5, 0.5, 1.))
                    .into_any_element()
            })
            .w(px(200.))
            .h(px(VIEWPORT_HEIGHT)),
        )
    }
}

fn page(cx: &mut TestAppContext, state: ListState, varied: bool) -> WindowHandle<Page> {
    let window = cx.add_window(move |_, _| Page {
        state,
        varied,
        rendered: Rc::default(),
    });
    draw(cx, window.into());
    window
}

fn with_window<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| f(window, cx))
        .unwrap()
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    with_window(cx, window, |window, cx| window.draw(cx).clear(cx));
}

/// Changes the list of `handle` with `f`, notifies its page and draws it.
fn update(cx: &mut TestAppContext, handle: WindowHandle<Page>, f: impl FnOnce(&ListState)) {
    handle
        .update(cx, |page, _, cx| {
            f(&page.state);
            cx.notify();
        })
        .unwrap();
    draw(cx, handle.into());
}

/// Scrolls by `dy` with the wheel over the list, and draws the frame that
/// follows, unless the scroll drew it.
fn wheel(cx: &mut TestAppContext, window: AnyWindowHandle, dy: f32) {
    let frame = with_window(cx, window, |window, _| window.fast_layers.frame);
    with_window(cx, window, |window, cx| {
        window.dispatch_event(
            crate::PlatformInput::Scroll(ScrollEvent {
                position: point(px(20.), px(20.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                modifiers: Default::default(),
                touch_phase: TouchPhase::Moved,
                source: crate::ScrollSource::Pointer,
                momentum_phase: None,
            }),
            cx,
        );
    });
    if with_window(cx, window, |window, _| window.fast_layers.frame) == frame {
        draw(cx, window);
    }
}

fn state(handle: WindowHandle<Page>, cx: &mut TestAppContext) -> ListState {
    handle.update(cx, |page, _, _| page.state.clone()).unwrap()
}

/// The top of row `ix` on screen, if the list showed it.
fn top_of(handle: WindowHandle<Page>, cx: &mut TestAppContext, ix: usize) -> Option<Pixels> {
    state(handle, cx)
        .bounds_for_item(ix)
        .map(|bounds| bounds.top())
}

/// How tall the list holds the rows from `ix` on.
fn height_from(handle: WindowHandle<Page>, cx: &mut TestAppContext, ix: usize) -> Pixels {
    let state = state(handle, cx);
    state.offset_for_item(state.item_count()) - state.offset_for_item(ix)
}

/// A list scrolled to its last row walks back so that its rows fill the
/// viewport; with a reservation from that row, the row stays at the top and
/// the rest of the viewport is space below it, which rows appended take up
/// until they fill it.
#[crate::test]
fn a_reservation_keeps_its_row_at_the_top_until_rows_below_fill_it(cx: &mut TestAppContext) {
    // An overdraw measures the rows appended below the viewport.
    let handle = page(cx, ListState::new(30, ListAlignment::Top, px(50.)), false);
    let scroll_to_29 = |state: &ListState| {
        state.scroll_to(ListOffset {
            item_ix: 29,
            offset_in_item: px(0.),
        })
    };

    // Without a reservation the list walks back from its last row.
    update(cx, handle, scroll_to_29);
    assert_eq!(top_of(handle, cx, 29), Some(px(80.)));
    assert_eq!(height_from(handle, cx, 29), px(20.));
    assert!(!state(handle, cx).tail_reservation_filled());

    update(cx, handle, |state| {
        state.set_tail_reservation(Some((29, px(0.))));
        scroll_to_29(state);
    });
    assert_eq!(top_of(handle, cx, 29), Some(px(0.)));
    assert_eq!(height_from(handle, cx, 29), px(100.));
    // Rows above are counted as the list measured them.
    let state_now = state(handle, cx);
    assert_eq!(
        state_now.offset_for_item(30) - state_now.offset_for_item(28),
        px(120.)
    );
    assert!(!state(handle, cx).tail_reservation_filled());

    // Two rows appended take up 40 px of the 80 left.
    update(cx, handle, |state| state.splice(30..30, 2));
    assert_eq!(top_of(handle, cx, 29), Some(px(0.)));
    assert_eq!(top_of(handle, cx, 31), Some(px(40.)));
    assert_eq!(height_from(handle, cx, 29), px(100.));
    assert!(!state(handle, cx).tail_reservation_filled());

    // Two more fill it, and a fifth goes past it.
    update(cx, handle, |state| state.splice(32..32, 2));
    assert_eq!(height_from(handle, cx, 29), px(100.));
    assert!(state(handle, cx).tail_reservation_filled());
    update(cx, handle, |state| state.splice(34..34, 1));
    assert_eq!(height_from(handle, cx, 29), px(120.));
    assert!(state(handle, cx).tail_reservation_filled());

    // Taking the reservation away leaves the rows at their own heights.
    update(cx, handle, |state| {
        state.splice(34..35, 0);
        state.set_tail_reservation(None);
    });
    assert_eq!(height_from(handle, cx, 29), px(100.));
    update(cx, handle, |state| state.splice(33..34, 0));
    assert_eq!(height_from(handle, cx, 29), px(80.));
    assert!(!state(handle, cx).tail_reservation_filled());
}

/// The inset is taken off the viewport's height, so the reserved row comes
/// to rest that far from the top; and a reservation changed with nothing
/// else draws the list again: the list's state changed.
#[crate::test]
fn a_reservation_takes_its_inset_off_the_viewport(cx: &mut TestAppContext) {
    let handle = page(cx, ListState::new(30, ListAlignment::Top, px(0.)), false);
    update(cx, handle, |state| {
        state.set_tail_reservation(Some((28, px(30.))));
        state.scroll_to(ListOffset {
            item_ix: 28,
            offset_in_item: px(0.),
        });
    });
    assert_eq!(height_from(handle, cx, 28), px(70.));
    assert_eq!(top_of(handle, cx, 28), Some(px(30.)));

    // No notification: the reservation's change is what draws the list.
    handle
        .update(cx, |page, _, _| {
            page.state.set_tail_reservation(Some((28, px(10.))));
        })
        .unwrap();
    draw(cx, handle.into());
    assert_eq!(height_from(handle, cx, 28), px(90.));
    // The scroll stays where it was: moving it is the application's.
    assert_eq!(top_of(handle, cx, 28), Some(px(30.)));
}

/// A reservation from a row past rows whose heights are unknown waits for
/// the list to walk back over them rather than counting them as nothing.
#[crate::test]
fn a_reservation_past_unmeasured_rows_adds_no_space(cx: &mut TestAppContext) {
    let handle = page(cx, ListState::new(100, ListAlignment::Top, px(0.)), false);
    update(cx, handle, |state| {
        state.set_tail_reservation(Some((0, px(0.))));
        state.scroll_to(ListOffset {
            item_ix: 99,
            offset_in_item: px(0.),
        });
    });
    assert_eq!(height_from(handle, cx, 99), px(20.));
    assert!(state(handle, cx).tail_reservation_filled());
}

/// A list holding a reservation scrolls on its scroll layer: it draws what
/// it draws without one, frame by frame, composites most frames, and keeps
/// its last row, with the space below it, on the layer: the row is rendered
/// once into the overscan, out of sight, and not again as it shows.
#[crate::test]
fn a_list_with_a_reservation_scrolls_on_its_layer(cx: &mut TestAppContext) {
    if !crate::fast::layers::COMPILED {
        return;
    }
    let reserved = |count: usize| {
        let state = ListState::new(count, ListAlignment::Top, px(0.));
        state.set_tail_reservation(Some((count - 2, px(10.))));
        state.scroll_to(ListOffset {
            item_ix: count - 2,
            offset_in_item: px(0.),
        });
        state
    };
    let with_layers = page(cx, reserved(300), true);
    let without_layers = page(cx, reserved(300), true);
    with_window(cx, without_layers.into(), |window, _| {
        window.set_scroll_layers(false)
    });
    draw(cx, with_layers.into());
    draw(cx, without_layers.into());
    assert_eq!(height_from(with_layers, cx, 298), px(90.));
    assert_eq!(top_of(with_layers, cx, 298), Some(px(10.)));

    // Up from the end, out of sight of the last row, which the list
    // measured with the space below it.
    for handle in [with_layers, without_layers] {
        update(cx, handle, |state| {
            state.scroll_to(ListOffset {
                item_ix: 291,
                offset_in_item: px(0.),
            })
        });
    }
    assert!(top_of(with_layers, cx, 299).is_none_or(|top| top >= px(VIEWPORT_HEIGHT)));

    let log = with_layers
        .update(cx, |page, _, _| page.rendered.clone())
        .unwrap();
    log.borrow_mut().clear();
    let mut composited = 0;
    // Promoted out of sight of the last row, down to the end and past it,
    // back up and down again.
    let mut deltas = vec![5., -5.];
    deltas.extend([-35.; 10]);
    deltas.extend([35.; 10]);
    deltas.extend([20., -7., 13., -26., -40., -40., 40.]);
    for (frame, dy) in deltas.iter().copied().enumerate() {
        wheel(cx, with_layers.into(), dy);
        wheel(cx, without_layers.into(), dy);
        let decided = with_window(cx, with_layers.into(), |window, _| {
            let id = window.fast_layers.layers.keys().next().cloned();
            id.and_then(|id| last_decision(window, &id))
        });
        if decided == Some(Decision::Composite) {
            composited += 1;
        }
        let quads = |window: &mut Window| {
            let mut quads: Vec<String> = window
                .rendered_frame
                .scene
                .quads
                .iter()
                .map(|quad| format!("{:?} {:?}", quad.bounds, quad.background))
                .collect();
            quads.sort();
            quads
        };
        let expected = with_window(cx, without_layers.into(), |window, _| quads(window));
        let actual = with_window(cx, with_layers.into(), |window, _| quads(window));
        assert_eq!(actual, expected, "frame {frame}, scrolled by {dy}");
        assert_eq!(
            height_from(with_layers, cx, 298),
            height_from(without_layers, cx, 298),
            "frame {frame}"
        );
    }
    assert!(composited >= deltas.len() / 2, "composited {composited}");
    // Once, into the layer's overscan as it was promoted.
    let last_rendered = log.borrow().iter().filter(|row| **row == 299).count();
    assert_eq!(last_rendered, 1, "the last row is rendered once");
}
