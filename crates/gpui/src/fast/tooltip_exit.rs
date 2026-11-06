//! A tooltip fades out when its element lets it go, rather than vanishing.
//!
//! Upstream drops a tooltip the frame its element stops asking for it: the
//! pointer leaves, a press puts it away, the element goes. Radix, Base UI and
//! opencode give it an exit instead, a short ease-in fade that lets the eye
//! follow it off. This keeps the tooltip the last frame drew and, for
//! [`EXIT`] after its element let it go, draws it again where it was, fading.
//! A new tooltip ends the fade at once, and so does a prompt or a drag, which
//! draw where a tooltip would. Under reduced motion it vanishes as upstream's.

use std::time::Duration;

use scheduler::Instant;

use crate::{AnyElement, AnyView, App, AvailableSpace, IntoElement as _, Pixels, Point, Window};

/// How long a tooltip takes to fade out (opencode's 80 ms).
pub(crate) const EXIT: Duration = Duration::from_millis(80);

/// A window's tooltip on its way out.
#[derive(Default)]
pub(crate) struct TooltipExit {
    /// The tooltip the last frame drew, and where.
    shown: Option<(AnyView, Point<Pixels>)>,
    /// The one fading out, since when, and its opacity in the frame drawn.
    fading: Option<Fading>,
}

struct Fading {
    view: AnyView,
    origin: Point<Pixels>,
    started: Instant,
    opacity: f32,
}

/// After the window prepaints its tooltip: `drawn`, the one an element asks
/// for, is kept as the one shown; with none, the one shown last fades out in
/// its place, unless a prompt or a drag stands where it would draw.
pub(crate) fn prepaint(
    window: &mut Window,
    cx: &mut App,
    drawn: Option<AnyElement>,
) -> Option<AnyElement> {
    if drawn.is_some() {
        let shown = window.tooltip_bounds.as_ref().and_then(|bounds| {
            window
                .next_frame
                .tooltip_requests
                .iter()
                .flatten()
                .find(|request| request.id == bounds.id)
                .map(|request| (request.tooltip.view.clone(), bounds.bounds.origin))
        });
        let exit = &mut window.fast_tooltip_exit;
        exit.shown = shown;
        exit.fading = None;
        return drawn;
    }
    let now = crate::fast::frame_time::now(window, cx);
    let free = window.prompt.is_none() && cx.active_drag.is_none();
    let exit = &mut window.fast_tooltip_exit;
    let left = exit.shown.take();
    if !free || cx.reduce_motion() {
        exit.fading = None;
        return None;
    }
    if let Some((view, origin)) = left {
        exit.fading = Some(Fading {
            view,
            origin,
            started: now,
            opacity: 1.,
        });
    }
    let fading = exit.fading.as_mut()?;
    let t = now.saturating_duration_since(fading.started).as_secs_f32() / EXIT.as_secs_f32();
    if t >= 1. {
        exit.fading = None;
        return None;
    }
    // Ease-in: it leaves slowly, then all at once.
    fading.opacity = 1. - t * t;
    let (view, origin) = (fading.view.clone(), fading.origin);
    let mut element = view.into_any_element();
    element.layout_as_root(AvailableSpace::min_size(), window, cx);
    window.with_absolute_element_offset(origin, |window| element.prepaint(window, cx));
    // No view is being drawn here, so the next frame is asked of the window.
    window.on_next_frame(|window, _| window.refresh());
    Some(element)
}

/// Paints the tooltip [`prepaint`] gave, at its fade's opacity while fading.
pub(crate) fn paint(window: &mut Window, cx: &mut App, element: &mut AnyElement) {
    let exit = &window.fast_tooltip_exit;
    let opacity = exit
        .fading
        .as_ref()
        .filter(|_| exit.shown.is_none())
        .map(|f| f.opacity);
    window.with_element_opacity(opacity, |window| element.paint(window, cx));
}
