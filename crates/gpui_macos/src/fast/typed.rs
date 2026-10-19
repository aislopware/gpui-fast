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

use crate::NSStringExt;
use cocoa::{
    appkit::NSEvent,
    base::{id, nil},
};

/// The text a key-down types: the event's own characters when they are printable and differ
/// from `layout`, what the key code types on the current layout; else `layout`.
///
/// # Safety
///
/// `native_event` is an `NSEvent` of a key-down or key-up type.
pub(crate) unsafe fn key_char(native_event: id, layout: String) -> String {
    // SAFETY: a key event, as the caller guarantees; AppKit defines `characters` for key
    // events, and a non-nil `NSString` reads as UTF-8.
    let characters = unsafe {
        let characters = native_event.characters();
        if characters == nil {
            return layout;
        }
        characters.to_str().to_string()
    };
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
mod tests {
    use crate::events::{NO_MOD, chars_for_modified_key, platform_input_from_native};
    use cocoa::{
        appkit::{NSEvent, NSEventModifierFlags, NSEventType},
        base::{NO, nil},
        foundation::NSPoint,
    };
    use gpui::{Keystroke, PlatformInput};

    fn key_down(key_code: u16, characters: &str) -> Keystroke {
        let _spelling = super::SPELLING.lock();
        // SAFETY: AppKit builds a key event from plain values, without a window or a running
        // application; the strings are retained by it.
        let event = unsafe {
            let characters = crate::ns_string(characters);
            NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode_(
                nil,
                NSEventType::NSKeyDown,
                NSPoint::new(0., 0.),
                NSEventModifierFlags::empty(),
                0.,
                0,
                nil,
                characters,
                characters,
                NO,
                key_code,
            )
        };
        // SAFETY: `event` is the key event made above.
        match unsafe { platform_input_from_native(event, None) } {
            Some(PlatformInput::KeyDown(event)) => event.keystroke,
            other => panic!("{key_code:#x} made {other:?}"),
        }
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
