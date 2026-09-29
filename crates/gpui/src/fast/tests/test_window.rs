//! Tests of what a test window records of what GPUI asked the platform.

use std::ops::Range;

use crate::{
    AnyWindowHandle, Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    IntoElement, Pixels, Point, Render, TestAppContext, UTF16Selection, Window, canvas, div, point,
    prelude::*, px, size,
};

/// A text field with nothing in it but a caret, ten pixels a character.
struct Caret {
    focus_handle: FocusHandle,
    at: usize,
}

impl Render for Caret {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let focus_handle = self.focus_handle.clone();
        div().size_full().track_focus(&self.focus_handle).child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    window.handle_input(&focus_handle, ElementInputHandler::new(bounds, view), cx);
                },
            )
            .size_full(),
        )
    }
}

impl EntityInputHandler for Caret {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.at..self.at,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(caret_bounds(element_bounds.origin, range.start))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

fn caret_bounds(origin: Point<Pixels>, at: usize) -> Bounds<Pixels> {
    Bounds::new(
        origin + point(px(10. * at as f32), px(0.)),
        size(px(2.), px(20.)),
    )
}

#[gpui::test]
fn the_platform_is_asked_again_where_the_caret_is(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, cx| Caret {
        focus_handle: cx.focus_handle(),
        at: 0,
    });
    let any = AnyWindowHandle::from(window);
    let frame = |cx: &mut TestAppContext| {
        cx.update_window(any, |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
    };
    window
        .update(cx, |caret, window, cx| {
            window.focus(&caret.focus_handle, cx);
        })
        .unwrap();
    frame(cx);
    // Moving the caret alone tells the platform nothing.
    window
        .update(cx, |caret, _, cx| {
            caret.at = 3;
            cx.notify();
        })
        .unwrap();
    frame(cx);
    assert_eq!(cx.ime_positions(any), Vec::new());

    window
        .update(cx, |_, window, _| window.invalidate_character_coordinates())
        .unwrap();
    frame(cx);
    window
        .update(cx, |caret, window, cx| {
            caret.at = 5;
            cx.notify();
            window.invalidate_character_coordinates();
        })
        .unwrap();
    frame(cx);
    let origin = Point::default();
    assert_eq!(
        cx.ime_positions(any),
        vec![caret_bounds(origin, 3), caret_bounds(origin, 5)]
    );
}
