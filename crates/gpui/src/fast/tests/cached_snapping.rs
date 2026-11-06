//! A view drawn cached (`cached`) is laid out on its own and placed at its
//! bounds, snapped, as upstream's and monocode's are. Where those bounds lie
//! on whole device pixels it paints what it paints laid out in place, ties
//! included; where layout put it off a pixel's edge, what it holds snaps
//! from its snapped bounds, as monocode drew it, and may lie a device pixel
//! from where it lies in place.

use super::element_oracle::GlyphBoxTextSystem;
use crate::{
    Context, Entity, IntoElement, NoopTextSystem, Render, StyleRefinement, TestAppContext, Window,
    WindowHandle, div, hsla, prelude::*, px,
};
use std::sync::Arc;

/// A row a device pixel taller than the box it is centred in, so that it
/// starts half a device pixel above the box, as a terminal's sticky header
/// centres a line taller than itself.
struct Header;

impl Render for Header {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .h(px(8.))
                .flex()
                .items_center()
                .bg(hsla(0.6, 0.5, 0.5, 1.))
                .child(
                    div()
                        .flex_none()
                        .w(px(40.))
                        .h(px(8.5))
                        .bg(hsla(0.1, 0.8, 0.5, 1.))
                        .child("label"),
                ),
        )
    }
}

/// `header`, below a box `above` tall and centred in a column `column`
/// tall, drawn cached or in place.
struct Holder {
    header: Entity<Header>,
    cached: bool,
    above: f32,
    column: f32,
}

impl Render for Holder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let header = if self.cached {
            self.header
                .clone()
                .cached(StyleRefinement::default().w(px(100.)).h(px(20.)))
                .into_any_element()
        } else {
            div()
                .w(px(100.))
                .h(px(20.))
                .child(self.header.clone())
                .into_any_element()
        };
        div().size_full().child(div().h(px(self.above))).child(
            div()
                .h(px(self.column))
                .flex()
                .flex_col()
                .justify_center()
                .child(header),
        )
    }
}

/// What a window holding the header below `above`, in a column `column`
/// tall, paints with the header cached and in place.
fn painted(above: f32, column: f32) -> [Vec<String>; 2] {
    let mut cx = TestAppContext::with_text_system(Arc::new(GlyphBoxTextSystem(NoopTextSystem)));
    [true, false].map(|cached| {
        let window: WindowHandle<Holder> = cx.add_window(|_, cx| Holder {
            header: cx.new(|_| Header),
            cached,
            above,
            column,
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            window.painted_primitives()
        })
        .unwrap()
    })
}

/// The label starts half a device pixel above the header: in place that is
/// a positive place in the window, cached a negative one in the header's own
/// layout, and the two must snap alike.
#[test]
fn a_cached_view_snaps_what_sticks_out_of_it_as_in_place() {
    let [cached, in_place] = painted(10., 20.);
    assert_eq!(cached, in_place);
}

/// The header is centred half a device pixel off a pixel's edge: cached, it
/// is laid out from its bounds, snapped, as monocode's cached views are, so
/// what it holds lies where it lies in place moved by the half pixel its
/// bounds snapped away; drawn twice cached, it draws alike.
#[test]
fn a_cached_view_placed_off_a_pixels_edge_snaps_from_its_snapped_bounds() {
    let [cached, in_place] = painted(10., 20.5);
    assert_ne!(cached, in_place, "cached, the header snaps from its bounds");
    let [again, _] = painted(10., 20.5);
    assert_eq!(cached, again);
    // A whole device pixel lower, the column's half pixel is gone: cached
    // and in place agree.
    let [cached, in_place] = painted(10., 21.);
    assert_eq!(cached, in_place);
}
