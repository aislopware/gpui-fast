//! Boxes laid out inside a line of text in place of a range of it: a formula, an image or a chip
//! inside a paragraph, which upstream's text cannot hold (its runs are glyphs only).
//!
//! The text keeps its bytes, so selecting, copying and assistive technologies read them, but the
//! line shows one unbroken box of the box's width where they were: the range's glyphs give way
//! to one blank glyph (its font's space) at its start, and every glyph after it moves by the
//! difference. The line's ascent and descent stay, so its height and baseline do, and since no
//! glyph but the blank is left inside a range, the line never wraps inside a box. The caller
//! paints the box, at [`TextLayout::baseline_for_index`] of its range's start.
//!
//! The text is shaped as it is without boxes, from the line layout cache, and the boxes are laid
//! out over that shape, so the cache keeps the text's own layout.

use std::ops::Range;

use parking_lot::Mutex;
use smallvec::SmallVec;

use crate::{
    FontId, GlyphId, LineLayout, Pixels, Point, ShapedGlyph, ShapedRun, SharedString, StyledText,
    TextLayout, TextRun, WindowTextSystem, WrappedLine, WrappedLineLayout, point, px,
};

/// A box laid out inside a line of text in place of a range of it.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineBox {
    /// The bytes of the text the box stands for.
    pub range: Range<usize>,
    /// How wide the box is.
    pub width: Pixels,
}

/// The boxes of the text a window's text system is shaping now, which its upstream
/// `shape_text` reads line by line through [`lay_out_boxes`].
#[derive(Default)]
pub(crate) struct Pending(Mutex<SmallVec<[InlineBox; 2]>>);

/// `text` shaped as `WindowTextSystem::shape_text` shapes it, with `boxes` (byte ranges of the
/// text, in order and apart) laid out in place of the text they stand for. A box that crosses a
/// line's end is left as text.
pub(crate) fn shape_text(
    text_system: &WindowTextSystem,
    text: SharedString,
    font_size: Pixels,
    runs: &[TextRun],
    wrap_width: Option<Pixels>,
    line_clamp: Option<usize>,
    boxes: &[InlineBox],
) -> anyhow::Result<SmallVec<[WrappedLine; 1]>> {
    if boxes.is_empty() {
        return text_system.shape_text(text, font_size, runs, wrap_width, line_clamp);
    }
    *text_system.fast_inline_boxes.0.lock() = boxes.iter().cloned().collect();
    let lines = text_system.shape_text(text, font_size, runs, wrap_width, line_clamp);
    text_system.fast_inline_boxes.0.lock().clear();
    lines
}

/// The line `layout` of `line_text`, which starts at byte `line_start` of the text being shaped,
/// with the boxes of that text that fall inside it laid out, and wrapped again as that; `layout`
/// itself when none do.
pub(crate) fn lay_out_boxes(
    text_system: &WindowTextSystem,
    layout: std::sync::Arc<WrappedLineLayout>,
    line_text: &str,
    line_start: usize,
    max_lines: Option<usize>,
) -> std::sync::Arc<WrappedLineLayout> {
    let line_end = line_start + line_text.len();
    let boxes: SmallVec<[InlineBox; 2]> = text_system
        .fast_inline_boxes
        .0
        .lock()
        .iter()
        .filter(|b| b.range.start >= line_start && b.range.end <= line_end)
        .map(|b| InlineBox {
            range: b.range.start - line_start..b.range.end - line_start,
            width: b.width,
        })
        .collect();
    if boxes.is_empty() {
        return layout;
    }
    let platform = &text_system.line_layout_cache.platform_text_system;
    let boxed = layout
        .unwrapped_layout
        .with_inline_boxes(&boxes, |font_id| platform.glyph_for_char(font_id, ' '));
    let wrap_boundaries = match layout.wrap_width {
        Some(wrap_width) => boxed.compute_wrap_boundaries(line_text, wrap_width, max_lines),
        None => SmallVec::new(),
    };
    std::sync::Arc::new(WrappedLineLayout {
        unwrapped_layout: std::sync::Arc::new(boxed),
        wrap_boundaries,
        wrap_width: layout.wrap_width,
    })
}

