//! A node's bounds in the accessibility tree are the part of it that is drawn.

use std::{cell::RefCell, rc::Rc};

use crate::{
    AnyWindowHandle, AppContext as _, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, TestAppContext,
    Window, div, px,
};

/// A 100 px scroll container holding three 60 px buttons: the first shows whole, the second
/// is cut by the container's edge, the third is scrolled out of view.
struct Scrolled {
    pressed: Rc<RefCell<Vec<&'static str>>>,
}

impl Render for Scrolled {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let button = |label: &'static str| {
            let pressed = self.pressed.clone();
            div()
                .id(label)
                .role(accesskit::Role::Button)
                .aria_label(label)
                .w(px(100.))
                .h(px(60.))
                .flex_none()
                .on_click(move |_, _, _| pressed.borrow_mut().push(label))
        };
        div()
            .id("root")
            .role(accesskit::Role::Group)
            .size_full()
            .child(
                div()
                    .id("scroller")
                    .role(accesskit::Role::ScrollView)
                    .w(px(100.))
                    .h(px(100.))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .child(button("whole"))
                    .child(button("cut"))
                    .child(button("hidden")),
            )
    }
}

/// The bounds a button reports, in device pixels as top and bottom, and its node.
fn button(window: &Window, label: &str) -> (f64, f64, accesskit::NodeId) {
    let tree = window.a11y_tree().expect("a tree after an active frame");
    let (id, node) = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label))
        .unwrap_or_else(|| panic!("{label} has a node"));
    let bounds = node.bounds().expect("bounds");
    (bounds.y0, bounds.y1, *id)
}

/// A button the scroll container cuts reports only what shows of it, and one scrolled out of
/// view reports nothing. Pressing it through assistive technology presses what shows of it,
/// and one that does not show is not pressed: upstream pressed the middle of its whole bounds,
/// wherever that was and whatever was drawn there.
#[test]
fn a_node_reports_and_is_pressed_where_it_shows() {
    let mut cx = TestAppContext::single();
    let pressed = Rc::new(RefCell::new(Vec::new()));
    let window: AnyWindowHandle = cx
        .add_window({
            let pressed = pressed.clone();
            move |_, _| Scrolled { pressed }
        })
        .into();
    cx.update_window(window, |_, window, cx| {
        window.set_a11y_active(true);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let (whole, cut, hidden, scale) = cx
        .update_window(window, |_, window, _| {
            (
                button(window, "whole"),
                button(window, "cut"),
                button(window, "hidden"),
                f64::from(window.scale_factor()),
            )
        })
        .unwrap();
    assert_eq!((whole.0, whole.1), (0., 60. * scale), "whole");
    assert_eq!(
        (cut.0, cut.1),
        (60. * scale, 100. * scale),
        "cut at the edge"
    );
    assert_eq!(hidden.0, hidden.1, "nothing of it shows");

    let press = |cx: &mut TestAppContext, node| {
        cx.update_window(window, |_, window, cx| {
            window.handle_a11y_action(
                accesskit::ActionRequest {
                    action: accesskit::Action::Click,
                    target_tree: accesskit::TreeId::ROOT,
                    target_node: node,
                    data: None,
                },
                cx,
            );
        })
        .unwrap();
    };
    press(&mut cx, hidden.2);
    press(&mut cx, cut.2);
    assert_eq!(*pressed.borrow(), ["cut"]);
}
