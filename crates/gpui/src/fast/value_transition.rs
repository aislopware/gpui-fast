//! A value eased toward a goal over a duration, kept with an element from frame
//! to frame, that turns back from wherever it stands when the goal changes.
//!
//! Ported from gpui-ce's `Transition` and `Window::use_keyed_transition`
//! (<https://github.com/gpui-ce/gpui-ce>, `crates/gpui/src/transition.rs`, the
//! duration-based form before its `Motion` rewrite), Copyright the gpui-ce
//! contributors, licensed under the Apache License, Version 2.0. Changed from
//! it: values interpolate through [`Interpolate`]; time is read from the
//! executor's clock; the value shown when the goal changes is taken at that
//! moment rather than from the last frame drawn; a change back to where the
//! value came from takes only as long as the way back, as CSS transitions
//! shorten a reversal; [`App::reduce_motion`] applies every change at once; a
//! changed goal notifies the view holding the value.
//!
//! One value serves one surface. An overlay's openness, say, goes to 1 when it
//! opens and 0 when it closes, and every property drawn from it (opacity,
//! offset, scale) follows that one value, so an overlay closed half open
//! turns back from where it is, in half the time.
//!
//! ```ignore
//! fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
//!     let openness = window
//!         .use_keyed_transition("menu", cx, Duration::from_millis(160), |_, _| 0.)
//!         .with_easing(ease_out_quint());
//!     openness.set_goal(if self.open { 1. } else { 0. }, cx);
//!     let shown = openness.evaluate(window, cx);
//!     div().opacity(shown).top(px(8. * (1. - shown)))
//! }
//! ```
//!
//! Nothing outside this module refers to it: code that never asks for a
//! transition pays nothing.
//!
//! [`App::reduce_motion`]: crate::App::reduce_motion

use crate::{App, ElementId, Entity, EntityId, Interpolate, Window};
use scheduler::Instant;
use std::{rc::Rc, time::Duration};

/// A handle on a value easing toward its goal, from
/// [`Window::use_keyed_transition`] or [`Window::use_transition`]. See the
/// [module](self) for how it moves.
///
/// The handle is made anew each frame; where the value stands lives in the
/// element state the window keeps under its key, while the duration and
/// easing are the handle's own, so a frame may change them.
pub struct ValueTransition<T: 'static> {
    state: Entity<ValueTransitionState<T>>,
    duration: Duration,
    easing: Rc<dyn Fn(f32) -> f32>,
}

impl<T: 'static> Clone for ValueTransition<T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            duration: self.duration,
            easing: self.easing.clone(),
        }
    }
}

impl<T: 'static> std::fmt::Debug for ValueTransition<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValueTransition")
            .field("state", &self.state.entity_id())
            .field("duration", &self.duration)
            .finish_non_exhaustive()
    }
}

/// Where a [`ValueTransition`] stands, kept by the window between frames.
#[derive(Clone, Debug)]
pub struct ValueTransitionState<T> {
    /// The value the current move started from.
    from: T,
    /// The goal.
    to: T,
    /// When the current move started; `None` once at rest.
    started: Option<Instant>,
    /// The share of the duration the current move takes: 1 for a move to a
    /// new goal, less for a turn back part way.
    scale: f32,
    /// The goal before the current one, which a turn back returns to.
    reversing_from: T,
}

impl<T: Clone> ValueTransitionState<T> {
    /// A value at rest at `value`.
    pub fn new(value: T) -> Self {
        Self {
            from: value.clone(),
            to: value.clone(),
            started: None,
            scale: 1.,
            reversing_from: value,
        }
    }

    fn settle(&mut self, value: T) {
        *self = Self::new(value);
    }
}

