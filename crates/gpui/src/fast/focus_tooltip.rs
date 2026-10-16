//! An element's tooltip shown to the keyboard as well as to the pointer.
//!
//! Upstream shows a tooltip only while the pointer rests on its element, and
//! no element counts as hovered once the last input was the keyboard, so a
//! control reached with Tab never names itself. Here an element that has a
//! tooltip and tracks a focus handle also shows its tooltip while it is
//! focus-visible: focused, with the keyboard the last input, as
//! `focus_visible` styles are drawn.
//!
//! - It shows after the element's tooltip delay, as a hover's does.
//! - It sits centred below the element, or above it where the window has no
//!   room below, rather than at the pointer.
//! - It hides when the focus leaves the element or the pointer is used.
//! - Escape hides it and keeps the focus where it is, before the key reaches
//!   any binding, so a dialog's Escape closes the hint first. It stays hidden
//!   until the focus leaves the element or the pointer is used.
//!
//! An element without a tooltip, or without a focus handle, never reaches
//! this module beyond one test in its paint.
//!
//! The module also places hover tooltips and warms them up:
//!
//! - An element, or the window for all its elements, can anchor a hover
//!   tooltip to the element rather than to the pointer
//!   ([`TooltipPlacement`]): centred below it or above it, 4 px off,
//!   flipping where the window has no room, so the pointer never covers the
//!   text it asked for.
//! - Once a tooltip has shown, the next one shows at once while a tooltip is
//!   still up or hid less than [`TOOLTIP_WARM_FOR`] ago, as Base UI's and
//!   Radix's provider delays do: running the pointer along a toolbar reads
//!   each button without waiting each time.

use crate::{
    ActiveTooltip, AnyTooltip, AnyView, App, Bounds, EntityId, FocusHandle, Pixels, Subscription,
    TooltipId, Window, point, px,
};
use scheduler::Instant;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

/// The space between an element and the tooltip its focus shows.
const GAP: Pixels = px(4.);

/// What an element's tooltip needs to follow its focus, kept in its element
/// state.
#[derive(Default)]
pub(crate) struct FocusTooltip {
    /// Escape hid the tooltip while the element kept the focus.
    dismissed: Cell<bool>,
    /// The view of the tooltip shown for the focus, while one is.
    shown: Cell<Option<EntityId>>,
    /// Hides the tooltip on Escape before any binding sees the key, held
    /// while it shows.
    escape: RefCell<Option<Subscription>>,
}

/// Where a hover tooltip sits ([`crate::StatefulInteractiveElement::tooltip_placement`],
/// [`Window::set_tooltip_placement`]). A tooltip a focus shows always sits below its element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipPlacement {
    /// Just below and right of the pointer, as upstream places it.
    #[default]
    Pointer,
    /// Centred below its element, or above it where the window has no room below.
    Below,
    /// Centred above its element, or below it where the window has no room above.
    Above,
}

/// How soon after a tooltip hides the next one skips its delay.
pub const TOOLTIP_WARM_FOR: Duration = Duration::from_millis(400);

/// Where the tooltip shown this frame is anchored, how the window places hover tooltips, and
/// when one last showed, kept by the window.
#[derive(Default)]
pub(crate) struct WindowFocusTooltip {
    anchor: Option<(TooltipId, Bounds<Pixels>, TooltipPlacement)>,
    /// The placement of a hover tooltip whose element names none.
    pub(crate) placement: TooltipPlacement,
    /// A tooltip was drawn in the last frame.
    showing: bool,
    /// When the last tooltip stopped being drawn.
    hidden_at: Option<Instant>,
}

/// The delay a hover tooltip waits now: none while the window is warm (a tooltip is up or hid
/// less than [`TOOLTIP_WARM_FOR`] ago), else `delay`.
pub(crate) fn warm_delay(window: &Window, cx: &App, delay: Duration) -> Duration {
    let state = &window.fast_focus_tooltip;
    let warm = state.showing
        || state
            .hidden_at
            .is_some_and(|at| cx.background_executor().now() < at + TOOLTIP_WARM_FOR);
    if warm { Duration::ZERO } else { delay }
}

/// Notes whether the frame being drawn shows a tooltip, so the window knows when the last one
/// hid ([`warm_delay`]).
pub(crate) fn note_drawn(window: &mut Window, cx: &App, drawn: bool) {
    let state = &mut window.fast_focus_tooltip;
    if state.showing && !drawn {
        state.hidden_at = Some(cx.background_executor().now());
    }
    state.showing = drawn;
}

type Active = Rc<RefCell<Option<ActiveTooltip>>>;
type BuildTooltip = Rc<dyn Fn(&mut Window, &mut App) -> Option<(AnyView, bool)>>;

fn focus_visible(handle: &FocusHandle, window: &Window) -> bool {
    // The focus is asked first, so a retained view records the read either
    // way.
    handle.is_focused(window) && window.last_input_was_keyboard()
}

/// Whether the tooltip `active` holds is the one `state` showed for the
/// focus.
fn is_shown(state: &FocusTooltip, active: &Active) -> bool {
    let Some(shown) = state.shown.get() else {
        return false;
    };
    matches!(
        active.borrow().as_ref(),
        Some(ActiveTooltip::Visible { tooltip, .. }) if tooltip.view.entity_id() == shown
    )
}

fn hide(state: &FocusTooltip, active: &Active, window: &mut Window) {
    if is_shown(state, active) {
        crate::clear_active_tooltip(active, window);
    }
    state.shown.set(None);
}

