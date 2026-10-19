//! A tooltip fades out over [`EXIT`] once its element lets it go, where it
//! stood, and vanishes at once under reduced motion. See
//! [`crate::fast::tooltip_exit`].

use crate::fast::tooltip_exit::EXIT;
use crate::{
    AnyWindowHandle, AppContext as _, Context, DEFAULT_TOOLTIP_SHOW_DELAY, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, div, hsla, point, px,
};

const TIP: Hsla = hsla(0.8, 1., 0.5, 1.);

struct Tip;

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(60.)).h(px(16.)).bg(TIP)
    }
}

/// A 40×20 button at (100, 50) with a tooltip.
struct Button;

impl Render for Button {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("button")
                .absolute()
                .left(px(100.))
                .top(px(50.))
                .w(px(40.))
                .h(px(20.))
                .tooltip(|_, cx| cx.new(|_| Tip).into()),
        )
    }
}

struct Setup {
    cx: TestAppContext,
    window: AnyWindowHandle,
}

fn setup() -> Setup {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, _| Button).into();
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

    fn wait(&mut self, duration: std::time::Duration) {
        self.cx.executor().advance_clock(duration);
        self.draw();
    }

    /// The tooltip drawn, if one is: where it stands and how opaque it is.
    fn tooltip(&mut self) -> Option<(Pixels, f32)> {
        self.cx
            .update_window(self.window, |_, window, _| {
                let scale = window.scale_factor();
                window
                    .rendered_frame
                    .scene
                    .quads
                    .iter()
                    .find(|quad| {
                        let color = quad.background.solid;
                        (color.h, color.s, color.l) == (TIP.h, TIP.s, TIP.l)
                    })
                    .map(|quad| (quad.bounds.origin.x / scale, quad.background.solid.a))
            })
            .unwrap()
    }

    fn shown(&mut self) -> (Pixels, f32) {
        self.move_mouse(110., 55.);
        self.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
        let shown = self.tooltip().expect("shown");
        assert_eq!(shown.1, 1.);
        shown
    }
}

#[test]
fn a_tooltip_fades_out_where_it_stood_once_the_pointer_leaves() {
    let mut s = setup();
    let (x, _) = s.shown();
    s.move_mouse(5., 5.);
    let (left_at, opacity) = s.tooltip().expect("still drawn as the pointer leaves");
    assert_eq!(left_at, x, "where it stood, not under the pointer");
    assert!(opacity <= 1.);
    s.wait(EXIT / 2);
    let (_, half) = s.tooltip().expect("halfway out");
    assert!(half > 0. && half < 1., "fading: {half}");
    s.wait(EXIT);
    assert_eq!(s.tooltip(), None, "gone once the fade ends");
}

#[test]
fn under_reduced_motion_a_tooltip_vanishes_at_once() {
    let mut s = setup();
    s.cx.update(|cx| cx.set_reduce_motion(true));
    s.shown();
    s.move_mouse(5., 5.);
    assert_eq!(s.tooltip(), None);
}
