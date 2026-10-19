//! A press puts a tooltip away until the pointer leaves its element: the
//! shown one hides, a waiting one never shows its old words, and moving over
//! the element again shows nothing until the pointer has left and come back.
//! See [`crate::fast::tooltip_press`].

use crate::{
    AnyWindowHandle, AppContext as _, Context, DEFAULT_TOOLTIP_SHOW_DELAY, Hsla, InputEvent as _,
    InteractiveElement as _, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    ParentElement as _, Pixels, Render, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, Window, div, hsla, point, px,
};
use std::time::Duration;

const TIP: Hsla = hsla(0.8, 1., 0.5, 1.);

/// A tooltip as wide as its words are long.
struct Tip(Pixels);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(self.0).h(px(16.)).bg(TIP)
    }
}

/// A 40×20 toggle at (100, 50) whose tooltip names what a click does next:
/// 60 px wide while off, 90 px once on.
struct Toggle {
    on: bool,
}

impl Render for Toggle {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let words = if self.on { px(90.) } else { px(60.) };
        div().size_full().child(
            div()
                .id("toggle")
                .absolute()
                .left(px(100.))
                .top(px(50.))
                .w(px(40.))
                .h(px(20.))
                .on_click(cx.listener(|toggle, _, _, cx| {
                    toggle.on = !toggle.on;
                    cx.notify();
                }))
                .tooltip(move |_, cx| cx.new(|_| Tip(words)).into()),
        )
    }
}

struct Setup {
    cx: TestAppContext,
    window: AnyWindowHandle,
}

fn setup() -> Setup {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, _| Toggle { on: false }).into();
    let mut setup = Setup { cx, window };
    setup.draw();
    setup
}

impl Setup {
    fn draw(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    fn move_mouse(&mut self, x: f32, y: f32) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.simulate_mouse_move(point(px(x), px(y)), cx);
            })
            .unwrap();
        self.draw();
    }

    /// A left click where the pointer is, with no move between down and up.
    fn click(&mut self) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                let position = window.mouse_position();
                let (button, modifiers) = (MouseButton::Left, Modifiers::default());
                window.dispatch_event(
                    MouseDownEvent {
                        button,
                        position,
                        modifiers,
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                window.dispatch_event(
                    MouseUpEvent {
                        button,
                        position,
                        modifiers,
                        click_count: 1,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
        self.draw();
    }

    fn wait(&mut self, duration: Duration) {
        self.cx.executor().advance_clock(duration);
        self.draw();
    }

    /// The width of the tooltip drawn, if one is.
    fn tooltip(&mut self) -> Option<f32> {
        self.cx
            .update_window(self.window, |_, window, _| {
                let scale = window.scale_factor();
                window
                    .rendered_frame
                    .scene
                    .quads
                    .iter()
                    .find(|quad| quad.background.solid == TIP)
                    .map(|quad| quad.bounds.size.width.0 / scale)
            })
            .unwrap()
    }
}

#[test]
fn a_press_hides_the_tooltip_until_the_pointer_leaves() {
    let mut s = setup();
    s.move_mouse(110., 55.);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert_eq!(s.tooltip(), Some(60.));

    s.click();
    s.wait(crate::fast::tooltip_exit::EXIT); // it fades out first
    assert_eq!(s.tooltip(), None, "the press hides it");
    s.move_mouse(120., 60.);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY * 2);
    assert_eq!(s.tooltip(), None, "moving over the element shows nothing");

    s.move_mouse(5., 5.);
    s.wait(Duration::from_secs(1));
    s.move_mouse(110., 55.);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert_eq!(s.tooltip(), Some(90.), "back over it: the new words");
}

/// A press while the tooltip waits out its delay cancels it, rather than
/// letting it show the words from before the click.
#[test]
fn a_press_cancels_a_waiting_tooltip() {
    let mut s = setup();
    s.move_mouse(110., 55.);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY / 2);
    s.click();
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY * 2);
    assert_eq!(s.tooltip(), None);
}
