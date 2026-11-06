//! When a press on a draggable element becomes a drag. See [`crate::fast::drag`]
//! (ported from zui `0dc69aa`, Apache-2.0).

use crate::{
    AppContext as _, Context, InteractiveElement as _, IntoElement, Modifiers, MouseButton,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, TestAppContext,
    Window, div, point, px,
};

struct Dragged;

impl Render for Dragged {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A 100 × 100 draggable row at the window's top left, counting its clicks.
struct DragSource {
    clicks: usize,
}

impl Render for DragSource {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("source")
                .size(px(100.))
                .on_click(cx.listener(|this, _, _, _| this.clicks += 1))
                .on_drag((), |_, _, _, cx| cx.new(|_| Dragged)),
        )
    }
}

#[crate::test]
fn a_drag_needs_travel_past_the_threshold_with_the_button_held(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| DragSource { clicks: 0 });
    let none = Modifiers::default();
    let start = point(px(50.), px(50.));

    // A press that rolls 3 px, as a trackpad press does, stays a click.
    cx.simulate_mouse_down(start, MouseButton::Left, none);
    cx.simulate_mouse_move(point(px(53.), px(50.)), MouseButton::Left, none);
    assert!(!cx.update(|_, cx| cx.has_active_drag()));
    cx.simulate_mouse_up(point(px(53.), px(50.)), MouseButton::Left, none);
    assert_eq!(view.read_with(cx, |view, _| view.clicks), 1);

    // A press whose release never arrived: moving with no button held is a hover,
    // not a drag, and the stale press is forgotten, so holding the button later
    // without a new press does not lift it either.
    cx.simulate_mouse_down(start, MouseButton::Left, none);
    cx.simulate_mouse_move(point(px(70.), px(50.)), None, none);
    assert!(!cx.update(|_, cx| cx.has_active_drag()));
    cx.simulate_mouse_move(point(px(90.), px(50.)), MouseButton::Left, none);
    assert!(!cx.update(|_, cx| cx.has_active_drag()));

    // Held and moved past 4 px: a drag.
    cx.simulate_mouse_down(start, MouseButton::Left, none);
    cx.simulate_mouse_move(point(px(54.5), px(50.)), MouseButton::Left, none);
    assert!(cx.update(|_, cx| cx.has_active_drag()));
}
