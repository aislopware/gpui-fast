//! A hoverable tooltip stays while the pointer travels to it and goes once the
//! pointer turns away. See [`crate::fast::hover_intent`].

use crate::{
    AnyWindowHandle, AppContext as _, Bounds, Context, DEFAULT_TOOLTIP_SHOW_DELAY, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, div, hsla, point, px,
    size,
};
use std::time::Duration;

const TIP: Hsla = hsla(0.3, 1., 0.5, 1.);

struct Tip;

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(120.)).h(px(40.)).bg(TIP)
    }
}

/// One 40×20 control at 100, 50 whose tooltip is hoverable.
struct Control;

impl Render for Control {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("control")
                .absolute()
                .left(px(100.))
                .top(px(50.))
                .w(px(40.))
                .h(px(20.))
                .hoverable_tooltip(|_, cx| cx.new(|_| Tip).into()),
        )
    }
}

struct Setup {
    cx: TestAppContext,
    window: AnyWindowHandle,
}

impl Setup {
    fn new() -> Self {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, _| Control).into();
        let mut setup = Self { cx, window };
        setup.draw();
        setup
    }

    fn draw(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    fn move_mouse(&mut self, at: crate::Point<Pixels>) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.simulate_mouse_move(at, cx)
            })
            .unwrap();
        self.draw();
    }

    fn wait(&mut self, duration: Duration) {
        self.cx.executor().advance_clock(duration);
        self.draw();
    }

    /// Where the tooltip is drawn at full strength, if it is.
    fn tooltip(&mut self) -> Option<Bounds<Pixels>> {
        self.cx
            .update_window(self.window, |_, window, _| {
                let scale = window.scale_factor();
                window
                    .rendered_frame
                    .scene
                    .quads
                    .iter()
                    .find(|quad| quad.background.solid == TIP)
                    .map(|quad| {
                        let (origin, extent) = (quad.bounds.origin, quad.bounds.size);
                        Bounds::new(
                            point(px(origin.x.0 / scale), px(origin.y.0 / scale)),
                            size(px(extent.width.0 / scale), px(extent.height.0 / scale)),
                        )
                    })
            })
            .unwrap()
    }

    /// Hovers the control's top right corner until its tooltip shows there,
    /// to the control's right and below it, then moves to the control's bottom
    /// left corner, still over it, and returns where the tooltip is.
    fn show(&mut self) -> Bounds<Pixels> {
        self.move_mouse(point(px(138.), px(52.)));
        self.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
        let tooltip = self.tooltip().expect("shown after the delay");
        assert!(
            tooltip.left() >= px(138.),
            "right of the control: {tooltip:?}"
        );
        self.move_mouse(point(px(101.), px(69.)));
        assert!(self.tooltip().is_some());
        tooltip
    }
}

/// Upstream would keep it 500 ms whichever way the pointer went; turned away,
/// it goes at the next move.
#[test]
fn turning_away_hides_it_at_once() {
    let mut s = Setup::new();
    s.show();
    s.move_mouse(point(px(101.), px(72.)));
    s.move_mouse(point(px(90.), px(85.)));
    s.wait(Duration::from_millis(100));
    assert_eq!(s.tooltip(), None);
}

/// A trip to the tooltip that takes longer than the 500 ms in all keeps it,
/// since each move toward it starts the wait again; on it, it stays.
#[test]
fn a_slow_trip_to_it_keeps_it() {
    let mut s = Setup::new();
    let tooltip = s.show();
    let start = point(px(101.), px(71.));
    let end = tooltip.center();
    let mut step = 0.;
    loop {
        let at = point(
            start.x + (end.x - start.x) * step,
            start.y + (end.y - start.y) * step,
        );
        if tooltip.contains(&at) {
            break;
        }
        s.move_mouse(at);
        s.wait(Duration::from_millis(300));
        assert!(s.tooltip().is_some(), "hidden {step} of the way");
        step += 0.1;
    }
    assert!(
        step >= 0.3,
        "the trip outside took at least three waits of 300 ms"
    );
    s.move_mouse(end);
    s.wait(Duration::from_secs(2));
    assert!(s.tooltip().is_some(), "kept while hovered");
}

/// Resting on the way runs the wait out.
#[test]
fn resting_on_the_way_hides_it_after_the_wait() {
    let mut s = Setup::new();
    s.show();
    s.move_mouse(point(px(101.), px(71.)));
    s.move_mouse(point(px(110.), px(71.5)));
    s.wait(Duration::from_millis(400));
    assert!(s.tooltip().is_some(), "within the wait");
    s.wait(Duration::from_millis(200));
    assert_eq!(s.tooltip(), None);
}
