//! A live region reaches the accessibility tree as one.

use crate::{
    AnyWindowHandle, AppContext as _, Context, InteractiveElement as _, IntoElement,
    LiveRegion as _, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, Window, div, px,
};

struct Announcing;

impl Render for Announcing {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("root")
            .role(accesskit::Role::Group)
            .size_full()
            .child(
                div()
                    .id("count")
                    .role(accesskit::Role::Status)
                    .aria_live(accesskit::Live::Polite)
                    .aria_value("3 new")
                    .w(px(40.))
                    .h(px(20.)),
            )
    }
}

/// The region carries its politeness and the value it announces; an element
/// that does not ask to be one stays quiet.
#[test]
fn a_live_region_carries_its_politeness_and_value() {
    let mut cx = TestAppContext::single();
    let window: AnyWindowHandle = cx.add_window(|_, _| Announcing).into();
    cx.update_window(window, |_, window, cx| {
        window.set_a11y_active(true);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let (live, value, quiet) = cx
        .update_window(window, |_, window, _| {
            let tree = window.a11y_tree().expect("a tree after an active frame");
            let node = |role| {
                tree.nodes
                    .iter()
                    .find(|(_, node)| node.role() == role)
                    .map(|(_, node)| node.clone())
                    .expect("the node")
            };
            let status = node(accesskit::Role::Status);
            let group = node(accesskit::Role::Group);
            (
                status.live(),
                status.value().map(str::to_owned),
                group.live(),
            )
        })
        .unwrap();
    assert_eq!(live, Some(accesskit::Live::Polite));
    assert_eq!(value.as_deref(), Some("3 new"));
    assert_eq!(quiet, None, "no region unless asked");
}
