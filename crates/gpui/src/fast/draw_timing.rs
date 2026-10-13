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

use crate::{App, Subscription, Window};

/// One draw of a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drawn {
    /// When the draw began.
    pub started: Instant,
    /// How long render, layout, prepaint and paint took.
    pub took: Duration,
}

/// A callback [`Window::observe_draws`] was handed.
pub(crate) type DrawObserver = Box<dyn FnMut(Drawn, &mut App) -> bool + 'static>;

impl Window {
    /// Calls `callback` after each draw of this window with how long it took,
    /// until the subscription is dropped.
    pub fn observe_draws(
        &self,
        mut callback: impl FnMut(Drawn, &mut App) + 'static,
    ) -> Subscription {
        let (subscription, activate) = self.draw_observers.insert(
            (),
            Box::new(move |drawn, cx| {
                callback(drawn, cx);
                true
            }),
        );
        activate();
        subscription
    }

    /// Tells the observers the draw that began at `started` is done.
    pub(crate) fn drawn(&self, started: Instant, cx: &mut App) {
        let drawn = Drawn {
            started,
            took: started.elapsed(),
        };
        self.draw_observers
            .clone()
            .retain(&(), |observer| observer(drawn, cx));
    }
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
        let subscription = cx.update(|window, _| {
            window.observe_draws(move |drawn, _| kept.borrow_mut().push(drawn))
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(seen.borrow().len(), 1, "one draw");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(seen.borrow().len(), 2);
        assert!(seen.borrow()[0].started <= seen.borrow()[1].started);
        drop(subscription);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(seen.borrow().len(), 2, "no longer asked");
    }
}
