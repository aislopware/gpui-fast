//! Value transitions: eased to their goal and then at rest, turned back from
//! where they stand in the time the way back takes, at once under reduced
//! motion, and drawn every frame they move inside a view that is otherwise
//! drawn from last frame. See [`crate::fast::value_transition`].

use crate::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
    Styled as _, TestAppContext, ValueTransition, Window, WindowHandle, div, px,
};
use std::time::Duration;

/// An overlay whose openness eases toward `goal`, in a view of its own.
struct Overlay {
    goal: f32,
    duration: Duration,
    easing: Option<fn(f32) -> f32>,
    /// The openness drawn last frame.
    shown: f32,
    /// Last frame's handle, to change the goal from outside a frame.
    openness: Option<ValueTransition<f32>>,
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut openness = window.use_keyed_transition("openness", cx, self.duration, |_, _| 0.);
        if let Some(easing) = self.easing {
            openness = openness.with_easing(easing);
        }
        openness.set_goal(self.goal, cx);
        self.shown = openness.evaluate(window, cx);
        self.openness = Some(openness);
        div().size_full().opacity(self.shown)
    }
}

/// The overlay, cached, so it is drawn from last frame unless something in
/// it asks otherwise.
struct Host {
    overlay: Entity<Overlay>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(300.)).child(
            self.overlay
                .clone()
                .cached(StyleRefinement::default().w(px(100.)).h(px(100.))),
        )
    }
}

fn window(
    cx: &mut TestAppContext,
    duration_ms: u64,
    easing: Option<fn(f32) -> f32>,
) -> (WindowHandle<Host>, Entity<Overlay>) {
    let window = cx.add_window(move |_, cx| Host {
        overlay: cx.new(|_| Overlay {
            goal: 0.,
            duration: Duration::from_millis(duration_ms),
            easing,
            shown: 0.,
            openness: None,
        }),
    });
    let overlay = window
        .update(cx, |host, _, _| host.overlay.clone())
        .unwrap();
    draw(cx, window, &overlay);
    (window, overlay)
}

/// Draws a frame and says the openness in it and whether the overlay asked
/// for another frame.
fn draw(
    cx: &mut TestAppContext,
    window: WindowHandle<Host>,
    overlay: &Entity<Overlay>,
) -> (f32, bool) {
    cx.update_window(window.into(), |_, window, cx| {
        // Frames asked for before: the view they would notify is drawn
        // again all the same, as it asked for an animation frame.
        window.next_frame_callbacks.take();
        window.draw(cx).clear(cx);
        let asked = !window.next_frame_callbacks.borrow().is_empty();
        (overlay.read(cx).shown, asked)
    })
    .unwrap()
}

/// Sets the goal the way the application's state would: the overlay's own
/// field, the view notified.
fn open(cx: &mut TestAppContext, overlay: &Entity<Overlay>, goal: f32) {
    overlay.update(cx, |overlay, cx| {
        overlay.goal = goal;
        cx.notify();
    });
}

/// Sets the goal through the handle alone, from outside any frame and without
/// notifying anything.
fn retarget(cx: &mut TestAppContext, overlay: &Entity<Overlay>, goal: f32) -> bool {
    overlay.update(cx, |overlay, cx| {
        overlay.goal = goal;
        let openness = overlay.openness.clone().expect("drawn once");
        openness.set_goal(goal, cx)
    })
}

fn wait(cx: &mut TestAppContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
}

fn near(value: f32, expected: f32) -> bool {
    (value - expected).abs() < 1e-3
}

