//! Keypad keys named apart from the main keys (zed #63402, widened from the
//! digits to the whole keypad), by their `kVK_ANSI_Keypad*` key codes from
//! HIToolbox's `Events.h`. See `gpui::fast::keypad` for the names and how a
//! binding for the main key still matches.

use crate::events::{NO_MOD, SHIFT_MOD, chars_for_modified_key, read_modifiers};
use cocoa::{appkit::NSEvent, base::id};
use gpui::Keystroke;

/// The name of the keypad key with this virtual key code, or `None` for a
/// key off the keypad. The clear key (0x47) keeps its old name.
pub(crate) fn key_name(key_code: u16) -> Option<&'static str> {
    Some(match key_code {
        0x41 => "kpdecimal",
        0x43 => "kpmultiply",
        0x45 => "kpadd",
        0x4b => "kpdivide",
        0x4c => "kpenter",
        0x4e => "kpsubtract",
        0x51 => "kpequal",
        0x52 => "kp0",
        0x53 => "kp1",
        0x54 => "kp2",
        0x55 => "kp3",
        0x56 => "kp4",
        0x57 => "kp5",
        0x58 => "kp6",
        0x59 => "kp7",
        0x5b => "kp8",
        0x5c => "kp9",
        _ => return None,
    })
}

/// The keystroke a keypad key makes, or `None` for a key off the keypad.
/// Enter carries a newline, as the main enter key does; the other keys carry
/// what they type unless a chord modifier is held, as zed #63402 has it.
///
/// # Safety
///
/// `native_event` is an `NSEvent` of a key-down or key-up type, whose
/// `keyCode` AppKit defines only for key events.
pub(crate) unsafe fn keystroke(native_event: id) -> Option<Keystroke> {
    // SAFETY: a key event, as the caller guarantees.
    let key_code = unsafe { native_event.keyCode() };
    let key = key_name(key_code)?;
    // SAFETY: an `NSEvent`, as the caller guarantees. Its function flag needs
    // none of `parse_keystroke`'s care for the arrow keys' private-use
    // characters, which no keypad key types.
    let modifiers = unsafe { read_modifiers(native_event) };
    let key_char = if key == "kpenter" {
        Some("\n".to_string())
    } else if modifiers.control || modifiers.platform || modifiers.function || modifiers.alt {
        None
    } else {
        let shift = if modifiers.shift { SHIFT_MOD } else { NO_MOD };
        Some(chars_for_modified_key(key_code, shift))
    };
    Some(Keystroke {
        modifiers,
        key: key.to_string(),
        key_char,
    })
}

#[cfg(test)]
mod tests {
    use crate::events::{NO_MOD, chars_for_modified_key, platform_input_from_native};
    use cocoa::{
        appkit::{NSEvent, NSEventModifierFlags, NSEventType},
        base::{NO, nil},
        foundation::NSPoint,
    };
    use gpui::{Keystroke, PlatformInput};

    fn key_down(key_code: u16, characters: &str, flags: NSEventModifierFlags) -> Keystroke {
        // SAFETY: AppKit builds a key event from plain values, without a
        // window or a running application; the strings are retained by it.
        let event = unsafe {
            let characters = crate::ns_string(characters);
            NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode_(
                nil,
                NSEventType::NSKeyDown,
                NSPoint::new(0., 0.),
                flags,
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

    const NUMERIC_PAD: NSEventModifierFlags = NSEventModifierFlags::NSNumericPadKeyMask;

    /// One test, because the Text Input Sources calls that spell a key abort
    /// the process when two threads make them at once. What a key types comes
    /// from the current keyboard layout, so it is compared with the layout's
    /// answer rather than with a US keyboard's.
    #[test]
    fn keypad_keys_are_named_apart_from_the_main_keys() {
        for (key_code, characters, key) in [
            (0x52, "0", "kp0"),
            (0x53, "1", "kp1"),
            (0x57, "5", "kp5"),
            (0x5b, "8", "kp8"),
            (0x5c, "9", "kp9"),
            (0x41, ".", "kpdecimal"),
            (0x43, "*", "kpmultiply"),
            (0x45, "+", "kpadd"),
            (0x4b, "/", "kpdivide"),
            (0x4e, "-", "kpsubtract"),
            (0x51, "=", "kpequal"),
        ] {
            let keystroke = key_down(key_code, characters, NUMERIC_PAD);
            assert_eq!(keystroke.key, key, "{key_code:#x}");
            let typed = chars_for_modified_key(key_code, NO_MOD);
            assert!(!typed.is_empty(), "{key_code:#x}");
            assert_eq!(keystroke.key_char, Some(typed), "{key_code:#x}");
            assert!(!keystroke.modifiers.modified(), "{key_code:#x}");
        }
        let enter = key_down(0x4c, "\u{3}", NUMERIC_PAD);
        assert_eq!(
            (enter.key.as_str(), enter.key_char.as_deref()),
            ("kpenter", Some("\n"))
        );

        for main_key in [0x12, 0x18, 0x24] {
            let keystroke = key_down(main_key, "", NSEventModifierFlags::empty());
            assert!(!keystroke.key.starts_with("kp"), "{main_key:#x}");
        }
        assert_eq!(
            key_down(0x24, "\r", NSEventModifierFlags::empty()).key,
            "enter"
        );
    }

    #[test]
    fn a_chord_on_the_keypad_keeps_the_keypad_name_and_types_nothing() {
        let keystroke = key_down(
            0x53,
            "1",
            NUMERIC_PAD | NSEventModifierFlags::NSCommandKeyMask,
        );
        assert_eq!(keystroke.key, "kp1");
        assert!(keystroke.modifiers.platform);
        assert_eq!(keystroke.key_char, None);
    }
}
