//! Keypad keys named apart from the main keys (zed #63402, widened from the
//! digits to the whole keypad): `kp0` to `kp9`, `kpadd`, `kpsubtract`,
//! `kpmultiply`, `kpdivide`, `kpdecimal`, `kpequal` and `kpenter`, spelled as
//! XKB's `KP_*` keysyms are. A keymap can bind the keypad on its own, and a
//! terminal or a remote desktop can send what was pressed.
//!
//! A keypad key still does what its main twin did when the two shared a name:
//! a binding written for `5`, `cmd-+` or `enter` matches it, and so does a
//! focused element's activation on enter. A binding for the keypad key itself
//! outranks the twin's only by the keymap's usual order (context depth, then
//! later added).

use crate::{KeybindingKeystroke, Keystroke};

/// The main key a keypad key stands in for, or `None` for any other key.
pub(crate) fn main_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "kp0" => "0",
        "kp1" => "1",
        "kp2" => "2",
        "kp3" => "3",
        "kp4" => "4",
        "kp5" => "5",
        "kp6" => "6",
        "kp7" => "7",
        "kp8" => "8",
        "kp9" => "9",
        "kpadd" => "+",
        "kpsubtract" => "-",
        "kpmultiply" => "*",
        "kpdivide" => "/",
        "kpdecimal" => ".",
        "kpequal" => "=",
        "kpenter" => "enter",
        _ => return None,
    })
}

/// Whether the key is enter, the main one or the keypad's.
pub(crate) fn is_enter(keystroke: &Keystroke) -> bool {
    matches!(keystroke.key.as_str(), "enter" | "kpenter")
}

/// Whether a typed keypad key matches a binding written for its main twin.
pub(crate) fn matches_main_key(typed: &Keystroke, target: &KeybindingKeystroke) -> bool {
    main_key(&typed.key).is_some_and(|main| {
        target.inner().modifiers == typed.modifiers && target.inner().key == main
    })
}

/// What a simulated keypad key types, or `None` for any other key.
pub(crate) fn simulated_char(key: &str) -> Option<String> {
    main_key(key).map(|main| match main {
        "enter" => "\n".to_string(),
        main => main.to_string(),
    })
}
