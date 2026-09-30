//! [`crate::App::subscription_counts`] counts what a view registers, and
//! what it leaves behind once it is gone.

use crate::{
    AppContext as _, Context, Entity, EventEmitter, Global, IntoElement, ParentElement as _, Render,
    Subscription,
    SubscriptionCounts, TestAppContext, Window, div,
};

struct Model;

impl EventEmitter<()> for Model {}

struct Theme;

impl Global for Theme {}

/// A view that registers one callback of each kind it can, and optionally
/// detaches one more observation, which outlives it.
struct Tile {
    _subscriptions: Vec<Subscription>,
}

impl Tile {
    fn new(model: &Entity<Model>, leak: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        if leak {
            cx.observe(model, |_, _, _| {}).detach();
        }
        Self {
            _subscriptions: vec![
                cx.observe(model, |_, _, _| {}),
                cx.subscribe(model, |_, _, _: &(), _| {}),
                cx.observe_release(model, |_, _, _| {}),
                cx.observe_global::<Theme>(|_, _| {}),
                cx.observe_window_bounds(window, |_, _, _| {}),
            ],
        }
    }
}

impl Render for Tile {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

struct Host {
    tile: Option<Entity<Tile>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().children(self.tile.clone())
    }
}

/// Opens a tile, returning the counts while it is open, then closes it and
/// draws twice, returning the counts once it is released.
fn open_and_close(
    cx: &mut TestAppContext,
    host: crate::WindowHandle<Host>,
    model: &Entity<Model>,
    leak: bool,
) -> (SubscriptionCounts, SubscriptionCounts) {
    host.update(cx, |host, window, cx| {
        host.tile = Some(cx.new(|cx| Tile::new(model, leak, window, cx)));
        cx.notify();
    })
    .unwrap();
    cx.run_until_parked();
    let open = cx.subscription_counts();
    host.update(cx, |host, _, cx| {
        host.tile = None;
        cx.notify();
    })
    .unwrap();
    cx.run_until_parked();
    // The frame drawn before holds the tile it drew until the next draw
    // clears it.
    host.update(cx, |_, _, cx| cx.notify()).unwrap();
    cx.run_until_parked();
    (open, cx.subscription_counts())
}

#[test]
fn a_view_released_leaves_nothing_subscribed_and_a_detached_observation_is_counted() {
    let mut cx = TestAppContext::single();
    let model = cx.new(|_| Model);
    let host = cx.add_window(|_, _| Host { tile: None });
    cx.run_until_parked();
    let baseline = cx.subscription_counts();

    let (open, closed) = open_and_close(&mut cx, host, &model, false);
    assert_eq!(
        open,
        SubscriptionCounts {
            observers: baseline.observers + 1,
            event_listeners: baseline.event_listeners + 1,
            release_listeners: baseline.release_listeners + 1,
            global_observers: baseline.global_observers + 1,
            window_observers: baseline.window_observers + 1,
            ..baseline
        }
    );
    assert_eq!(closed, baseline);

    for _ in 0..10 {
        assert_eq!(open_and_close(&mut cx, host, &model, false).1, baseline);
    }

    let (_, leaked) = open_and_close(&mut cx, host, &model, true);
    assert_eq!(
        leaked,
        SubscriptionCounts {
            observers: baseline.observers + 1,
            ..baseline
        }
    );
    assert_eq!(leaked.total(), baseline.total() + 1);
}
