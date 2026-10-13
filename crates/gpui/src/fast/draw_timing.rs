//! How long each draw of a window took, for a host's frame-time probe.
//!
//! A probe drawn inside the window can only time what the views it sits in
//! build again: a frame that builds one cached view anew leaves the root as
//! it was, so a probe at the root sees nothing of it. The window itself knows
//! when every draw begins and ends: [`Window::observe_draws`] hands each
//! draw's start and span (render, layout, prepaint and paint; not the
//! presentation that follows) to whoever asked, on the wall clock, since a
//! draw's cost is real time even under a test's clock.

use std::time::{Duration, Instant};

use crate::{App, Global, Subscription, SubscriberSet, Window};

/// Every window's draw observers, by the window's id.
struct DrawObservers(SubscriberSet<u64, DrawObserver>);

impl Default for DrawObservers {
    fn default() -> Self {
        Self(SubscriberSet::new())
    }
}

impl Global for DrawObservers {}

/// A callback [`Window::observe_draws`] was handed: when the draw began and
/// how long it took.
type DrawObserver = Box<dyn FnMut(Instant, Duration, &mut App) -> bool + 'static>;

impl Window {
    /// Calls `callback` after each draw of this window with when it began and
    /// how long render, layout, prepaint and paint took, until the
    /// subscription is dropped.
    pub fn observe_draws(
        &self,
        cx: &mut App,
        mut callback: impl FnMut(Instant, Duration, &mut App) + 'static,
    ) -> Subscription {
        let observers = cx.default_global::<DrawObservers>().0.clone();
        let (subscription, activate) = observers.insert(
            self.handle.window_id().as_u64(),
            Box::new(move |started, took, cx| {
                callback(started, took, cx);
                true
            }),
        );
        activate();
        subscription
    }
}

/// When a draw begins, on the wall clock: a draw's cost is real time even
/// under a test's clock.
pub(crate) fn now() -> Instant {
    Instant::now()
}

/// Tells `window`'s observers the draw that began at `started` is done.
pub(crate) fn drawn(window: &Window, started: Instant, cx: &mut App) {
    let Some(observers) = cx.try_global::<DrawObservers>().map(|o| o.0.clone()) else {
        return;
    };
    let took = started.elapsed();
    observers.retain(&window.handle.window_id().as_u64(), |observer| {
        observer(started, took, cx)
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::{Context, IntoElement, Render, TestAppContext, Window, div};

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// Every draw is told, once, with a span that ends after it began; a
    /// dropped subscription hears no more.
    #[gpui::test]
    fn each_draw_is_told_once(cx: &mut TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| Empty);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let kept = Rc::clone(&seen);
        let subscription = cx.update(|window, cx| {
            window.observe_draws(cx, move |started, took, _| {
                kept.borrow_mut().push((started, took))
            })
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(seen.borrow().len(), 1, "one draw");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(seen.borrow().len(), 2);
        assert!(seen.borrow()[0].0 <= seen.borrow()[1].0);
        drop(subscription);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(seen.borrow().len(), 2, "no longer asked");
    }
}
