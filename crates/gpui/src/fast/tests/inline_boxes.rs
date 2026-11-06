//! Tests of boxes laid out in a line of text in place of a range of it: a formula or an image
//! inside a paragraph. See [`crate::LineLayout::with_inline_boxes`] and
//! [`crate::StyledText::with_inline_boxes`].

use std::cell::RefCell;
use std::rc::Rc;

use crate::{
    AppContext as _, Context, InlineBox, IntoElement, ParentElement as _, Pixels, Render,
    Styled as _, StyledText, TestAppContext, TextLayout, Window, div, px,
};

/// A paragraph whose formula is a box, `width` wide, in a column `column` wide.
struct Paragraph {
    text: &'static str,
    formula: std::ops::Range<usize>,
    width: Pixels,
    column: Pixels,
    boxed: bool,
    layout: Rc<RefCell<Option<TextLayout>>>,
}

impl Render for Paragraph {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut text = StyledText::new(self.text);
        if self.boxed {
            text = text.with_inline_boxes([InlineBox {
                range: self.formula.clone(),
                width: self.width,
            }]);
        }
        *self.layout.borrow_mut() = Some(text.layout().clone());
        div()
            .w(self.column)
            .text_size(px(16.))
            .line_height(px(24.))
            .child(text)
    }
}

/// The paragraph laid out and drawn, and its layout.
fn drawn(
    cx: &mut TestAppContext,
    text: &'static str,
    formula: std::ops::Range<usize>,
    width: Pixels,
    column: Pixels,
    boxed: bool,
) -> TextLayout {
    let layout: Rc<RefCell<Option<TextLayout>>> = Rc::default();
    let held = Rc::clone(&layout);
    let window = cx.add_window(move |_, _| Paragraph {
        text,
        formula,
        width,
        column,
        boxed,
        layout: held,
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    layout.borrow().clone().expect("rendered")
}

/// The box takes its text's place at its own width, on the text's baseline; the line keeps its
/// height; the text keeps its bytes, which is what is copied and read aloud.
#[test]
fn a_box_takes_its_texts_place_on_the_baseline() {
    let mut cx = TestAppContext::single();
    let text = "so $\\frac{a}{b}$ is";
    let formula = 3..16;
    let plain = drawn(&mut cx, text, formula.clone(), px(30.), px(800.), false);
    let boxed = drawn(&mut cx, text, formula.clone(), px(30.), px(800.), true);

    let start = boxed.position_for_index(formula.start).unwrap();
    let end = boxed.position_for_index(formula.end).unwrap();
    assert!(
        near(end.x - start.x, px(30.)),
        "the box is as wide as it says"
    );
    assert_eq!(start, plain.position_for_index(formula.start).unwrap());
    let shift = plain.position_for_index(formula.end).unwrap().x - end.x;
    assert!(
        near(
            plain.position_for_index(text.len()).unwrap().x - shift,
            boxed.position_for_index(text.len()).unwrap().x
        ),
        "the text after moves by the difference"
    );

    assert_eq!(
        boxed.bounds().size.height,
        px(24.),
        "one line, its height kept"
    );
    assert_eq!(boxed.bounds().size.height, plain.bounds().size.height);
    let baseline = boxed.baseline_for_index(formula.start).unwrap();
    let line = boxed.line_layout_for_index(formula.start).unwrap();
    let top = start.y;
    assert_eq!(baseline.x, start.x);
    assert_eq!(
        baseline.y - top,
        (px(24.) - line.ascent() - line.descent()) / 2. + line.ascent(),
        "the baseline the line's glyphs sit on"
    );
    assert_eq!(boxed.text(), text, "the text keeps its bytes");
}

/// A narrow column wraps before or after the box, never inside it, though its text has
/// spaces.
#[test]
fn a_paragraph_wraps_around_a_box_not_inside_it() {
    let mut cx = TestAppContext::single();
    let text = "where $a + b + c$ holds for all";
    let formula = 6..17;
    let mut rows = Vec::new();
    for column in [60., 90., 120., 200.] {
        let layout = drawn(&mut cx, text, formula.clone(), px(40.), px(column), true);
        // Where the box is drawn: its blank glyph's place on the baseline, which is on the
        // line it starts, and which ends 40 px on, on the same line, unless the line breaks
        // right after it.
        let start = layout.baseline_for_index(formula.start).unwrap();
        let end = layout.baseline_for_index(formula.end).unwrap();
        assert!(
            (end.y == start.y && near(end.x - start.x, px(40.))) || end.x == px(0.),
            "column {column}: box from {start:?} to {end:?}"
        );
        for inside in formula.start + 1..formula.end {
            let at = layout.baseline_for_index(inside).unwrap();
            assert!(
                at == end,
                "column {column}: byte {inside} of the box is not where the box ends: {at:?}"
            );
        }
        rows.push(start.y);
    }
    assert!(
        rows.first() > rows.last(),
        "the narrowest column put the box on a later line: {rows:?}"
    );
}

/// Pixels equal but for float rounding.
fn near(a: Pixels, b: Pixels) -> bool {
    (a - b).abs() < px(0.01)
}
