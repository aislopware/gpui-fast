//! `ListState::scroll_by` from an offset `scroll_to` allows but a scroll never makes: an item
//! held below the viewport's top by a negative `offset_in_item`, as a chat holds a prompt
//! under its titlebar. A nudge smaller than the gap lands before that item, and the seek must
//! not run backward.

use crate::{
    AppContext as _, Context, Element as _, IntoElement, ListAlignment, ListOffset, ListState,
    Render, Styled as _, TestAppContext, Window, div, list, point, px, size,
};

struct Rows(ListState);

impl Render for Rows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        list(self.0.clone(), |_, _, _| {
            div().h(px(20.)).w_full().into_any()
        })
        .w_full()
        .h_full()
    }
}

#[crate::test]
fn a_nudge_from_a_negative_offset_lands_in_the_item_above(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(5, ListAlignment::Top, px(10.));
    cx.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, cx| {
        cx.new(|_| Rows(state.clone())).into_any_element()
    });

    state.scroll_to(ListOffset {
        item_ix: 2,
        offset_in_item: px(-15.),
    });
    state.scroll_by(px(5.));

    let offset = state.logical_scroll_top();
    assert_eq!(offset.item_ix, 1);
    assert_eq!(offset.offset_in_item, px(10.));
}
