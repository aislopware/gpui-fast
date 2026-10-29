//! Where a line may break, found only when it has to wrap.
//!
//! Line breaks follow UAX #14 (zed #64624), which costs about 8 ns a byte:
//! three times what measuring the line costs. A line that fits its wrap width
//! breaks nowhere, so these skip the segmenter unless something on the line
//! passes the width, and read ASCII as Latin-1, which the segmenter walks a
//! fifth faster than UTF-8 and which breaks at the same places.

use crate::{
    CharWidthCache, FontId, LineFragment, LineLayout, LineWrapper, Pixels, TextSystem, px,
    width_for_char,
};

/// The break opportunities [`LineWrapper::wrap_line`] weighs in `text`, the
/// fragments joined, or none when the fragments fit `wrap_width`. The widths
/// are read from, and added to, the wrapper's `char_widths`.
pub(crate) fn unless_fragments_fit<'t>(
    char_widths: &mut CharWidthCache,
    text_system: &TextSystem,
    font_id: FontId,
    font_size: Pixels,
    fragments: &[LineFragment],
    text: &'t str,
    wrap_width: Pixels,
) -> impl Iterator<Item = usize> + use<'t> {
    let mut width = px(0.);
    let mut overflows = false;
    'fragments: for fragment in fragments {
        match fragment {
            LineFragment::Text { text } => {
                for character in text.chars().filter(|&character| character != '\n') {
                    width +=
                        width_for_char(char_widths, character, text_system, font_id, font_size);
                    if width > wrap_width {
                        overflows = true;
                        break 'fragments;
                    }
                }
            }
            LineFragment::Element {
                width: element_width,
                ..
            } => width += *element_width,
        }
        if width > wrap_width {
            overflows = true;
            break;
        }
    }
    breaks(text, overflows)
}

/// The break opportunities `LineLayout::compute_wrap_boundaries` weighs in
/// `text`, or none when no glyph and not the line's end passes `wrap_width`.
pub(crate) fn unless_layout_fits<'t>(
    layout: &LineLayout,
    text: &'t str,
    wrap_width: Pixels,
) -> impl Iterator<Item = usize> + use<'t> {
    let overflows = layout.width > wrap_width
        || layout
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .any(|glyph| glyph.position.x > wrap_width);
    breaks(text, overflows)
}

/// Where UAX #14 lets `text` break.
#[cfg(test)]
pub(crate) fn break_indices(text: &str) -> impl Iterator<Item = usize> + use<'_> {
    breaks(text, true)
}

/// Where UAX #14 lets `text` break when `overflows`, and nowhere otherwise.
fn breaks(text: &str, overflows: bool) -> impl Iterator<Item = usize> + use<'_> {
    let ascii = overflows && text.is_ascii();
    let latin1 = ascii.then(|| {
        icu_segmenter::LineSegmenter::new_for_non_complex_scripts(Default::default())
            .segment_latin1(text.as_bytes())
    });
    let utf8 = (overflows && !ascii).then(|| LineWrapper::break_indices(text));
    latin1
        .into_iter()
        .flatten()
        .chain(utf8.into_iter().flatten())
}
