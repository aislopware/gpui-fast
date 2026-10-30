//! Tests of what a platform tells a window while the app is borrowed
//! (`fast/window_wait.rs`): it waits for the borrow to end and is heard then, in
//! the order it came, never dropped.

use std::{cell::RefCell, rc::Rc};

use crate::{
    self as gpui, Context, InteractiveElement, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    ParentElement, Pixels, PlatformInput, Render, Size, Styled, TestAppContext, VisualTestContext,
    Window, div, point, px, size,
};

/// What a window heard, in order.
type Heard = Rc<RefCell<Vec<String>>>;

/// A window that notes every press it hears, by its left edge, and every
/// change of its bounds, by its size.
struct Pads {
    heard: Heard,
    /// A press at this left edge sends one more press at the given edge from
    /// inside its own dispatch, as a platform sends input from inside GPUI's work.
    echo: Option<(Pixels, Pixels, crate::TestWindow)>,
    _bounds: gpui::Subscription,
}

impl Render for Pads {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let heard = self.heard.clone();
        let echo = self.echo.clone();
        div()
            .id("pads")
            .size_full()
            .on_mouse_down(MouseButton::Left, move |event, _, _| {
                heard
                    .borrow_mut()
                    .push(format!("press {}", f32::from(event.position.x)));
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

fn pads(cx: &mut TestAppContext) -> (Heard, gpui::Entity<Pads>, &mut VisualTestContext) {
    let heard: Heard = Rc::default();
    let held = heard.clone();
    let (pads, cx) = cx.add_window_view(move |window, cx| Pads {
        heard: held.clone(),
        echo: None,
        _bounds: cx.observe_window_bounds(window, move |_, window, _| {
            let Size { width, height } = window.viewport_size();
            held.borrow_mut()
                .push(format!("bounds {}x{}", f32::from(width), f32::from(height)));
        }),
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    heard.borrow_mut().clear();
    (heard, pads, cx)
}

fn platform_window(cx: &mut VisualTestContext) -> crate::TestWindow {
    let window = cx.update(|window, _| window.window_handle());
    cx.test_window(window)
}

#[gpui::test]
fn an_input_that_comes_while_the_app_is_borrowed_waits_for_it(cx: &mut TestAppContext) {
    let (heard, _, cx) = pads(cx);
    let mut window = platform_window(cx);
    cx.update(|_, _| {
        window.simulate_input(press(px(10.)));
    });
    assert!(heard.borrow().is_empty(), "the press waits for the update");
    cx.run_until_parked();
    assert_eq!(*heard.borrow(), ["press 10"], "and is heard once it ends");
}

#[gpui::test]
fn inputs_after_one_that_waits_wait_behind_it(cx: &mut TestAppContext) {
    let (heard, _, cx) = pads(cx);
    let mut window = platform_window(cx);
    cx.update(|_, _| {
        window.simulate_input(press(px(10.)));
    });
    // The app is free now, but a press came before this one.
    window.simulate_input(press(px(20.)));
    assert!(heard.borrow().is_empty());
    cx.run_until_parked();
    assert_eq!(*heard.borrow(), ["press 10", "press 20"]);
    // With nothing waiting, a press goes at once again.
    window.simulate_input(press(px(30.)));
    assert_eq!(*heard.borrow(), ["press 10", "press 20", "press 30"]);
}

#[gpui::test]
fn an_input_sent_from_inside_a_waiting_ones_dispatch_goes_in_its_turn(cx: &mut TestAppContext) {
    let (heard, pads, cx) = pads(cx);
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
    assert_eq!(*heard.borrow(), ["press 10", "press 20", "press 30"]);
}

#[gpui::test]
fn a_resize_that_comes_while_the_app_is_borrowed_reaches_the_window(cx: &mut TestAppContext) {
    let (heard, _, cx) = pads(cx);
    let mut window = platform_window(cx);
    let resized = size(px(321.), px(123.));
    // As UIKit lays GPUI's view out while GPUI's present commits, inside the draw.
    cx.update(|_, _| window.simulate_resize(resized));
    cx.run_until_parked();
    assert_eq!(cx.update(|window, _| window.viewport_size()), resized);
    assert_eq!(*heard.borrow(), ["bounds 321x123"]);
}

#[gpui::test]
fn a_resize_and_a_press_that_wait_are_heard_in_the_order_they_came(cx: &mut TestAppContext) {
    let (heard, _, cx) = pads(cx);
    let mut window = platform_window(cx);
    cx.update(|_, _| {
        window.simulate_input(press(px(10.)));
        window.simulate_resize(size(px(300.), px(200.)));
        window.simulate_input(press(px(20.)));
    });
    cx.run_until_parked();
    assert_eq!(*heard.borrow(), ["press 10", "bounds 300x200", "press 20"]);
}
