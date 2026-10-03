//! State transitions: a hover's colours eased in and out, from where they
//! stand when the state changes again, at once under reduced motion or when
//! the element's own colours change, and drawn every frame they move inside
//! a view that is otherwise drawn from last frame. See
//! [`crate::fast::transition`].

use crate::{
    AppContext as _, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StateTransition, StatefulInteractiveElement as _, StyleRefinement,
    Styled as _, TestAppContext, Window, WindowHandle, div, hsla, px,
};
use std::time::Duration;

const REST: Hsla = hsla(0., 0., 0., 1.);
const HOVERED: Hsla = hsla(0., 0., 1., 1.);
const PRESSED: Hsla = hsla(0.6, 1., 0.5, 1.);

/// A row with a hover and a pressed colour, in a view of its own.
struct Row {
    rest: Hsla,
    transition: Option<StateTransition>,
}

impl Render for Row {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = div()
            .id("row")
            .size_full()
            .bg(self.rest)
            .hover(|style| style.bg(HOVERED))
            .active(|style| style.bg(PRESSED));
        match self.transition.clone() {
            Some(transition) => row.transition(transition),
            None => row,
        }
    }
}

/// The row, cached, so it is drawn from last frame unless something in it
/// asks otherwise.
struct Rows {
    row: Entity<Row>,
}

impl Render for Rows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(300.)).child(
            self.row
                .clone()
                .cached(StyleRefinement::default().w(px(100.)).h(px(20.))),
        )
    }
}

fn window(
    cx: &mut TestAppContext,
    transition: Option<StateTransition>,
) -> (WindowHandle<Rows>, Entity<Row>) {
    let window = cx.add_window(move |_, cx| Rows {
        row: cx.new(|_| Row {
            rest: REST,
            transition,
        }),
    });
    let row = window.update(cx, |rows, _, _| rows.row.clone()).unwrap();
    draw(cx, window);
    (window, row)
}

/// The fade-out the design asks for: in at once, out over 100 ms, linear so
/// the middle is easy to name.
fn fade_out() -> StateTransition {
    StateTransition::new(Duration::from_millis(100)).enter(Duration::ZERO)
}

/// Draws a frame and says the row's colour in it, as a lightness for the
/// greys, and whether the row asked for another frame.
fn draw(cx: &mut TestAppContext, window: WindowHandle<Rows>) -> (Hsla, bool) {
    cx.update_window(window.into(), |_, window, cx| {
        // Frames asked for before: the view they would notify is drawn
        // again all the same, as it asked for an animation frame.
        window.next_frame_callbacks.take();
        window.draw(cx).clear(cx);
        let row = window
            .rendered_frame
            .scene
            .quads
            .iter()
            .find(|quad| {
                let width = quad.bounds.size.width.0 / window.scale_factor();
                (width - 100.).abs() < 0.5
            })
            .map(|quad| quad.background.solid)
            .expect("the row is painted");
        let asked = !window.next_frame_callbacks.borrow().is_empty();
        (row, asked)
    })
    .unwrap()
}

fn move_mouse(cx: &mut TestAppContext, window: WindowHandle<Rows>, x: f32, y: f32) {
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_mouse_move(crate::point(px(x), px(y)), cx);
    })
    .unwrap();
}

fn press(cx: &mut TestAppContext, window: WindowHandle<Rows>, down: bool) {
    let position = crate::point(px(10.), px(10.));
    let (button, modifiers) = (crate::MouseButton::Left, crate::Modifiers::default());
    let event = if down {
        crate::PlatformInput::MouseDown(crate::MouseDownEvent {
            position,
            button,
            modifiers,
            click_count: 1,
            first_mouse: false,
        })
    } else {
        crate::PlatformInput::MouseUp(crate::MouseUpEvent {
            position,
            button,
            modifiers,
            click_count: 1,
        })
    };
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_event(event, cx);
    })
    .unwrap();
}

fn wait(cx: &mut TestAppContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
}

