//! What draws no pixel is no part of what a frame shows ([`crate::Window::painted_primitives`]):
//! a frame that paints text faded to nothing and one that skips it draw the same pixels.

use std::sync::Arc;

use super::element_oracle::GlyphBoxTextSystem;
use crate::{
    AnyWindowHandle, AppContext as _, Context, IntoElement, NoopTextSystem, ParentElement as _,
    Render, Styled as _, TestAppContext, Window, div, hsla, prelude::FluentBuilder as _, px,
};

/// A swatch, and beside it words faded to nothing, or none.
struct Swatch {
    faded_words: bool,
}

impl Render for Swatch {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(div().size(px(20.)).bg(hsla(0.3, 0.5, 0.5, 1.)))
            .when(self.faded_words, |row| {
                row.child(
                    div()
                        .text_color(hsla(0.6, 0.1, 0.2, 0.))
                        .child("Quiet actions"),
                )
            })
    }
}

#[test]
fn words_faded_to_nothing_paint_what_no_words_paint() {
    let mut cx = TestAppContext::with_text_system(Arc::new(GlyphBoxTextSystem(NoopTextSystem)));
    let mut painted = |faded_words: bool| {
        let window: AnyWindowHandle = cx.add_window(move |_, _| Swatch { faded_words }).into();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.painted_primitives()
        })
        .unwrap()
    };
    let (faded, none) = (painted(true), painted(false));
    assert_eq!(faded, none, "faded words drew pixels");
    assert!(!none.is_empty(), "the swatch draws");
}