impl<T: Interpolate + Clone + PartialEq + 'static> ValueTransition<T> {
    /// A handle on `state`, moving over `duration`, eased linearly.
    pub fn new(state: Entity<ValueTransitionState<T>>, duration: Duration) -> Self {
        Self {
            state,
            duration,
            easing: Rc::new(crate::linear),
        }
    }

    /// The easing, mapping the time gone, from 0 to 1, to how far the value
    /// has moved. It may overshoot 0 and 1, as a spring's does.
    pub fn with_easing(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Rc::new(easing);
        self
    }

    /// The value at this moment, asking for the next frame while it moves.
    pub fn evaluate(&self, window: &mut Window, cx: &App) -> T {
        let state = self.state.read(cx);
        if cx.reduce_motion() {
            return state.to.clone();
        }
        match self.time_gone(state, cx) {
            Some(gone) if gone < 1. => {
                window.request_animation_frame();
                T::interpolate(state.from.clone(), state.to.clone(), (self.easing)(gone))
            }
            _ => state.to.clone(),
        }
    }

    /// The goal the value moves toward.
    pub fn goal<'a>(&self, cx: &'a App) -> &'a T {
        &self.state.read(cx).to
    }

    /// Whether the value is still on its way to the goal.
    pub fn is_moving(&self, cx: &App) -> bool {
        !cx.reduce_motion()
            && self
                .time_gone(self.state.read(cx), cx)
                .is_some_and(|gone| gone < 1.)
    }

    /// Moves the value toward `goal` from where it stands now, notifying the
    /// view that holds it. Returns whether the goal changed.
    ///
    /// A goal that returns to where the value was coming from takes as long
    /// as the way back, so an overlay closed half open takes half the time
    /// to close. Under [`App::reduce_motion`] the value goes to `goal` at
    /// once.
    ///
    /// [`App::reduce_motion`]: crate::App::reduce_motion
    pub fn set_goal(&self, goal: T, cx: &mut App) -> bool {
        let now = cx.background_executor().now();
        let reduce_motion = cx.reduce_motion();
        self.state.update(cx, |state, cx| {
            if state.to == goal {
                return false;
            }
            cx.notify();
            let gone = self
                .time_gone_at(state, now)
                .filter(|gone| *gone < 1. && !reduce_motion);
            let Some(gone) = gone else {
                if reduce_motion || self.duration.is_zero() {
                    state.settle(goal);
                } else {
                    let last = std::mem::replace(&mut state.to, goal);
                    state.from = last.clone();
                    state.reversing_from = last;
                    state.started = Some(now);
                    state.scale = 1.;
                }
                return true;
            };
            let eased = (self.easing)(gone);
            let shown = T::interpolate(state.from.clone(), state.to.clone(), eased);
            // CSS Transitions' reversing shortening factor: a turn back to
            // the value's last goal takes the share of the duration the
            // value has covered, so going and coming back match.
            state.scale = if goal == state.reversing_from {
                (eased.clamp(0., 1.) * state.scale + 1. - state.scale).clamp(0., 1.)
            } else {
                1.
            };
            state.reversing_from = std::mem::replace(&mut state.to, goal);
            state.from = shown;
            state.started = Some(now);
            true
        })
    }

    /// Puts the value at `value` at once, at rest, notifying the view that
    /// holds it.
    pub fn jump_to(&self, value: T, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.settle(value);
            cx.notify();
        });
    }

    /// The id of the entity holding the value.
    pub fn entity_id(&self) -> EntityId {
        self.state.entity_id()
    }

    fn time_gone(&self, state: &ValueTransitionState<T>, cx: &App) -> Option<f32> {
        self.time_gone_at(state, cx.background_executor().now())
    }

    /// The share of the current move's time gone at `now`, from 0 to 1, or
    /// `None` at rest.
    fn time_gone_at(&self, state: &ValueTransitionState<T>, now: Instant) -> Option<f32> {
        let started = state.started?;
        let duration = self.duration.as_secs_f32() * state.scale;
        if duration <= 0. {
            return Some(1.);
        }
        let gone = now.saturating_duration_since(started).as_secs_f32();
        Some((gone / duration).min(1.))
    }
}

impl Window {
    /// A value that eases toward its goal over `duration`, kept under `key`
    /// for as long as the element rendering it is drawn in consecutive
    /// frames, and starting at rest at what `init` returns. See
    /// [`ValueTransition`].
    pub fn use_keyed_transition<T: Interpolate + Clone + PartialEq + 'static>(
        &mut self,
        key: impl Into<ElementId>,
        cx: &mut App,
        duration: Duration,
        init: impl FnOnce(&mut Self, &mut App) -> T,
    ) -> ValueTransition<T> {
        let state = self.use_keyed_state(key, cx, |window, cx| {
            ValueTransitionState::new(init(window, cx))
        });
        ValueTransition::new(state, duration)
    }

    /// [`Window::use_keyed_transition`], keyed by where it is called from.
    #[track_caller]
    pub fn use_transition<T: Interpolate + Clone + PartialEq + 'static>(
        &mut self,
        cx: &mut App,
        duration: Duration,
        init: impl FnOnce(&mut Self, &mut App) -> T,
    ) -> ValueTransition<T> {
        self.use_keyed_transition(
            ElementId::CodeLocation(*core::panic::Location::caller()),
            cx,
            duration,
            init,
        )
    }
}
