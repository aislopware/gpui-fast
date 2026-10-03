//! An element's background and border colours eased into and out of its
//! interaction states, rather than swapped.
//!
//! A hover, press or focus style is a refinement laid over the element's base
//! style, applied in the frame the state changes. With a [`StateTransition`]
//! set (`.transition(..)` on an element with an id), paint follows the
//! colours that refinement asks for over a duration instead: the colour shown
//! when the state changed moves to the new one, on the transition's easing.
//! The state itself, and everything but the two colours, still changes at
//! once. Only the colours paint uses are followed, so layout never waits on a
//! transition.
//!
//! - Returning to the element's rest (every interaction state left) takes
//!   the `exit` duration; any other change, into a state or between two,
//!   takes `enter`. A pointer that leaves a row can fade its hover out while
//!   the next row lights at once.
//! - A change of the element's own base colours is applied at once: what the
//!   application set (a selection, a theme) is never eased behind its back.
//! - A transition changed half way starts from the colour it shows, so a
//!   quick in and out never jumps.
//! - Under [`App::reduce_motion`] every change is applied at once.
//! - Colours are mixed in sRGB with premultiplied alpha, as CSS transitions
//!   mix them, so a fill fading to transparent keeps its hue. A gradient or
//!   pattern background is swapped, not mixed.
//!
//! An element without a transition pays for one empty pointer in its
//! interactivity and one test in its paint.
//!
//! [`App::reduce_motion`]: crate::App::reduce_motion

use crate::{App, Background, Fill, Hsla, Rgba, Style, StyleRefinement, Window};
use scheduler::Instant;
use std::{rc::Rc, time::Duration};

/// How an element's background and border colours follow it into and out of
/// its interaction states (hover, press, focus, drag over). See the
/// [module](self) for what is eased and when.
///
/// ```ignore
/// div()
///     .id("row")
///     .bg(rest)
///     .hover(|style| style.bg(hovered))
///     .active(|style| style.bg(pressed))
///     .transition(
///         StateTransition::new(Duration::from_millis(150))
///             .enter(Duration::ZERO)
///             .with_easing(ease_out_quint()),
///     )
/// ```
#[derive(Clone)]
pub struct StateTransition {
    enter: Duration,
    exit: Duration,
    easing: Rc<dyn Fn(f32) -> f32>,
}

impl StateTransition {
    /// A transition taking `duration` each way, eased linearly.
    pub fn new(duration: Duration) -> Self {
        Self {
            enter: duration,
            exit: duration,
            easing: Rc::new(crate::linear),
        }
    }

    /// How long a change into a state, or between two, takes. Zero applies
    /// it at once.
    pub fn enter(mut self, duration: Duration) -> Self {
        self.enter = duration;
        self
    }

    /// How long the return to the element's rest takes. Zero applies it at
    /// once.
    pub fn exit(mut self, duration: Duration) -> Self {
        self.exit = duration;
        self
    }

    /// The easing, mapping the time gone, from 0 to 1, to how far the colours
    /// have moved. Its output is clamped to 0..=1.
    pub fn with_easing(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Rc::new(easing);
        self
    }
}

impl std::fmt::Debug for StateTransition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateTransition")
            .field("enter", &self.enter)
            .field("exit", &self.exit)
            .finish_non_exhaustive()
    }
}

/// Sets `transition` on an element's interactivity. See
/// [`crate::StatefulInteractiveElement::transition`].
#[inline]
pub(crate) fn set(interactivity: &mut crate::Interactivity, transition: StateTransition) {
    interactivity.fast_transition = crate::fast::interactivity::rare(transition);
}

/// The two colours a transition follows.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Colors {
    background: Option<Fill>,
    border: Option<Hsla>,
}

impl Colors {
    fn of_style(style: &Style) -> Self {
        Self {
            background: style.background.clone(),
            border: style.border_color,
        }
    }

    fn of_refinement(refinement: &StyleRefinement) -> Self {
        Self {
            background: refinement.background.clone(),
            border: refinement.border_color,
        }
    }

    fn apply(&self, style: &mut Style) {
        style.background.clone_from(&self.background);
        style.border_color = self.border;
    }

    /// `self` moved `t` of the way to `to`, or `None` where they can't be
    /// mixed.
    fn mix(&self, to: &Self, t: f32) -> Option<Self> {
        let background = match (&self.background, &to.background) {
            (None, None) => None,
            (from, to) => {
                let from = solid(from.as_ref())?;
                let to = solid(to.as_ref())?;
                Some(Fill::Color(Background::from(mix(from, to, t))))
            }
        };
        let border = match (self.border, to.border) {
            (None, None) => None,
            (from, to) => Some(mix(from.unwrap_or_default(), to.unwrap_or_default(), t)),
        };
        Some(Self { background, border })
    }
}

/// A fill's colour if it is a solid one, transparent if there is none.
fn solid(fill: Option<&Fill>) -> Option<Hsla> {
    match fill {
        None => Some(Hsla::default()),
        Some(Fill::Color(background)) => background.as_solid(),
    }
}

