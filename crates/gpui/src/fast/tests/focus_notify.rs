//! Tests of a view notified by a focus listener, which runs once the frame
//! that moved the focus is drawn.

use crate::{
    Context, FocusHandle, IntoElement, Render, Subscription, TestAppContext, Window, div, hsla,
    prelude::*, px,
};

/// A text field whose caret starts blinking when it gains focus, as GPUI
/// Kit's does.
struct Field {
    focus: FocusHandle,
    blinking: bool,
    drawn_blinking: usize,
    _on_focus: Subscription,
}

impl Render for Field {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.blinking {
            self.drawn_blinking += 1;
        }
        div()
            .track_focus(&self.focus)
            .size(px(20.))
            .when(self.blinking, |field| field.bg(hsla(0., 0., 0., 1.)))
    }
}

fn a_caret_started_by_focus_is_drawn(cx: &mut TestAppContext, retention: bool) {
    let window = cx.add_window(|window, cx| {
        window.set_view_retention(retention);
        let focus = cx.focus_handle();
        let on_focus = cx.on_focus(&focus, window, |field: &mut Field, _, cx| {
            field.blinking = true;
            cx.notify();
        });
        Field {
            focus,
            blinking: false,
            drawn_blinking: 0,
            _on_focus: on_focus,
        }
    });
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |field, window, cx| window.focus(&field.focus, cx))
        .unwrap();
    cx.run_until_parked();
    let drawn = window
        .read_with(cx, |field, _| field.drawn_blinking)
        .unwrap();
    assert!(drawn > 0, "the caret started by focus was never drawn");
}

#[gpui::test]
fn a_caret_started_by_a_focus_listener_is_drawn(cx: &mut TestAppContext) {
    a_caret_started_by_focus_is_drawn(cx, true);
}

#[gpui::test]
fn a_caret_started_by_a_focus_listener_is_drawn_without_retention(cx: &mut TestAppContext) {
    a_caret_started_by_focus_is_drawn(cx, false);
}
