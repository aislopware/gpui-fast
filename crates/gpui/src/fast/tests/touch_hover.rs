//! A finger on a touch screen hovers nothing: hover listeners hear no hover from a touch, as
//! hover styles already show none, while a mouse resting there still hovers.

use std::{cell::RefCell, rc::Rc};

use crate::{
    AnyWindowHandle, AppContext as _, Context, InputEvent as _, InteractiveElement as _,
    IntoElement, Modifiers, MouseMoveEvent, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, TouchEvent, TouchId, TouchPhase,
    Window, div, point, px,
};

struct Row {
    hovers: Rc<RefCell<Vec<bool>>>,
}

impl Render for Row {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let hovers = self.hovers.clone();
        // Clear of the window's corner, where the pointer starts.
        div().size_full().pt(px(50.)).child(
            div()
                .id("row")
                .w(px(100.))
                .h(px(40.))
                .on_hover(move |hovered, _, _| hovers.borrow_mut().push(*hovered)),
        )
    }
}

/// A finger lifted from a row leaves no hover on it, as a mouse resting there does.
#[test]
fn a_touch_leaves_no_hover() {
    let mut cx = TestAppContext::single();
    let hovers: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let window: AnyWindowHandle = cx
        .add_window({
            let hovers = hovers.clone();
            move |_, _| Row { hovers }
        })
        .into();
    let draw = |cx: &mut TestAppContext| {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    };
    draw(&mut cx);
    let at = point(px(20.), px(70.));
    for phase in [TouchPhase::Started, TouchPhase::Ended] {
        let touch = TouchEvent {
            id: TouchId(1),
            phase,
            position: at,
            predicted_position: None,
            force: None,
        };
        cx.update_window(window, |_, window, cx| {
            window.dispatch_event(touch.to_platform_input(), cx);
        })
        .unwrap();
        draw(&mut cx);
    }
    assert!(!hovers.borrow().contains(&true), "{:?}", hovers.borrow());
    let moved = MouseMoveEvent {
        position: at,
        pressed_button: None,
        modifiers: Modifiers::default(),
    };
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(moved.to_platform_input(), cx);
    })
    .unwrap();
    draw(&mut cx);
    assert_eq!(
        hovers.borrow().last(),
        Some(&true),
        "a mouse resting there hovers it"
    );
}