impl LineLayout {
    /// This line with `boxes` (byte ranges of it, in order and apart) laid out in place of the
    /// text they stand for. A range's glyphs give way to one blank glyph at its start, which
    /// `blank` names for the range's font (its space); the box is as wide as it says, and every
    /// glyph after it moves by the difference. A range with no glyph of its own, out of order,
    /// past the line, or whose font has no blank, is left as text.
    pub fn with_inline_boxes(
        &self,
        boxes: &[InlineBox],
        blank: impl Fn(FontId) -> Option<GlyphId>,
    ) -> LineLayout {
        // Each box kept, with its blank glyph, the x its text started at, and how much wider
        // than that text it is.
        let mut kept: SmallVec<[(Range<usize>, GlyphId, Pixels, Pixels); 2]> = SmallVec::new();
        let mut last_end = 0;
        for inline_box in boxes {
            let range = inline_box.range.clone();
            if range.start < last_end || range.is_empty() || range.end > self.len {
                continue;
            }
            let font = self.runs.iter().find_map(|run| {
                run.glyphs
                    .iter()
                    .any(|glyph| range.contains(&glyph.index))
                    .then_some(run.font_id)
            });
            let Some(id) = font.and_then(&blank) else {
                continue;
            };
            let start = self.x_for_index(range.start);
            let end = self.x_for_index(range.end);
            last_end = range.end;
            kept.push((range, id, start, inline_box.width - (end - start)));
        }

        // `passed` boxes lie wholly before the glyph at hand, and the glyphs move by their
        // differences, `shift`; `blanked` boxes have their blank.
        let mut passed = 0;
        let mut blanked = 0;
        let mut shift = px(0.);
        let mut runs = Vec::with_capacity(self.runs.len());
        for run in &self.runs {
            let mut glyphs = Vec::with_capacity(run.glyphs.len());
            for glyph in &run.glyphs {
                while let Some((range, _, _, difference)) = kept.get(passed)
                    && range.end <= glyph.index
                {
                    shift += *difference;
                    passed += 1;
                }
                if let Some((range, id, start, _)) = kept.get(passed)
                    && range.contains(&glyph.index)
                {
                    if blanked <= passed {
                        blanked = passed + 1;
                        glyphs.push(ShapedGlyph {
                            id: *id,
                            position: point(*start + shift, glyph.position.y),
                            index: range.start,
                            is_emoji: false,
                        });
                    }
                    continue;
                }
                glyphs.push(ShapedGlyph {
                    position: point(glyph.position.x + shift, glyph.position.y),
                    ..glyph.clone()
                });
            }
            runs.push(ShapedRun {
                font_id: run.font_id,
                glyphs,
            });
        }
        let widened = kept
            .iter()
            .fold(px(0.), |all, (_, _, _, difference)| all + *difference);
        LineLayout {
            font_size: self.font_size,
            width: self.width + widened,
            ascent: self.ascent,
            descent: self.descent,
            runs,
            len: self.len,
        }
    }
}

impl WrappedLineLayout {
    /// The position where the glyph at the given byte index is drawn, for the given line
    /// height: `position_for_index`, but an index where the line wraps is at the start of the
    /// next line, where its glyph is, rather than at the end of the one before.
    pub fn glyph_position_for_index(
        &self,
        index: usize,
        line_height: Pixels,
    ) -> Option<Point<Pixels>> {
        if index > self.len() {
            return None;
        }
        let mut line_start_ix = 0;
        let mut row = 0;
        for wrap_boundary in &self.wrap_boundaries {
            let line_end_ix = self.unwrapped_layout.runs[wrap_boundary.run_ix].glyphs
                [wrap_boundary.glyph_ix]
                .index;
            if index < line_end_ix {
                break;
            }
            line_start_ix = line_end_ix;
            row += 1;
        }
        let line_start_x = self.unwrapped_layout.x_for_index(line_start_ix);
        let x = self.unwrapped_layout.x_for_index(index) - line_start_x;
        Some(point(x, line_height * row as f32))
    }
}

impl StyledText {
    /// Lay `boxes` out in place of ranges of the text: each shows as one unbroken box of its
    /// width where its text would be, the text keeping its bytes for selection, copying and
    /// assistive technologies. The caller paints the boxes, at
    /// [`TextLayout::baseline_for_index`] of each range's start. The ranges must be in order,
    /// apart, on character boundaries and within one line of the text; any that are not stay
    /// text. Text that truncates lays out no boxes.
    pub fn with_inline_boxes(mut self, boxes: impl IntoIterator<Item = InlineBox>) -> Self {
        self.fast_inline_boxes = boxes.into_iter().collect();
        self
    }
}

/// Request the layout of a styled text, with its boxes: what `TextLayout::layout` does, and the
/// boxes taken from the element.
pub(crate) fn layout(
    layout: &TextLayout,
    text: SharedString,
    runs: Option<Vec<TextRun>>,
    boxes: &mut Vec<InlineBox>,
    window: &mut crate::Window,
    cx: &mut crate::App,
) -> crate::LayoutId {
    let boxes = std::mem::take(boxes);
    crate::fast::text::layout_text_with_boxes(layout, text, runs, boxes, window, cx)
}

impl TextLayout {
    /// The point on the baseline where the glyph at the given byte index is drawn: where a box
    /// laid out in place of text from there ([`StyledText::with_inline_boxes`]) sits, its ascent
    /// above. Where the line wraps at the index, that is the start of the next line;
    /// `position_for_index` says the end of the line before.
    pub fn baseline_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        let state = self.0.borrow();
        let state = state.as_ref()?;
        let bounds = state.bounds?;
        let line_height = state.line_height;
        let mut line_origin = bounds.origin;
        let mut line_start_ix = 0;
        for line in &state.lines {
            let line_end_ix = line_start_ix + line.len();
            if index > line_end_ix {
                line_origin.y += line.size(line_height).height;
                line_start_ix = line_end_ix + 1;
                continue;
            }
            let position = line.glyph_position_for_index(index - line_start_ix, line_height)?;
            let padding_top = (line_height - line.ascent() - line.descent()) / 2.;
            return Some(point(
                line_origin.x + position.x,
                line_origin.y + position.y + padding_top + line.ascent(),
            ));
        }
        None
    }
}
