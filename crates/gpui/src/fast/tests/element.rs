//! Tests of elements drawn again from what they drew on the last frame.

use crate::{
    Context, IntoElement, Render, SharedString, TestAppContext, Window, WindowHandle, div, hsla,
    prelude::*, px,
};

struct Rows {
    texts: Vec<SharedString>,
}

impl Render for Rows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().children(
            self.texts
                .iter()
                .cloned()
                .map(|text| div().h(px(20.)).bg(hsla(0.5, 0.5, 0.5, 1.)).child(text)),
        )
    }
}

#[gpui::test]
fn unchanged_elements_of_a_rendered_view_are_reused(cx: &mut TestAppContext) {
    let window: WindowHandle<Rows> = cx.add_window(|_, _| Rows {
        texts: vec!["a".into(), "b".into(), "c".into()],
    });
    // Rendered again as it was, until its elements are recorded: an element
    // nested in no other is recorded once it stands still.
    for _ in 0..3 {
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
    }
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.reset_layout_stats();
    })
    .unwrap();
    window
        .update(cx, |rows, _, cx| {
            rows.texts[1] = "B".into();
            cx.notify();
        })
        .unwrap();
    let stats = cx
        .update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            window.layout_stats()
        })
        .unwrap();
    // The two rows that did not change, a div and its text each, and the
    // changed row's div, whose text changed.
    assert_eq!(stats.elements_reused, 4, "{stats:?}");
}

struct Narrowing {
    width: f32,
}

impl Render for Narrowing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_start()
            .w(px(self.width))
            .child(div().child("x"))
    }
}

#[gpui::test]
fn text_measured_again_is_not_drawn_again(cx: &mut TestAppContext) {
    let window: WindowHandle<Narrowing> = cx.add_window(|_, _| Narrowing { width: 200. });
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.reset_layout_stats();
    })
    .unwrap();
    window
        .update(cx, |narrowing, _, cx| {
            narrowing.width = 150.;
            cx.notify();
        })
        .unwrap();
    let stats = cx
        .update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            window.layout_stats()
        })
        .unwrap();
    // The text keeps its size in a narrower column, but was measured again
    // under the narrower width, and could have broken into other lines.
    assert!(stats.measure_calls > 0, "{stats:?}");
    assert_eq!(stats.elements_reused, 0, "{stats:?}");
}

struct Outer {
    tick: usize,
    middle: crate::Entity<Middle>,
}

struct Middle {
    tick: usize,
    inner: crate::Entity<Inner>,
}

struct Inner {
    inner: crate::Entity<Leaf>,
}

struct Leaf {
    tick: usize,
}

impl Render for Outer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(10.))
                    .child(SharedString::from(self.tick.to_string())),
            )
            .child(self.middle.clone())
    }
}

impl Render for Middle {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(10.))
                    .child(SharedString::from(self.tick.to_string())),
            )
            .child(self.inner.clone())
    }
}

impl Render for Inner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .child(div().h(px(10.)).child("inner"))
            .child(self.inner.clone())
    }
}

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(SharedString::from(self.tick.to_string()))
    }
}

