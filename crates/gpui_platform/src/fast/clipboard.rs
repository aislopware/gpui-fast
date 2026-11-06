//! Naming the pasteboard GPUI's clipboard uses, on the platforms that have
//! named pasteboards.

/// Makes GPUI's clipboard (`App::write_to_clipboard`, `App::read_from_clipboard`)
/// the pasteboard named `name` in place of the general one, for the rest of the
/// process or until named again: macOS's `NSPasteboard pasteboardWithName:`,
/// iOS's `UIPasteboard pasteboardWithName:create:`. Call it before the first
/// copy, so that no copy reaches the general pasteboard. An application that
/// shares its clipboard through a pasteboard of its own names it here, so what
/// it copies is what it shares, and a test keeps off the person's clipboard.
pub fn set_clipboard_pasteboard(name: &str) {
    #[cfg(target_os = "macos")]
    gpui_macos::set_clipboard_pasteboard(name);

    #[cfg(target_os = "ios")]
    gpui_ios::set_clipboard_pasteboard(name);
}
