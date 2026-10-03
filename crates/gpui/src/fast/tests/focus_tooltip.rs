//! Tooltips shown to the keyboard: after the delay on a control reached from
//! the keyboard, centred below it or above it at the window's foot, moved with
//! the focus, hidden by the pointer, and hidden by Escape before any binding
//! sees the key. See [`crate::fast::focus_tooltip`].

use crate::{
    AnyWindowHandle, AppContext as _, Bounds, Context, DEFAULT_TOOLTIP_SHOW_DELAY, Entity,
    FocusHandle, Hsla, InputEvent as _, InteractiveElement as _, IntoElement, KeyBinding,
    KeyDownEvent, KeyUpEvent, Keystroke, ParentElement as _, Pixels, Render, Stateful,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, TestAppContext, Window, div,
    hsla, point, px, size,
};
use std::{cell::Cell, rc::Rc, time::Duration};

actions!(focus_tooltip_test, [Cancel]);

const TIP: Hsla = hsla(0.8, 1., 0.5, 1.);

struct Tip;

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(60.)).h(px(16.)).bg(TIP)
    }
}

/// Three 40×20 controls with tooltips: `a` and `b` side by side near the
/// top, `c` at the window's foot. Escape is bound to `Cancel` around them.
struct Controls {
    a: FocusHandle,
    b: FocusHandle,
    c: FocusHandle,
    cancels: Rc<Cell<usize>>,
}

fn control(
    id: &'static str,
    focus: &FocusHandle,
    left: Pixels,
    top: Pixels,
) -> Stateful<crate::Div> {
    div()
        .id(id)
        .absolute()
        .left(left)
        .top(top)
        .w(px(40.))
        .h(px(20.))
        .track_focus(focus)
        .tooltip(|_, cx| cx.new(|_| Tip).into())
}

impl Render for Controls {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let cancels = self.cancels.clone();
        let foot = window.viewport_size().height - px(30.);
        div()
            .key_context("Controls")
            .size_full()
            .on_action(move |_: &Cancel, _, _| cancels.set(cancels.get() + 1))
            .child(control("a", &self.a, px(100.), px(50.)))
            .child(control("b", &self.b, px(200.), px(50.)))
            .child(control("c", &self.c, px(100.), foot))
    }
}

/// The controls, cached, so they are drawn from last frame unless something
/// they read changed.
struct Host {
    controls: Entity<Controls>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.controls
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

struct Setup {
    cx: TestAppContext,
    window: AnyWindowHandle,
    a: FocusHandle,
    b: FocusHandle,
    c: FocusHandle,
    cancels: Rc<Cell<usize>>,
}

fn setup() -> Setup {
    let mut cx = TestAppContext::single();
    cx.update(|cx| cx.bind_keys([KeyBinding::new("escape", Cancel, Some("Controls"))]));
    let (a, b, c) = cx.update(|cx| (cx.focus_handle(), cx.focus_handle(), cx.focus_handle()));
    let cancels = Rc::new(Cell::new(0));
    let window = cx
        .add_window({
            let (a, b, c, cancels) = (a.clone(), b.clone(), c.clone(), cancels.clone());
            move |_, cx| Host {
                controls: cx.new(|_| Controls { a, b, c, cancels }),
            }
        })
        .into();
    let mut setup = Setup {
        cx,
        window,
        a,
        b,
        c,
        cancels,
    };
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

    fn press(&mut self, key: &str) {
        let keystroke = Keystroke::parse(key).unwrap();
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    KeyDownEvent {
                        keystroke: keystroke.clone(),
                        is_held: false,
                        prefer_character_input: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
            })
            .unwrap();
        self.draw();
    }

    /// Moves the focus from the keyboard, as Tab would.
    fn tab_to(&mut self, focus: &FocusHandle) {
        self.press("f1");
        self.focus(focus);
    }

    fn focus(&mut self, focus: &FocusHandle) {
        self.cx
            .update_window(self.window, |_, window, cx| window.focus(focus, cx))
            .unwrap();
        self.draw();
    }

    fn move_mouse(&mut self, x: f32, y: f32) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.simulate_mouse_move(point(px(x), px(y)), cx);
            })
            .unwrap();
        self.draw();
    }

    fn wait(&mut self, duration: Duration) {
        self.cx.executor().advance_clock(duration);
        self.draw();
    }

    fn is_focused(&mut self, focus: &FocusHandle) -> bool {
        self.cx
            .update_window(self.window, |_, window, _| focus.is_focused(window))
            .unwrap()
    }

    /// Where the tooltip is drawn, if one is.
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

    fn tooltip_at(&mut self) -> Option<(f32, f32)> {
        self.tooltip()
            .map(|bounds| (bounds.origin.x.as_f32(), bounds.origin.y.as_f32()))
    }
}

const ALMOST: Duration = Duration::from_millis(DEFAULT_TOOLTIP_SHOW_DELAY.as_millis() as u64 - 1);

#[test]
fn a_control_reached_from_the_keyboard_shows_its_tooltip_centred_below_it() {
    let mut s = setup();
    let a = s.a.clone();
    s.tab_to(&a);
    s.wait(ALMOST);
    assert_eq!(s.tooltip(), None, "not before the delay");
    s.wait(Duration::from_millis(1));
    // Centred on the control's 120, 4 px below its foot at 70.
    let tooltip = s.tooltip().expect("shown after the delay");
    assert_eq!(
        tooltip,
        Bounds::new(point(px(90.), px(74.)), size(px(60.), px(16.)))
    );
    s.wait(Duration::from_secs(5));
    assert_eq!(s.tooltip_at(), Some((90., 74.)), "kept while focused");
}