/// Shows the element's tooltip, after its delay, while it is focus-visible:
/// the paint hook of an element with a tooltip.
#[allow(
    clippy::too_many_arguments,
    reason = "the element's parts, passed apart"
)]
pub(crate) fn paint(
    focus_handle: Option<&FocusHandle>,
    active: &Active,
    state: &mut Option<Rc<FocusTooltip>>,
    build: BuildTooltip,
    show_delay: Option<Duration>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(handle) = focus_handle else {
        return;
    };
    let state = state.get_or_insert_with(Default::default);
    if !is_shown(state, active) {
        state.shown.set(None);
        state.escape.take();
    }
    if !focus_visible(handle, window) {
        state.dismissed.set(false);
        return;
    }
    if state.dismissed.get() || active.borrow().is_some() {
        return;
    }
    let delay = show_delay.unwrap_or(crate::DEFAULT_TOOLTIP_SHOW_DELAY);
    let task = window.spawn(cx, {
        let (active, state) = (Rc::downgrade(active), Rc::downgrade(state));
        let handle = handle.clone();
        async move |cx| {
            cx.background_executor().timer(delay).await;
            let (Some(active), Some(state)) = (active.upgrade(), state.upgrade()) else {
                return;
            };
            cx.update(|window, cx| show(&active, &state, &handle, &build, bounds, window, cx))
                .ok();
        }
    });
    *active.borrow_mut() = Some(ActiveTooltip::WaitingForShow { _task: task });
}

fn show(
    active: &Active,
    state: &Rc<FocusTooltip>,
    handle: &FocusHandle,
    build: &BuildTooltip,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    if !focus_visible(handle, window) || state.dismissed.get() {
        active.borrow_mut().take();
        return;
    }
    let Some((view, is_hoverable)) = build(window, cx) else {
        active.borrow_mut().take();
        return;
    };
    state.shown.set(Some(view.entity_id()));
    let check_visible_and_update = Rc::new({
        let (active, state) = (Rc::downgrade(active), Rc::downgrade(state));
        let handle = handle.clone();
        move |_: Bounds<Pixels>, window: &mut Window, _: &mut App| {
            let (Some(active), Some(state)) = (active.upgrade(), state.upgrade()) else {
                return false;
            };
            let visible = !state.dismissed.get() && focus_visible(&handle, window);
            if !visible {
                hide(&state, &active, window);
            }
            visible
        }
    });
    *active.borrow_mut() = Some(ActiveTooltip::Visible {
        tooltip: AnyTooltip {
            view,
            // Where it is placed until the element's next prepaint anchors
            // it.
            mouse_position: point(bounds.left(), bounds.bottom()),
            check_visible_and_update,
        },
        is_hoverable,
    });
    let escape = cx.intercept_keystrokes({
        let (active, state) = (Rc::downgrade(active), Rc::downgrade(state));
        let window_handle = window.window_handle();
        move |event, window, cx| {
            let keystroke = &event.keystroke;
            if keystroke.key != "escape"
                || keystroke.modifiers.modified()
                || window.window_handle() != window_handle
            {
                return;
            }
            let (Some(active), Some(state)) = (active.upgrade(), state.upgrade()) else {
                return;
            };
            if is_shown(&state, &active) {
                state.dismissed.set(true);
                hide(&state, &active, window);
                cx.stop_propagation();
            }
        }
    });
    *state.escape.borrow_mut() = Some(escape);
    window.refresh();
}

/// Anchors the tooltip the element shows to the element's `bounds` this
/// frame: below it for its focus, else where `placement` (or the window's)
/// says for a hover: the prepaint hook of an element with an active tooltip.
pub(crate) fn anchor(
    state: &Option<Rc<FocusTooltip>>,
    active: &Active,
    id: Option<TooltipId>,
    bounds: Bounds<Pixels>,
    placement: Option<TooltipPlacement>,
    window: &mut Window,
) {
    let Some(id) = id else {
        return;
    };
    if state
        .as_deref()
        .is_some_and(|state| is_shown(state, active))
    {
        window.fast_focus_tooltip.anchor = Some((id, bounds, TooltipPlacement::Below));
        return;
    }
    let placement = placement.unwrap_or(window.fast_focus_tooltip.placement);
    if placement != TooltipPlacement::Pointer {
        window.fast_focus_tooltip.anchor = Some((id, bounds, placement));
    }
}

/// Places the tooltip `id` centred on the element it was anchored to, below
/// or above it as anchored, on the other side where the window has no room,
/// keeping it inside the window. A tooltip at the pointer keeps the place
/// upstream gave it.
pub(crate) fn place(window: &Window, id: TooltipId, tooltip: &mut Bounds<Pixels>) {
    let Some((anchor_id, anchor, placement)) = window.fast_focus_tooltip.anchor else {
        return;
    };
    if anchor_id != id {
        return;
    }
    let viewport = window.viewport_size();
    let size = tooltip.size;
    let x = (anchor.center().x - size.width / 2.)
        .min(viewport.width - size.width)
        .max(Pixels::ZERO);
    let below = anchor.bottom() + GAP;
    let above = anchor.top() - GAP - size.height;
    let fits_below = below + size.height <= viewport.height;
    let fits_above = above >= Pixels::ZERO;
    let y = match placement {
        TooltipPlacement::Above if !fits_above && fits_below => below,
        TooltipPlacement::Above => above.max(Pixels::ZERO),
        _ if fits_below => below,
        _ => above.max(Pixels::ZERO),
    };
    tooltip.origin = point(x, y);
}
