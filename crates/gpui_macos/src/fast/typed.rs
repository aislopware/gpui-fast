//! What a key event types: the characters AppKit gives the event, when they differ from what
//! its key code types on the current layout.
//!
//! `parse_keystroke` spells `key_char` from the key code through the layout, which is right for
//! a key pressed on the keyboard. An input method that works by posting events of its own
//! (XKey, EVKey and OpenKey for Vietnamese) instead posts a key-down on key code 0 carrying a
//! Unicode string (`CGEventKeyboardSetUnicodeString`): the layout spells that `a`, while the
//! event types `ê`. A view that takes keys itself (a terminal) then typed `a`. Caps Lock and a
//! dead key's composed letter (`é` after ⌥E) differ the same way, and the event's characters
//! are what the person typed there too.

use objc2_app_kit::NSEvent;

/// The text a key-down types: the event's own characters when they are printable and differ
/// from `layout`, what the key code types on the current layout; else `layout`. `native_event`
/// is an `NSEvent` of a key-down or key-up type, for which AppKit defines `characters`.
pub(crate) fn key_char(native_event: &NSEvent, layout: String) -> String {
    let Some(characters) = native_event.characters() else {
        return layout;
    };
    let characters = characters.to_string();
    if typed(&characters) && characters != layout {
        characters
    } else {
        layout
    }
}

/// Whether `characters` is text: not empty, no control character, and none of AppKit's
/// private-use function-key characters (U+F700–U+F8FF: arrows, F-keys, Home…).
fn typed(characters: &str) -> bool {
    !characters.is_empty()
        && characters
            .chars()
            .all(|c| !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&c))
}

/// Held by every test that spells a key: the Text Input Sources calls behind
/// `chars_for_modified_key` abort the process when two threads make them at once.
#[cfg(test)]
pub(crate) static SPELLING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[cfg(test)]
pub(crate) mod tests {
    use crate::events::{NO_MOD, chars_for_modified_key, platform_input_from_native};
    use gpui::{Keystroke, PlatformInput};
    use objc2::rc::Retained;
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::{NSPoint, NSString};

    /// A key-down of `key_code` carrying `characters`, as AppKit builds one from plain values,
    /// without a window or a running application, and what GPUI makes of it.
    pub(crate) fn key_down_with(
        key_code: u16,
        characters: &str,
        flags: NSEventModifierFlags,
    ) -> Keystroke {
        let characters = NSString::from_str(characters);
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::new(0., 0.),
            flags,
            0.,
            0,
            None,
            &characters,
            &characters,
            false,
            key_code,
        )
        .expect("a key event");
        // SAFETY: `event` is the live key event made above.
        match unsafe {
            platform_input_from_native(Retained::as_ptr(&event).cast_mut().cast(), None)
        } {
            Some(PlatformInput::KeyDown(event)) => event.keystroke,
            other => panic!("{key_code:#x} made {other:?}"),
        }
    }

    fn key_down(key_code: u16, characters: &str) -> Keystroke {
        let _spelling = super::SPELLING.lock();
        key_down_with(key_code, characters, NSEventModifierFlags::empty())
    }

    fn spelled(key_code: u16) -> String {
        let _spelling = super::SPELLING.lock();
        chars_for_modified_key(key_code, NO_MOD)
    }

    #[test]
    fn a_key_types_the_text_its_event_carries() {
        // XKey's `ê`, posted on key code 0 (the A key), types `ê`; the key stays the A key.
        let posted = key_down(0x00, "ê");
        assert_eq!(posted.key_char.as_deref(), Some("ê"));
        let layout = spelled(0x00);
        assert_eq!(posted.key, layout);
        // A chunk of text in one event types all of it.
        assert_eq!(key_down(0x00, "tiếng").key_char.as_deref(), Some("tiếng"));
        // A key pressed on the keyboard types what the layout says, as before.
        assert_eq!(key_down(0x00, &layout).key_char, Some(layout.clone()));
        // An event with no characters (a dead key) keeps the layout's.
        assert_eq!(key_down(0x00, "").key_char, Some(layout));
    }
}