#[test]
fn a_value_eases_to_its_goal_then_rests() {
    let mut cx = TestAppContext::single();
    let (window, overlay) = window(&mut cx, 100, None);
    assert_eq!(draw(&mut cx, window, &overlay), (0., false));

    open(&mut cx, &overlay, 1.);
    let (shown, asked) = draw(&mut cx, window, &overlay);
    assert!(near(shown, 0.) && asked);
    wait(&mut cx, 50);
    // Drawn again only because it asked for the frame: nothing notified the
    // cached overlay.
    let (shown, asked) = draw(&mut cx, window, &overlay);
    assert!(near(shown, 0.5) && asked, "half way: {shown}");
    wait(&mut cx, 50);
    assert_eq!(draw(&mut cx, window, &overlay), (1., false));
    wait(&mut cx, 50);
    assert_eq!(draw(&mut cx, window, &overlay), (1., false));
}

/// An overlay closed half open closes from where it is, in the time it took
/// to get there.
#[test]
fn turned_back_half_way_it_returns_from_where_it_is_in_half_the_time() {
    let mut cx = TestAppContext::single();
    let (window, overlay) = window(&mut cx, 100, None);
    open(&mut cx, &overlay, 1.);
    draw(&mut cx, window, &overlay);
    wait(&mut cx, 50);
    assert!(near(draw(&mut cx, window, &overlay).0, 0.5));

    // Through the handle alone: the change notifies the view holding it.
    assert!(retarget(&mut cx, &overlay, 0.));
    let (shown, asked) = draw(&mut cx, window, &overlay);
    assert!(near(shown, 0.5) && asked, "no jump: {shown}");
    wait(&mut cx, 25);
    assert!(near(draw(&mut cx, window, &overlay).0, 0.25));
    wait(&mut cx, 25);
    assert_eq!(draw(&mut cx, window, &overlay), (0., false));
}

/// Turned back twice, the second turn takes the share of the full move left
/// to cover, as CSS transitions compound their shortening.
#[test]
fn turned_back_twice_each_way_takes_the_share_it_covers() {
    let mut cx = TestAppContext::single();
    let (window, overlay) = window(&mut cx, 200, None);
    open(&mut cx, &overlay, 1.);
    draw(&mut cx, window, &overlay);
    wait(&mut cx, 100);
    assert!(near(draw(&mut cx, window, &overlay).0, 0.5));
    open(&mut cx, &overlay, 0.);
    draw(&mut cx, window, &overlay);
    wait(&mut cx, 50);
    assert!(near(draw(&mut cx, window, &overlay).0, 0.25));

    // From 0.25 back to 1: three quarters of the way, three quarters of the
    // time.
    open(&mut cx, &overlay, 1.);
    assert!(near(draw(&mut cx, window, &overlay).0, 0.25));
    wait(&mut cx, 75);
    let shown = draw(&mut cx, window, &overlay).0;
    assert!(near(shown, 0.625), "half of the way back: {shown}");
    wait(&mut cx, 75);
    assert_eq!(draw(&mut cx, window, &overlay), (1., false));
}

/// A goal that isn't where the value came from takes the full duration, from
/// the value shown.
#[test]
fn a_new_goal_mid_way_takes_the_full_duration_from_the_value_shown() {
    let mut cx = TestAppContext::single();
    let (window, overlay) = window(&mut cx, 100, None);
    open(&mut cx, &overlay, 1.);
    draw(&mut cx, window, &overlay);
    wait(&mut cx, 50);
    draw(&mut cx, window, &overlay);
    open(&mut cx, &overlay, 2.);
    assert!(near(draw(&mut cx, window, &overlay).0, 0.5));
    wait(&mut cx, 50);
    assert!(near(draw(&mut cx, window, &overlay).0, 1.25));
    wait(&mut cx, 50);
    assert_eq!(draw(&mut cx, window, &overlay), (2., false));
}

/// The value shown when the goal changes is the one at that moment, not the
/// one last drawn, so a change between frames still continues smoothly.
#[test]
fn a_goal_changed_between_frames_starts_from_the_value_at_that_moment() {
    let mut cx = TestAppContext::single();
    let (window, overlay) = window(&mut cx, 100, None);
    open(&mut cx, &overlay, 1.);
    draw(&mut cx, window, &overlay);
    wait(&mut cx, 20);
    draw(&mut cx, window, &overlay);
    // 60 ms in, though the last frame drew it 20 ms in.
    wait(&mut cx, 40);
    retarget(&mut cx, &overlay, 0.);
    let shown = draw(&mut cx, window, &overlay).0;
    assert!(near(shown, 0.6), "from where it stands now: {shown}");
}

