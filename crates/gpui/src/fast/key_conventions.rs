//! The keyboard conventions an operating system's keystrokes follow.
//!
//! Upstream gives the Mac's conventions to macOS alone: `secondary-` is ⌘
//! there and control everywhere else, and only there are keys shown as
//! symbols. A hardware keyboard on an iPad follows the Mac's: ⌘ is the key
//! its shortcuts use, and iPadOS lists them with the Mac's symbols. So a
//! keystroke on iOS is parsed, matched and shown as on a Mac.

use crate::OperatingSystem;

/// The operating system whose keyboard conventions `os` follows: the Mac's
/// for iOS, its own for any other.
pub(crate) fn of(os: OperatingSystem) -> OperatingSystem {
    match os {
        OperatingSystem::Ios => OperatingSystem::Mac,
        os => os,
    }
}

#[cfg(test)]
mod tests {
    use crate::{Keystroke, OperatingSystem};

    #[test]
    fn ios_keystrokes_follow_the_mac() {
        let keystroke =
            Keystroke::parse_with_os("secondary-shift-left", OperatingSystem::Ios).expect("parses");
        assert!(keystroke.modifiers.platform && !keystroke.modifiers.control);
        assert_eq!(keystroke.display(OperatingSystem::Ios).to_string(), "⌘⇧←");
        let linux =
            Keystroke::parse_with_os("secondary-left", OperatingSystem::Linux).expect("parses");
        assert!(linux.modifiers.control && !linux.modifiers.platform);
    }
}
