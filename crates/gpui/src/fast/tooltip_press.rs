//! A press on an element puts its tooltip away until the pointer leaves it.
//!
//! Upstream hides a visible tooltip on a mouse-down, but a show still waiting
//! out its delay survives the press and then shows what its element said
//! before the click: a toggle's tooltip read "Expand side pane" on a button
//! the click had just made "Restore side pane". And once the button is up, the
//! next move over the element shows the tooltip again, at once while the
//! window is warm, over the thing the click opened. AppKit, Base UI and Radix
//! close a tooltip on the press and show it again only after the pointer has
//! left and come back; so does this.

use crate::{ActiveTooltip, Hitbox, MouseDownEvent, MouseMoveEvent, Window};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// Watches presses on an element with a tooltip: a mouse-down on it drops
/// the tooltip, shown or waiting, and until the pointer has left the element
/// any show a move schedules is dropped again. Registered before upstream's
/// tooltip listeners, so its bubble-phase move listener runs after theirs.
pub(crate) fn watch(
    pressed: &Rc<Cell<bool>>,
    active: &Rc<RefCell<Option<ActiveTooltip>>>,
    hitbox: &Hitbox,
    window: &mut Window,
) {
    window.on_mouse_event({
        let (active, pressed, hitbox) = (active.clone(), pressed.clone(), hitbox.clone());
        move |_: &MouseDownEvent, phase, window, _| {
            if phase.capture() && hitbox.is_hovered(window) {
                pressed.set(true);
                crate::clear_active_tooltip(&active, window);
            }
        }
    });
    window.on_mouse_event({
        let (active, pressed, hitbox) = (active.clone(), pressed.clone(), hitbox.clone());
        move |_: &MouseMoveEvent, phase, window, _| {
            if !phase.bubble() || !pressed.get() {
                return;
            }
            if !hitbox.is_hovered(window) {
                pressed.set(false);
            } else if matches!(*active.borrow(), Some(ActiveTooltip::WaitingForShow { .. })) {
                active.borrow_mut().take();
            }
        }
    });
}