#[test]
fn the_easing_shapes_the_way() {
    let mut cx = TestAppContext::single();
    let (window, overlay) = window(&mut cx, 100, Some(|t| t * t));
    open(&mut cx, &overlay, 1.);
    draw(&mut cx, window, &overlay);
    wait(&mut cx, 50);
    assert!(near(draw(&mut cx, window, &overlay).0, 0.25));
}

#[test]
fn under_reduced_motion_a_goal_is_taken_at_once() {
    let mut cx = TestAppContext::single();
    let (window, overlay) = window(&mut cx, 100, None);
    cx.update(|cx| cx.set_reduce_motion(true));
    open(&mut cx, &overlay, 1.);
    assert_eq!(draw(&mut cx, window, &overlay), (1., false));

    // A move under way when motion is reduced ends where it is going.
    cx.update(|cx| cx.set_reduce_motion(false));
    open(&mut cx, &overlay, 0.);
    draw(&mut cx, window, &overlay);
    wait(&mut cx, 30);
    assert!(near(draw(&mut cx, window, &overlay).0, 0.7));
    cx.update(|cx| cx.set_reduce_motion(true));
    assert_eq!(draw(&mut cx, window, &overlay), (0., false));
}

#[test]
fn the_same_goal_changes_nothing() {
    let mut cx = TestAppContext::single();
    let (window, overlay) = window(&mut cx, 100, None);
    assert!(!retarget(&mut cx, &overlay, 0.));
    assert_eq!(draw(&mut cx, window, &overlay), (0., false));
    assert!(retarget(&mut cx, &overlay, 1.));
    assert!(!retarget(&mut cx, &overlay, 1.));
}

/// A list of rows, each with its opacity from a value transition of its own
/// or from none, drawn every frame.
struct List {
    rows: usize,
    transitions: bool,
    goal: f32,
}

impl Render for List {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let goal = self.goal;
        let rows = (0..self.rows)
            .map(|n| {
                let opacity = if self.transitions {
                    let value =
                        window.use_keyed_transition(n, cx, Duration::from_secs(3600), |_, _| 1.);
                    value.set_goal(goal, cx);
                    value.evaluate(window, cx)
                } else {
                    goal
                };
                div().h(px(2.)).w_full().opacity(opacity)
            })
            .collect::<Vec<_>>();
        div().size(px(300.)).flex().flex_col().children(rows)
    }
}

/// What a frame of 100 rows costs, notified every frame, with each row's
/// opacity a constant or a value transition's: at rest, and moving toward a
/// goal an hour away.
/// Ignored by default; run it with
///
/// ```text
/// cargo test -p gpui --lib --release --features test-support value_transition_bench -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn value_transition_bench() {
    let frames = 2_000;
    let measure = |transitions: bool, moving: bool| {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, _| List {
            rows: 100,
            transitions,
            goal: 1.,
        });
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
        if moving {
            window.update(&mut cx, |list, _, _| list.goal = 0.).unwrap();
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
        ("constant opacities", false, false),
        ("a transition on each, at rest", true, false),
        ("a transition on each, moving", true, true),
    ];
    // Rounds alternate the three, and the median of each is taken, so load
    // from elsewhere falls on all of them alike.
    let mut costs = vec![Vec::new(); configs.len()];
    for _ in 0..9 {
        for (cost, (_, transitions, moving)) in costs.iter_mut().zip(&configs) {
            cost.push(measure(*transitions, *moving));
        }
    }
    for (mut cost, (name, _, _)) in costs.into_iter().zip(configs) {
        cost.sort_by(f64::total_cmp);
        println!("{name}: median {:.4} ms per frame", cost[cost.len() / 2]);
    }
}