fn near(color: Hsla, lightness: f32) -> bool {
    (color.l - lightness).abs() < 0.02 && (color.a - 1.).abs() < 1e-3
}

/// The hover comes in at once and leaves over its exit: half way through
/// it is half way between, and the row stops asking for frames once it is
/// back at rest. The row's view is cached and nothing notifies it: the
/// frames it asks for draw it.
#[test]
fn a_hover_comes_in_at_once_and_fades_out_over_its_exit() {
    let mut cx = TestAppContext::single();
    let (window, _) = window(&mut cx, Some(fade_out()));

    move_mouse(&mut cx, window, 10., 10.);
    let (color, asked) = draw(&mut cx, window);
    assert_eq!(color, HOVERED, "in at once");
    assert!(!asked, "nothing moves");

    move_mouse(&mut cx, window, 250., 250.);
    let (color, asked) = draw(&mut cx, window);
    assert!(
        near(color, 1.),
        "the fade starts where the hover was: {color:?}"
    );
    assert!(asked);
    wait(&mut cx, 50);
    let (color, asked) = draw(&mut cx, window);
    assert!(near(color, 0.5), "half way: {color:?}");
    assert!(asked);
    wait(&mut cx, 60);
    let (color, asked) = draw(&mut cx, window);
    assert_eq!(color, REST, "at rest");
    assert!(!asked, "and still");
}

/// A hover that comes back half way through its fade starts from the colour
/// shown, not from either end.
#[test]
fn a_change_half_way_starts_from_the_colour_shown() {
    let mut cx = TestAppContext::single();
    let (window, _) = window(
        &mut cx,
        Some(StateTransition::new(Duration::from_millis(100))),
    );
    move_mouse(&mut cx, window, 10., 10.);
    draw(&mut cx, window);
    wait(&mut cx, 100);
    assert_eq!(draw(&mut cx, window).0, HOVERED);

    move_mouse(&mut cx, window, 250., 250.);
    draw(&mut cx, window);
    wait(&mut cx, 50);
    assert!(near(draw(&mut cx, window).0, 0.5));

    move_mouse(&mut cx, window, 10., 10.);
    draw(&mut cx, window);
    wait(&mut cx, 50);
    let (color, _) = draw(&mut cx, window);
    assert!(near(color, 0.75), "from half way, half way back: {color:?}");
}

/// A press while hovered is a change between two states, which takes the
/// enter duration (none here); its release back to the hover does too.
#[test]
fn a_press_and_its_release_take_the_enter_duration() {
    let mut cx = TestAppContext::single();
    let (window, _) = window(&mut cx, Some(fade_out()));
    move_mouse(&mut cx, window, 10., 10.);
    draw(&mut cx, window);
    press(&mut cx, window, true);
    assert_eq!(draw(&mut cx, window).0, PRESSED);
    press(&mut cx, window, false);
    assert_eq!(draw(&mut cx, window).0, HOVERED);
}

/// Under reduced motion the fade-out is skipped.
#[test]
fn reduced_motion_applies_every_change_at_once() {
    let mut cx = TestAppContext::single();
    cx.update(|cx| cx.set_reduce_motion(true));
    let (window, _) = window(&mut cx, Some(fade_out()));
    move_mouse(&mut cx, window, 10., 10.);
    draw(&mut cx, window);
    move_mouse(&mut cx, window, 250., 250.);
    let (color, asked) = draw(&mut cx, window);
    assert_eq!(color, REST);
    assert!(!asked);
}

/// The element's own colour changing, as a selection moving off it, is
/// applied at once, while a fade is under way too.
#[test]
fn a_change_of_the_elements_own_colour_is_applied_at_once() {
    let mut cx = TestAppContext::single();
    let (window, row) = window(&mut cx, Some(fade_out()));
    move_mouse(&mut cx, window, 10., 10.);
    draw(&mut cx, window);
    move_mouse(&mut cx, window, 250., 250.);
    draw(&mut cx, window);
    wait(&mut cx, 50);
    draw(&mut cx, window);

    let selected = hsla(0.3, 0.5, 0.5, 1.);
    row.update(&mut cx, |row, cx| {
        row.rest = selected;
        cx.notify();
    });
    let (color, asked) = draw(&mut cx, window);
    assert_eq!(color, selected);
    assert!(!asked);
}

