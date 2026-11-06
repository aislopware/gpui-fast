//! Which input sources compose: an input method's printable keys go to it before any binding.
//!
//! Upstream counts an input source as composing when it is an input method's mode and cannot
//! type ASCII (Japanese, Chinese, Korean). An input method that types Latin letters composes as
//! well: the Vietnamese ones (Telex, Simple Telex, VNI, VIQR) are modes of `VietnameseIM` that
//! type ASCII, and turn `tieesng` into `tiếng` only if they see the keys. Sent to the view's
//! bindings first, each key was taken there (a terminal takes every key), and nothing was ever
//! composed. A plain keyboard layout (ABC, U.S.) is no input method and is unchanged.

/// Whether an input source composes text from the keys it is given: any input method's mode,
/// whether or not it types ASCII.
pub(crate) fn composes(is_input_mode: bool, _is_ascii_capable: bool) -> bool {
    is_input_mode
}

#[cfg(test)]
mod tests {
    /// Vietnamese Simple Telex (an ASCII-capable input mode) composes, as Japanese (one that is
    /// not) does; the ABC layout does not.
    #[test]
    fn a_latin_input_method_composes() {
        assert!(super::composes(true, true), "Simple Telex");
        assert!(super::composes(true, false), "Japanese");
        assert!(!super::composes(false, true), "ABC");
    }
}
