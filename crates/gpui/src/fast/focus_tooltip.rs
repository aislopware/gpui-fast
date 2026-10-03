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

use crate::{
    ActiveTooltip, AnyTooltip, AnyView, App, Bounds, EntityId, FocusHandle, Pixels, Subscription,
    TooltipId, Window, point, px,
};
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

/// Where the tooltip a focus shows is anchored, kept by the window.
#[derive(Default)]
pub(crate) struct WindowFocusTooltip {
    anchor: Option<(TooltipId, Bounds<Pixels>)>,
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

/// Anchors the tooltip the element's focus shows to the element's `bounds`
/// this frame: the prepaint hook of an element with an active tooltip.
pub(crate) fn anchor(
    state: &Option<Rc<FocusTooltip>>,
    active: &Active,
    id: Option<TooltipId>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    if let (Some(state), Some(id)) = (state.as_deref(), id)
        && is_shown(state, active)
    {
        window.fast_focus_tooltip.anchor = Some((id, bounds));
    }
}

/// Places the tooltip `id` centred below the element its focus showed it
/// for, or above it where the window has no room below, keeping it inside
/// the window. Any other tooltip keeps the place upstream gave it.
pub(crate) fn place(window: &Window, id: TooltipId, tooltip: &mut Bounds<Pixels>) {
    let Some((anchor_id, anchor)) = window.fast_focus_tooltip.anchor else {
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
    let y = if below + size.height <= viewport.height {
        below
    } else {
        (anchor.top() - GAP - size.height).max(Pixels::ZERO)
    };
    tooltip.origin = point(x, y);
}