/// Without a transition the hover is swapped, as upstream swaps it.
#[test]
fn without_a_transition_the_hover_is_swapped() {
    let mut cx = TestAppContext::single();
    let (window, _) = window(&mut cx, None);
    move_mouse(&mut cx, window, 10., 10.);
    assert_eq!(draw(&mut cx, window).0, HOVERED);
    move_mouse(&mut cx, window, 250., 250.);
    let (color, asked) = draw(&mut cx, window);
    assert_eq!(color, REST);
    assert!(!asked);
}

/// A list of rows that each have a hover, with or without a transition,
/// drawn every frame.
struct List {
    rows: usize,
    transition: Option<StateTransition>,
}

impl Render for List {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size(px(300.))
            .flex()
            .flex_col()
            .children((0..self.rows).map(|n| {
                let row = div()
                    .id(n)
                    .h(px(2.))
                    .w_full()
                    .bg(REST)
                    .hover(|style| style.bg(HOVERED));
                match self.transition.clone() {
                    Some(transition) => row.transition(transition),
                    None => row,
                }
            }))
    }
}

/// What a frame of 100 rows with a hover costs, notified every frame, with
/// and without a transition on each: at rest, and with the pointer moved
/// every frame, which leaves the first row's hover fading for the rest of
/// the run where it has a transition.
/// Ignored by default; run it with
///
/// ```text
/// cargo test -p gpui --lib --release --features test-support transition_bench -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn transition_bench() {
    let frames = 2_000;
    let measure = |transition: Option<StateTransition>, fading: bool| {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, _| List {
            rows: 100,
            transition,
        });
        // Notified and drawn in one update, and the frame a fading row asks
        // for taken (it is drawn again all the same), so that the test
        // platform draws nothing more after it.
        let frame = |cx: &mut TestAppContext| {
            cx.update_window(window.into(), |root, window, cx| {
                cx.notify(root.entity_id());
                window.draw(cx).clear(cx);
                window.next_frame_callbacks.take();
            })
            .unwrap();
        };
        for _ in 0..3 {
            frame(&mut cx);
        }
        cx.update_window(window.into(), |_, window, _| window.reset_layout_stats())
            .unwrap();
        for n in 0..frames {
            if fading {
                // Over a row, then off it: its hover fades for the rest of
                // the frames, which the clock never lets end.
                let y = if n == 0 { 1. } else { 250. };
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_mouse_move(crate::point(px(10.), px(y)), cx);
                })
                .unwrap();
            }
            frame(&mut cx);
        }
        let stats = cx
            .update_window(window.into(), |_, window, _| window.layout_stats())
            .unwrap();
        let spent = stats.build_time + stats.prepaint_time + stats.paint_time;
        spent.as_secs_f64() * 1e3 / stats.frames as f64
    };
    let long = StateTransition::new(Duration::from_secs(3600));
    let configs = [
        ("at rest, no transitions", None, false),
        ("at rest, a transition on each", Some(long.clone()), false),
        ("pointer off a row, no transitions", None, true),
        ("pointer off a row, its hover fading", Some(long), true),
    ];
    // Rounds alternate the four, and the median of each is taken, so load
    // from elsewhere falls on all of them alike.
    let mut costs = vec![Vec::new(); configs.len()];
    for _ in 0..9 {
        for (cost, (_, transition, moving)) in costs.iter_mut().zip(&configs) {
            cost.push(measure(transition.clone(), *moving));
        }
    }
    for (mut cost, (name, _, _)) in costs.into_iter().zip(configs) {
        cost.sort_by(f64::total_cmp);
        println!("{name}: median {:.4} ms per frame", cost[cost.len() / 2]);
    }
}