/// A view drawn from last frame carries the element records drawn inside it,
/// and those of the views nested in it; one of them drawn around a view
/// nested in it, which carries none, must not throw that off.
#[gpui::test]
fn a_view_drawn_again_carries_what_a_spliced_view_in_it_did_not(cx: &mut TestAppContext) {
    let window: WindowHandle<Outer> = cx.add_window(|_, cx| {
        let leaf = cx.new(|_| Leaf { tick: 0 });
        let inner = cx.new(|_| Inner { inner: leaf });
        Outer {
            tick: 0,
            middle: cx.new(|_| Middle { tick: 0, inner }),
        }
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap()
    };
    draw(cx);
    let (middle, leaf) = window
        .read_with(cx, |outer, cx| {
            let middle = outer.middle.clone();
            let leaf = middle.read(cx).inner.read(cx).inner.clone();
            (middle, leaf)
        })
        .unwrap();
    for _ in 0..3 {
        // The middle view is built, and the inner one drawn around the leaf.
        cx.update(|cx| {
            middle.update(cx, |middle, cx| {
                middle.tick += 1;
                cx.notify();
            });
            leaf.update(cx, |leaf, cx| {
                leaf.tick += 1;
                cx.notify();
            });
        });
        draw(cx);
        // The outer view is built, and the middle one drawn from last frame.
        window
            .update(cx, |outer, _, cx| {
                outer.tick += 1;
                cx.notify();
            })
            .unwrap();
        draw(cx);
    }
}

struct Buttons {
    labels: Vec<SharedString>,
}

impl Render for Buttons {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .children(
                self.labels
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, label)| {
                        div()
                            .flex()
                            .h(px(20.))
                            .child(div().w(px(80.)).child(label))
                            .child(
                                div()
                                    .id(index)
                                    .w(px(20.))
                                    .hover(|style| style.bg(hsla(0.5, 0.5, 0.5, 1.)))
                                    .child("x"),
                            )
                    }),
            )
    }
}

/// A plain row holding an interactive element is drawn as upstream draws it,
/// but it is kept where it was, so that its plain cells are drawn again on
/// their own rather than skipped as though the row had just appeared.
#[gpui::test]
fn plain_elements_beside_an_interactive_one_are_reused(cx: &mut TestAppContext) {
    let window: WindowHandle<Buttons> = cx.add_window(|_, _| Buttons {
        labels: vec!["a".into(), "b".into(), "c".into()],
    });
    for _ in 0..4 {
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }
    cx.update_window(window.into(), |_, window, _| window.reset_layout_stats())
        .unwrap();
    window
        .update(cx, |buttons, _, cx| {
            buttons.labels[1] = "B".into();
            cx.notify();
        })
        .unwrap();
    let stats = cx
        .update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            window.layout_stats()
        })
        .unwrap();
    // The label cells of the two rows that did not change, a div and its
    // text each, and the text in every button.
    assert_eq!(stats.elements_reused, 7, "{stats:?}");
}

const SQUARE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect x="1" y="1" width="6" height="6"/></svg>"#;
const DOT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><circle cx="4" cy="4" r="2"/></svg>"#;

struct Icon {
    data: &'static str,
    turned: bool,
}

impl Render for Icon {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let icon = crate::svg()
            .size(px(16.))
            .text_color(hsla(0., 0., 0., 1.))
            .data(self.data.as_bytes());
        div().child(if self.turned {
            icon.with_transformation(crate::Transformation::rotate(crate::radians(1.)))
        } else {
            icon
        })
    }
}

/// An svg is drawn again only while it shows the same image, turned the
/// same way.
#[gpui::test]
fn an_svg_showing_another_image_is_drawn_anew(cx: &mut TestAppContext) {
    let window: WindowHandle<Icon> = cx.add_window(|_, _| Icon {
        data: SQUARE,
        turned: false,
    });
    let sprite = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let stats = window.layout_stats();
            window.reset_layout_stats();
            let sprites = &window.rendered_frame.scene.monochrome_sprites;
            assert_eq!(sprites.len(), 1);
            (
                sprites[0].tile.tile_id,
                sprites[0].transformation,
                stats.elements_reused,
            )
        })
        .unwrap()
    };
    let (square, straight, _) = sprite(cx);
    for _ in 0..3 {
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        sprite(cx);
    }
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    assert_eq!(
        sprite(cx),
        (square, straight, 2),
        "drawn again, div and svg"
    );

    window
        .update(cx, |icon, _, cx| {
            icon.data = DOT;
            cx.notify();
        })
        .unwrap();
    let (dot, _, reused) = sprite(cx);
    assert_ne!(dot, square);
    assert_eq!(reused, 0);

    window
        .update(cx, |icon, _, cx| {
            icon.turned = true;
            cx.notify();
        })
        .unwrap();
    let (_, turned, reused) = sprite(cx);
    assert_ne!(turned, straight);
    assert_eq!(reused, 0);
}
