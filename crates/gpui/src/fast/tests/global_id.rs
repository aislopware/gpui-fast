//! Tests of the ids the inspector finds elements by.

#![cfg(any(feature = "inspector", debug_assertions))]

use crate::{
    AnyWindowHandle, AppContext as _, Context, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, Window, div, px,
};

/// A few rows of text, for elements to be found by.
struct Rows;

impl Render for Rows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .children((0..4).map(|ix| div().h(px(20.)).child(format!("row {ix}"))))
    }
}

fn draw_frame(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

/// Elements are given the ids the inspector finds them by only while it is
/// open, since building one copies the whole element id stack. Opening it
/// has to bring them back on the next frame.
#[test]
fn inspector_ids_are_built_only_while_the_inspector_is_open() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, _| Rows);
    let inspector_ids = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, _| {
            window.rendered_frame.next_inspector_instance_ids.len()
        })
        .unwrap()
    };

    draw_frame(&mut cx, window.into());
    assert_eq!(inspector_ids(&mut cx), 0);

    cx.update_window(window.into(), |_, window, cx| window.toggle_inspector(cx))
        .unwrap();
    draw_frame(&mut cx, window.into());
    assert!(
        inspector_ids(&mut cx) > 0,
        "opening the inspector should give elements their ids again"
    );

    cx.update_window(window.into(), |_, window, cx| window.toggle_inspector(cx))
        .unwrap();
    draw_frame(&mut cx, window.into());
    assert_eq!(inspector_ids(&mut cx), 0);
}
