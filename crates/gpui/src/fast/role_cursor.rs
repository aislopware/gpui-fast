//! The pointing hand over a control by its accessible role.
//!
//! An element given an interactive role ([`crate::StatefulInteractiveElement::role`]):
//! a button, a link, a tab, a menu item and the like, shows the pointing hand
//! under the pointer unless it asks for a cursor of its own, as a browser
//! shows one over a link or a `<button>` styled so. Such an element gets a
//! hitbox for it even when nothing else would give it one, so a control
//! marked only by its role still shows the hand.
//!
//! Ported from monocode (hardbeat920/monocode, its patched GPUI's
//! `Interactivity::role_cursor`), which onicode keeps pixel and behaviour
//! exact; there an element with a role alone had no hitbox and kept the arrow.

use crate::{CursorStyle, Style};

/// Gives `style`, an element's computed style, the cursor of its `role` when
/// it names none of its own in the state it is in: the element then gets a hitbox, and
/// shows the cursor over it.
pub(crate) fn fill(style: &mut Style, role: Option<accesskit::Role>) {
    if style.mouse_cursor.is_none() {
        style.mouse_cursor = of(role);
    }
}

/// The cursor `role` shows when its element names none: the pointing hand
/// for a role that is clicked, none for any other.
pub(crate) fn of(role: Option<accesskit::Role>) -> Option<CursorStyle> {
    use accesskit::Role;
    matches!(
        role?,
        Role::Button
            | Role::Link
            | Role::Tab
            | Role::ListBoxOption
            | Role::MenuItem
            | Role::MenuItemCheckBox
            | Role::MenuItemRadio
            | Role::CheckBox
            | Role::RadioButton
            | Role::Switch
            | Role::ComboBox
            | Role::TreeItem
    )
    .then_some(CursorStyle::PointingHand)
}
