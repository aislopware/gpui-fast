//! The SVG renderer's font questions, asked through skrifa (fontations), the font reader
//! resvg 0.48 shapes its text with, so GPUI reads fonts with one library rather than a second.

/// Whether the face at `index` in `data` maps `ch` to a glyph.
pub(crate) fn font_has_char(data: &[u8], index: u32, ch: char) -> bool {
    use skrifa::MetadataProvider as _;
    skrifa::FontRef::from_index(data, index)
        .ok()
        .and_then(|font| font.charmap().map(ch))
        .is_some()
}
