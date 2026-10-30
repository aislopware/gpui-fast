//! Tests of an input that comes while the app is borrowed (`fast/input_wait.rs`):
//! it waits for the borrow to end and is dispatched then, in the order the
//! inputs came, never dropped.

use std::{cell::RefCell, rc::Rc};

use crate::{
    self as gpui, Context, InteractiveElement, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    ParentElement, Pixels, PlatformInput, Render, Styled, TestAppContext, VisualTestContext,
    Window, div, point, px,
};

/// A window that notes the left edge of every press it hears.
struct Pads {
    heard: Rc<RefCell<Vec<Pixels>>>,
    /// A press at this left edge sends one more press at the given edge from
    /// inside its own dispatch, as UIKit sends an event from inside GPUI's work.
    echo: Option<(Pixels, Pixels, crate::TestWindow)>,
}

impl Render for Pads {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let heard = self.heard.clone();
        let echo = self.echo.clone();
        div()
            .id("pads")
            .size_full()
            .on_mouse_down(MouseButton::Left, move |event, _, _| {
                heard.borrow_mut().push(event.position.x);
                if let Some((on, at, window)) = &echo
                    && event.position.x == *on
                {
                    window.clone().simulate_input(press(*at));
                }
            })
            .child(div().size(px(400.)))
    }
}

fn press(x: Pixels) -> PlatformInput {
    PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: point(x, px(10.)),
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    })
}

fn pads(cx: &mut TestAppContext) -> (Rc<RefCell<Vec<Pixels>>>, &mut VisualTestContext) {
    let heard: Rc<RefCell<Vec<Pixels>>> = Rc::default();
    let held = heard.clone();
    let (_, cx) = cx.add_window_view(move |_, _| Pads {
        heard: held,
        echo: None,
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (heard, cx)
}

fn platform_window(cx: &mut VisualTestContext) -> crate::TestWindow {
    let window = cx.update(|window, _| window.window_handle());
    cx.test_window(window)
}

#[gpui::test]
fn an_input_that_comes_while_the_app_is_borrowed_waits_for_it(cx: &mut TestAppContext) {
    let (heard, cx) = pads(cx);
    let mut window = platform_window(cx);
    cx.update(|_, _| {
        window.simulate_input(press(px(10.)));
    });
    assert!(heard.borrow().is_empty(), "the press waits for the update");
    cx.run_until_parked();
    assert_eq!(*heard.borrow(), [px(10.)], "and is dispatched once it ends");
}

#[gpui::test]
fn inputs_after_one_that_waits_wait_behind_it(cx: &mut TestAppContext) {
    let (heard, cx) = pads(cx);
    let mut window = platform_window(cx);
    cx.update(|_, _| {
        window.simulate_input(press(px(10.)));
    });
    // The app is free now, but a press came before this one.
    window.simulate_input(press(px(20.)));
    assert!(heard.borrow().is_empty());
    cx.run_until_parked();
    assert_eq!(*heard.borrow(), [px(10.), px(20.)]);
    // With nothing waiting, a press goes at once again.
    window.simulate_input(press(px(30.)));
    assert_eq!(*heard.borrow(), [px(10.), px(20.), px(30.)]);
}

#[gpui::test]
fn an_input_sent_from_inside_a_waiting_ones_dispatch_goes_in_its_turn(cx: &mut TestAppContext) {
    let heard: Rc<RefCell<Vec<Pixels>>> = Rc::default();
    let held = heard.clone();
    let (pads, cx) = cx.add_window_view(move |_, _| Pads {
        heard: held,
        echo: None,
    });
    cx.run_until_parked();
    let mut window = platform_window(cx);
    let echoed = window.clone();
    pads.update(cx, |pads, cx| {
        pads.echo = Some((px(10.), px(30.), echoed));
        cx.notify();
    });
    cx.run_until_parked();
    // Two presses wait for an update; the first, dispatched, sends one more
    // from inside its dispatch, with the app borrowed again. It came after the
    // second, so it goes after it.
    cx.update(|_, _| {
        window.simulate_input(press(px(10.)));
        window.simulate_input(press(px(20.)));
    });
    cx.run_until_parked();
    assert_eq!(*heard.borrow(), [px(10.), px(20.), px(30.)]);
}