/// `from` moved `t` of the way to `to`, in sRGB with premultiplied alpha.
fn mix(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let (from, to) = (Rgba::from(from), Rgba::from(to));
    let a = from.a + (to.a - from.a) * t;
    if a <= 0. {
        return Hsla::default();
    }
    let channel = |from_c: f32, to_c: f32| {
        let premultiplied = from_c * from.a + (to_c * to.a - from_c * from.a) * t;
        (premultiplied / a).clamp(0., 1.)
    };
    Hsla::from(Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a,
    })
}

/// Where an element's transition stands, kept in its element state.
#[derive(Debug)]
pub(crate) struct TransitionState {
    /// The element's base colours when last painted.
    rest: Colors,
    /// The colours the current transition started from.
    from: Colors,
    /// The colours its styles ask for.
    to: Colors,
    started: Instant,
    duration: Duration,
}

impl TransitionState {
    fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.;
        }
        let gone = now.saturating_duration_since(self.started);
        (gone.as_secs_f32() / self.duration.as_secs_f32()).min(1.)
    }

    /// The colours shown at `now`.
    fn shown(&self, now: Instant, easing: &dyn Fn(f32) -> f32) -> Colors {
        let progress = self.progress(now);
        if progress >= 1. {
            return self.to.clone();
        }
        self.from
            .mix(&self.to, easing(progress).clamp(0., 1.))
            .unwrap_or_else(|| self.to.clone())
    }
}

/// `style` with the colours the element's transition shows, asking for the
/// next frame while they move: the paint hook of [`crate::Interactivity`].
#[inline]
pub(crate) fn paint(
    interactivity: &crate::Interactivity,
    mut style: Style,
    element_state: &mut Option<crate::InteractiveElementState>,
    window: &mut Window,
    cx: &App,
) -> Style {
    if let (Some(transition), Some(element_state)) =
        (interactivity.fast_transition.as_deref(), element_state)
    {
        follow_transition(
            transition,
            &interactivity.base_style,
            &mut element_state.fast_transition,
            &mut style,
            window,
            cx,
        );
    }
    style
}

fn follow_transition(
    transition: &StateTransition,
    base: &StyleRefinement,
    state: &mut crate::fast::interactivity::Rare<TransitionState>,
    style: &mut Style,
    window: &mut Window,
    cx: &App,
) {
    let now = cx.background_executor().now();
    let target = Colors::of_style(style);
    let rest = Colors::of_refinement(base);
    let Some(state) = state.as_deref_mut() else {
        *state = crate::fast::interactivity::rare(TransitionState {
            rest,
            from: target.clone(),
            to: target,
            started: now,
            duration: Duration::ZERO,
        });
        return;
    };
    if state.rest != rest || cx.reduce_motion() {
        *state = TransitionState {
            rest,
            from: target.clone(),
            to: target,
            started: now,
            duration: Duration::ZERO,
        };
        return;
    }
    if state.to != target {
        let shown = state.shown(now, &*transition.easing);
        state.duration = if target == state.rest {
            transition.exit
        } else {
            transition.enter
        };
        state.from = shown;
        state.to = target;
        state.started = now;
    }
    if state.progress(now) >= 1. {
        return;
    }
    state.shown(now, &*transition.easing).apply(style);
    window.request_animation_frame();
}

#[cfg(test)]
mod tests {
    use super::{Colors, mix};
    use crate::{Background, Fill, Hsla, hsla, linear_color_stop, linear_gradient};

    /// A fill fading to transparent, or to no fill, keeps its hue on the way:
    /// mixed premultiplied, transparent black adds no black.
    #[test]
    fn a_fill_fading_out_keeps_its_hue() {
        let red = hsla(0., 1., 0.5, 1.);
        let half = mix(red, Hsla::default(), 0.5);
        assert!((half.a - 0.5).abs() < 1e-4);
        assert!((half.l - 0.5).abs() < 1e-3 && (half.s - 1.).abs() < 1e-3);

        let filled = Colors {
            background: Some(Fill::Color(Background::from(red))),
            border: Some(red),
        };
        let mixed = filled
            .mix(&Colors::default(), 0.5)
            .expect("solid fills mix");
        let background = mixed.background.and_then(|fill| fill.color()?.as_solid());
        assert_eq!(background, Some(half));
        assert_eq!(mixed.border, Some(half));
    }

    /// A gradient is swapped, not mixed.
    #[test]
    fn a_gradient_does_not_mix() {
        let stops = (
            linear_color_stop(hsla(0., 1., 0.5, 1.), 0.),
            linear_color_stop(hsla(0.5, 1., 0.5, 1.), 1.),
        );
        let gradient = Colors {
            background: Some(Fill::Color(linear_gradient(0., stops.0, stops.1))),
            border: None,
        };
        assert_eq!(gradient.mix(&Colors::default(), 0.5), None);
    }
}
