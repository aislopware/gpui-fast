//! A finger on a touch screen hovers nothing.
//!
//! Hover listeners ([`crate::StatefulInteractiveElement::on_hover`]) ignore hover after keyboard
//! input; after a touch they heard the place the finger lifted from as hovered, and a row's hover
//! card opened under a tap on an iPad. Hover styles already show none after a touch; listeners now
//! agree, until the pointer moves again.

use crate::{Hitbox, Window};

/// Whether `hitbox` is hovered for an input-modality-aware hover listener: as
/// [`Hitbox::is_hovered`] says, never after a touch.
pub(crate) fn hovered(hitbox: &Hitbox, window: &Window) -> bool {
    !window.last_input_was_touch() && hitbox.is_hovered(window)
}
