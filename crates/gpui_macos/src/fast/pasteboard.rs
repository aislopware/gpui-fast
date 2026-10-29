//! The pasteboard GPUI's clipboard reads and writes: the general one, or the
//! one an application names.
//!
//! An application that shares its clipboard with something else (another
//! machine, a test that must never touch the person's clipboard) names a
//! pasteboard with [`set_clipboard_pasteboard`], and from then on
//! `App::write_to_clipboard` and `App::read_from_clipboard` go to that one.
//! The find pasteboard is never redirected.

use gpui::ClipboardItem;
use objc2_app_kit::NSPasteboard;
use objc2_foundation::NSString;
use parking_lot::Mutex;

use crate::pasteboard::Pasteboard;

/// The pasteboard the application named, if it named one.
static NAME: Mutex<Option<String>> = Mutex::new(None);

/// Makes GPUI's clipboard the pasteboard named `name` (`NSPasteboard
/// pasteboardWithName:`), made when first used, in place of the general
/// pasteboard, for the rest of the process or until named again. Call it
/// before the first copy, so that no copy reaches the general pasteboard.
pub fn set_clipboard_pasteboard(name: &str) {
    *NAME.lock() = Some(name.to_owned());
}

/// What the clipboard holds: the named pasteboard's item, or `general`'s
/// while none is named.
pub(crate) fn read(general: &Pasteboard) -> Option<ClipboardItem> {
    match named() {
        Some(board) => board.read(),
        None => general.read(),
    }
}

/// Puts `item` on the named pasteboard, or on `general` while none is named.
pub(crate) fn write(general: &Pasteboard, item: ClipboardItem) {
    match named() {
        Some(board) => board.write(item),
        None => general.write(item),
    }
}

/// The named pasteboard. `pasteboardWithName:` hands back the same pasteboard
/// for a name every time, so nothing is kept here.
fn named() -> Option<Pasteboard> {
    let name = NAME.lock().clone()?;
    Some(Pasteboard::named(&name))
}

impl Pasteboard {
    fn named(name: &str) -> Self {
        Self::new(NSPasteboard::pasteboardWithName(&NSString::from_str(name)))
    }
}

#[cfg(test)]
mod tests {
    use gpui::ClipboardItem;
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::NSString;

    use super::{read, set_clipboard_pasteboard, write};
    use crate::pasteboard::Pasteboard;

    /// The only test that names the clipboard, so no other sees it named. A
    /// pasteboard of its own stands in for the general one, which no test
    /// writes.
    #[test]
    fn a_named_clipboard_takes_every_copy_and_the_general_pasteboard_none() {
        let general = Pasteboard::unique();
        let before = ClipboardItem::new_string("before the name".to_string());
        write(&general, before.clone());
        assert_eq!(
            general.read(),
            Some(before.clone()),
            "unnamed, the general pasteboard"
        );

        let name = format!("dev.gpui-fast.test.clipboard.{}", std::process::id());
        set_clipboard_pasteboard(&name);
        let copied = ClipboardItem::new_string("copied".to_string());
        write(&general, copied.clone());
        assert_eq!(
            Pasteboard::named(&name).read(),
            Some(copied.clone()),
            "on the named one"
        );
        assert_eq!(
            general.read(),
            Some(before),
            "the general pasteboard keeps what it had"
        );
        assert_eq!(read(&general), Some(copied), "a paste reads the named one");

        let board = NSPasteboard::pasteboardWithName(&NSString::from_str(&name));
        // SAFETY: `releaseGlobally` takes no arguments and returns nothing.
        let () = unsafe { objc2::msg_send![&*board, releaseGlobally] };
    }
}
