//! Whether the focused element is a text field ([`Window::text_input_focused`]):
//! a field whose input handler takes text is one, a button or a pane focused
//! is not, and a view drawn from its last frame that asks is told as it
//! changes.

use crate::{
    AnyWindowHandle, AppContext as _, Bounds, Context, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Render, StyleRefinement, Styled as _, TestAppContext, UTF16Selection, Window, canvas,
    div,
};
use std::{cell::Cell, ops::Range, rc::Rc};

/// A field that takes text, its input handler registered as it paints.
struct Field {
    focus_handle: FocusHandle,
}

impl Render for Field {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

impl EntityInputHandler for Field {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: crate::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// The field focused is a text field until the focus leaves it; a button
/// focused after it is none, before any frame and after one.
#[test]
fn a_focused_element_without_a_text_input_is_no_text_field() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, cx| Field {
        focus_handle: cx.focus_handle(),
    });
    let view = window.root(&mut cx).unwrap();
    let window = AnyWindowHandle::from(window);
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        assert!(!window.text_input_focused(), "nothing focused");
        let field = view.read(cx).focus_handle.clone();
        window.focus(&field, cx);
        window.draw(cx).clear(cx);
        assert!(window.text_input_focused(), "the field");
        let button = cx.focus_handle();
        window.focus(&button, cx);
        assert!(!window.text_input_focused(), "a button, before any frame");
        window.draw(cx).clear(cx);
        assert!(!window.text_input_focused(), "a button");
        window.blur(cx);
        window.draw(cx).clear(cx);
        assert!(!window.text_input_focused(), "nothing focused again");
    })
    .unwrap();
}

/// A view that asks whether a text field is focused, drawn from its last
/// frame otherwise, learns it once the frame that found the field focused is
/// drawn, and learns it went once the focus moves to a button.
#[test]
fn a_view_asking_for_the_text_field_is_told_as_it_changes() {
    let mut cx = TestAppContext::single();
    let field = cx.new(|cx| Field {
        focus_handle: cx.focus_handle(),
    });
    let seen = Rc::new(Cell::new(None));
    let reader = cx.new(|_| Reader { seen: seen.clone() });
    let window = cx.add_window({
        let field = field.clone();
        move |_, _| Host { field, reader }
    });
    let window = AnyWindowHandle::from(window);
    let field_focus = field.read_with(&cx, |field, _| field.focus_handle.clone());
    let frames = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }
    };
    frames(&mut cx);
    assert_eq!(seen.get(), Some(false), "nothing focused");
    cx.update_window(window, |_, window, cx| window.focus(&field_focus, cx))
        .unwrap();
    frames(&mut cx);
    assert_eq!(seen.get(), Some(true), "the field");
    let _button = cx
        .update_window(window, |_, window, cx| {
            let button = cx.focus_handle();
            window.focus(&button, cx);
            button
        })
        .unwrap();
    frames(&mut cx);
    assert_eq!(seen.get(), Some(false), "a button");
}

/// Says what it read of the text field each time it is built.
struct Reader {
    seen: Rc<Cell<Option<bool>>>,
}

impl Render for Reader {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.seen.set(Some(window.text_input_focused()));
        div().size_full()
    }
}

/// The field and the reader, each cached.
struct Host {
    field: Entity<Field>,
    reader: Entity<Reader>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                self.field
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .child(
                self.reader
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
    }
}