#[test]
fn at_the_windows_foot_it_shows_above_the_control() {
    let mut s = setup();
    let c = s.c.clone();
    s.tab_to(&c);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    let viewport =
        s.cx.update_window(s.window, |_, window, _| window.viewport_size())
            .unwrap();
    let top = viewport.height.as_f32() - 30.;
    assert_eq!(s.tooltip_at(), Some((90., top - 4. - 16.)));
}

#[test]
fn the_tooltip_follows_the_focus() {
    let mut s = setup();
    let (a, b) = (s.a.clone(), s.b.clone());
    s.tab_to(&a);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert_eq!(s.tooltip_at(), Some((90., 74.)));
    s.tab_to(&b);
    assert_eq!(s.tooltip(), None, "gone with the focus");
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert_eq!(s.tooltip_at(), Some((190., 74.)));
}

/// Escape hides the tooltip before the dialog's Escape binding sees the key,
/// and the focus stays; the next Escape reaches the binding.
#[test]
fn escape_hides_it_first_and_keeps_the_focus() {
    let mut s = setup();
    let (a, b) = (s.a.clone(), s.b.clone());
    s.tab_to(&a);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert!(s.tooltip().is_some());

    s.press("escape");
    assert_eq!(s.tooltip(), None);
    assert!(s.is_focused(&a));
    assert_eq!(s.cancels.get(), 0, "the binding never saw it");
    s.wait(Duration::from_secs(5));
    assert_eq!(s.tooltip(), None, "stays hidden while focused");

    s.press("escape");
    assert_eq!(
        s.cancels.get(),
        1,
        "with no tooltip up, Escape is the dialog's"
    );

    // Away and back, the tooltip shows again.
    s.tab_to(&b);
    s.tab_to(&a);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert_eq!(s.tooltip_at(), Some((90., 74.)));
}

#[test]
fn escape_waiting_for_the_delay_is_the_dialogs() {
    let mut s = setup();
    let a = s.a.clone();
    s.tab_to(&a);
    s.press("escape");
    assert_eq!(s.cancels.get(), 1);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert!(s.tooltip().is_some());
}

#[test]
fn the_pointer_hides_it() {
    let mut s = setup();
    let a = s.a.clone();
    s.tab_to(&a);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert!(s.tooltip().is_some());
    s.move_mouse(5., 5.);
    assert_eq!(s.tooltip(), None);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert_eq!(s.tooltip(), None);
}

/// Focused while the pointer was the last input, as a click focuses, the
/// control shows nothing to the keyboard: only a hover would show it.
#[test]
fn a_control_focused_with_the_pointer_shows_nothing() {
    let mut s = setup();
    let a = s.a.clone();
    s.move_mouse(5., 5.);
    s.focus(&a);
    s.wait(DEFAULT_TOOLTIP_SHOW_DELAY);
    assert_eq!(s.tooltip(), None);
}

/// A list of rows, each a control with a tooltip and a focus handle.
struct List {
    rows: Vec<FocusHandle>,
}

impl Render for List {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size(px(300.))
            .flex()
            .flex_col()
            .children(self.rows.iter().enumerate().map(|(n, focus)| {
                div()
                    .id(n)
                    .h(px(2.))
                    .w_full()
                    .track_focus(focus)
                    .tooltip(|_, cx| cx.new(|_| Tip).into())
            }))
    }
}

/// What a frame of 100 controls with tooltips costs, notified every frame:
/// with the focus elsewhere, and with one of them focused from the keyboard
/// and its tooltip up.
/// Ignored by default; run it with
///
/// ```text
/// cargo test -p gpui --lib --release --features test-support focus_tooltip_bench -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn focus_tooltip_bench() {
    let frames = 2_000;
    let measure = |focused: bool| {
        let mut cx = TestAppContext::single();
        let rows: Vec<_> = cx.update(|cx| (0..100).map(|_| cx.focus_handle()).collect());
        let first = rows[0].clone();
        let window = cx.add_window(|_, _| List { rows });
        let frame = |cx: &mut TestAppContext| {
            cx.update_window(window.into(), |root, window, cx| {
                cx.notify(root.entity_id());
                window.draw(cx).clear(cx);
            })
            .unwrap();
        };
        frame(&mut cx);
        if focused {
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_event(
                    KeyDownEvent {
                        keystroke: Keystroke::parse("f1").unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                window.focus(&first, cx);
            })
            .unwrap();
            frame(&mut cx);
            cx.executor().advance_clock(DEFAULT_TOOLTIP_SHOW_DELAY);
            cx.run_until_parked();
        }
        for _ in 0..3 {
            frame(&mut cx);
        }
        cx.update_window(window.into(), |_, window, _| window.reset_layout_stats())
            .unwrap();
        for _ in 0..frames {
            frame(&mut cx);
        }
        let stats = cx
            .update_window(window.into(), |_, window, _| window.layout_stats())
            .unwrap();
        let spent = stats.build_time + stats.prepaint_time + stats.paint_time;
        spent.as_secs_f64() * 1e3 / stats.frames as f64
    };
    let configs = [
        ("focus elsewhere", false),
        ("one focused, tooltip up", true),
    ];
    // Rounds alternate the two, and the median of each is taken, so load
    // from elsewhere falls on both alike.
    let mut costs = vec![Vec::new(); configs.len()];
    for _ in 0..9 {
        for (cost, (_, focused)) in costs.iter_mut().zip(&configs) {
            cost.push(measure(*focused));
        }
    }
    for (mut cost, (name, _)) in costs.into_iter().zip(configs) {
        cost.sort_by(f64::total_cmp);
        println!("{name}: median {:.4} ms per frame", cost[cost.len() / 2]);
    }
}
