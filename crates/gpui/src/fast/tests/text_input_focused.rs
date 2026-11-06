//! Whether the focused element is a text field ([`Window::text_input_focused`]):
//! a field whose input handler takes text is one, a button or a pane focused
//! is not, the answer is what the platform was last told, and a view drawn
//! from its last frame that asks is told as it changes.

use crate::{
    AnyWindowHandle, AppContext as _, Bounds, Context, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Render, StyleRefinement, Styled as _, TestAppContext, UTF16Selection, Window, canvas,
    div,
};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

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
/// focused after it is none once a frame has found it, the platform keeping
/// its keyboard up until then.
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
        assert!(window.text_input_focused(), "a button, before any frame");
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

/// The focus moved from one field to another between frames, as a tap from
/// one terminal's pane to another's moves it: the platform's keyboard stays
/// up, and every frame, the one between included, is told a text field is
/// focused, so a layout keeping the keyboard's band clear never lets it go.
#[test]
fn the_focus_moved_between_two_fields_keeps_a_text_field_focused() {
    let mut cx = TestAppContext::single();
    let first = cx.new(|cx| Field {
        focus_handle: cx.focus_handle(),
    });
    let second = cx.new(|cx| Field {
        focus_handle: cx.focus_handle(),
    });
    let seen = Rc::new(RefCell::new(Vec::new()));
    let reader = cx.new(|_| Recorder { seen: seen.clone() });
    let window = cx.add_window({
        let (first, second, reader) = (first.clone(), second.clone(), reader.clone());
        move |_, _| TwoFields {
            first,
            second,
            reader,
        }
    });
    let window = AnyWindowHandle::from(window);
    let focus_of = |field: &Entity<Field>, cx: &mut TestAppContext| {
        field.read_with(cx, |field, _| field.focus_handle.clone())
    };
    let (first_focus, second_focus) = (focus_of(&first, &mut cx), focus_of(&second, &mut cx));
    // Each frame builds the recorder again, so it says what the frame was told.
    let frames = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            reader.update(cx, |_, cx| cx.notify());
            cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }
    };
    cx.update_window(window, |_, window, cx| window.focus(&first_focus, cx))
        .unwrap();
    frames(&mut cx);
    assert_eq!(seen.borrow().last(), Some(&true), "the first field");
    seen.borrow_mut().clear();

    cx.update_window(window, |_, window, cx| {
        window.focus(&second_focus, cx);
        assert!(
            window.text_input_focused(),
            "the second field, before any frame"
        );
    })
    .unwrap();
    frames(&mut cx);
    assert_eq!(*seen.borrow(), [true; 3], "every frame after the move");
    let focused = cx
        .update_window(window, |_, window, _| {
            second_focus.is_focused(window) && window.text_input_focused()
        })
        .unwrap();
    assert!(focused, "the second field holds the focus and takes text");
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

/// Keeps every answer it read of the text field, one each time it is built.
struct Recorder {
    seen: Rc<RefCell<Vec<bool>>>,
}

impl Render for Recorder {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.seen.borrow_mut().push(window.text_input_focused());
        div().size_full()
    }
}

/// Two fields and a recorder, each cached.
struct TwoFields {
    first: Entity<Field>,
    second: Entity<Field>,
    reader: Entity<Recorder>,
}

impl Render for TwoFields {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let cached = StyleRefinement::default().size_full();
        div()
            .size_full()
            .child(self.first.clone().cached(cached.clone()))
            .child(self.second.clone().cached(cached.clone()))
            .child(self.reader.clone().cached(cached))
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
