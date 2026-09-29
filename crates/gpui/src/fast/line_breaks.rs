//! Where a line may break, found only when it has to wrap.
//!
//! Line breaks follow UAX #14 (zed #64624), which costs about 8 ns a byte:
//! three times what measuring the line costs. A line that fits its wrap width
//! breaks nowhere, so these skip the segmenter unless something on the line
//! passes the width, and read ASCII as Latin-1, which the segmenter walks a
//! fifth faster than UTF-8 and which breaks at the same places.

use crate::{LineFragment, LineLayout, LineWrapper, Pixels, px};

/// The break opportunities [`LineWrapper::wrap_line`] weighs in `text`, the
/// fragments joined, or none when the fragments fit `wrap_width`.
pub(crate) fn unless_fragments_fit(
    wrapper: &mut LineWrapper,
    fragments: &[LineFragment],
    text: &str,
    wrap_width: Pixels,
) -> Vec<usize> {
    let mut width = px(0.);
    let mut overflows = false;
    'fragments: for fragment in fragments {
        match fragment {
            LineFragment::Text { text } => {
                for character in text.chars().filter(|&character| character != '\n') {
                    width += wrapper.width_for_char(character);
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
    if overflows {
        break_indices(text)
    } else {
        Vec::new()
    }
}

/// The break opportunities `LineLayout::compute_wrap_boundaries` weighs in
/// `text`, or none when no glyph and not the line's end passes `wrap_width`.
pub(crate) fn unless_layout_fits(
    layout: &LineLayout,
    text: &str,
    wrap_width: Pixels,
) -> Vec<usize> {
    let overflows = layout.width > wrap_width
        || layout
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .any(|glyph| glyph.position.x > wrap_width);
    if overflows {
        break_indices(text)
    } else {
        Vec::new()
    }
}

/// Where UAX #14 lets `text` break.
pub(crate) fn break_indices(text: &str) -> Vec<usize> {
    if text.is_ascii() {
        icu_segmenter::LineSegmenter::new_for_non_complex_scripts(Default::default())
            .segment_latin1(text.as_bytes())
            .collect()
    } else {
        LineWrapper::break_indices(text).collect()
    }
}
