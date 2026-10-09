//! Native fonts made once per size and kept, so CoreText's shaping caches survive from line to line,
//! and faces matched on the weights AppKit gives them.

use collections::HashMap;
use core_text::font::CTFont;
use core_text::font_descriptor::TraitAccessors;
use font_kit::font::Font as FontKitFont;
use font_kit::properties::{Properties, Weight};
use gpui::{FontId, Pixels};

/// What a face is matched on: font-kit's properties, with the CSS weight
/// AppKit gives the face's Core Text weight in place of font-kit's
/// ([`crate::fast::font_weight`]), so Medium is 500 and Heavy 800.
pub(crate) fn properties(font: &FontKitFont) -> Properties {
    let weight = font.native_font().all_traits().normalized_weight();
    Properties {
        weight: Weight(crate::fast::font_weight::css_weight(weight)),
        ..font.properties()
    }
}

/// Each font at each size a line has been laid out at. CoreText keeps the
/// caches it builds for shaping — the advances and glyphs of ASCII among
/// them — on the font object, so a font made afresh for every line built
/// them again for every line.
#[derive(Default)]
pub(crate) struct SizedFonts(HashMap<(FontId, u32), CTFont>);

impl SizedFonts {
    /// The native font for `font`, whose id is `font_id`, at `font_size`, made
    /// once and kept.
    pub(crate) fn get(&mut self, font_id: FontId, font: &FontKitFont, font_size: Pixels) -> CTFont {
        // Sizes can animate, and every size a font is drawn at would otherwise
        // be kept for good.
        const MAX_SIZED_FONTS: usize = 1024;
        let key = (font_id, f32::from(font_size).to_bits());
        if let Some(font) = self.0.get(&key) {
            return font.clone();
        }
        if self.0.len() >= MAX_SIZED_FONTS {
            self.0.clear();
        }
        let font = font
            .native_font()
            .clone_with_font_size(f32::from(font_size).into());
        self.0.insert(key, font.clone());
        font
    }
}

#[cfg(test)]
mod tests {
    use crate::AppleTextSystem;
    use gpui::{
        Font, FontId, FontRun, FontWeight, LineLayout, PlatformTextSystem, RenderGlyphParams,
        TextRun, TextSystem, WindowTextSystem, font, point, px,
    };
    use std::sync::Arc;

    /// Every CSS weight from 100 to 900 reaches a face of its own in the
    /// system's UI family and in Avenir Next's, Medium at 500 and Heavy at 800
    /// among them, each inking more than the one below. 500 used to fall back
    /// to Regular and 800 to Black.
    #[test]
    fn each_css_weight_reaches_its_own_face() {
        let text = AppleTextSystem::new();
        for (family, weights) in [
            (
                ".SystemUIFont",
                &[100., 200., 300., 400., 500., 600., 700., 800., 900.][..],
            ),
            ("Avenir Next", &[100., 400., 500., 600., 700., 800.][..]),
        ] {
            let inked: Vec<(FontId, f64)> = weights
                .iter()
                .map(|&weight| {
                    let face = Font {
                        weight: FontWeight(weight),
                        ..font(family)
                    };
                    let font_id = text.font_id(&face).unwrap();
                    let glyph_id = text.glyph_for_char(font_id, 'n').unwrap();
                    let params = RenderGlyphParams {
                        font_id,
                        glyph_id,
                        font_size: px(24.),
                        subpixel_variant: point(0, 0),
                        scale_factor: 2.,
                        is_emoji: false,
                        subpixel_rendering: false,
                        dilation: 0,
                    };
                    let bounds = text.glyph_raster_bounds(&params).unwrap();
                    let (_, bytes) = text.rasterize_glyph(&params, bounds).unwrap();
                    (font_id, bytes.iter().map(|&b| f64::from(b)).sum())
                })
                .collect();
            for (pair, weight) in inked.windows(2).zip(&weights[1..]) {
                assert_ne!(
                    pair[0].0, pair[1].0,
                    "{family} {weight} has no face of its own"
                );
                assert!(
                    pair[1].1 > pair[0].1,
                    "{family} {weight} inks no more than the weight below"
                );
            }
        }
    }

    /// A line is shaped at the size it is asked for, so it is as wide as Core
    /// Text sets it with the font at exactly that size: at sizes on either
    /// side of 20 pt, where San Francisco changes how it tracks its letters.
    /// Every other run of a line, the first among them, used to be shaped a
    /// float step above its size to keep ligatures from joining runs, and
    /// Core Text tracks a size off the point differently: 0.11% wider at 20 pt
    /// and 0.1% narrower at 11.
    #[test]
    fn a_line_is_as_wide_as_core_text_sets_it_at_its_size() {
        use core_foundation::{
            attributed_string::CFMutableAttributedString,
            base::{CFRange, TCFType as _},
            string::CFString,
        };
        use core_text::{line::CTLine, string_attributes::kCTFontAttributeName};

        let fonts = AppleTextSystem::new();
        let font_id = fonts.font_id(&font(".SystemUIFont")).unwrap();
        let line = "Sphinx of black quartz, judge my vow";
        for size in [11., 13., 17., 19., 20., 21., 26., 34.] {
            let ours = fonts
                .layout_line(
                    line,
                    px(size),
                    &[FontRun {
                        font_id,
                        len: line.len(),
                    }],
                )
                .width;

            let native = core_text::font::new_from_name(".AppleSystemUIFont", f64::from(size))
                .expect("the system UI font");
            let mut string = CFMutableAttributedString::new();
            string.replace_str(&CFString::new(line), CFRange::init(0, 0));
            // SAFETY: the range is the whole string just written, and the value
            // is a CTFont, the type `kCTFontAttributeName` takes.
            unsafe {
                string.set_attribute(
                    CFRange::init(0, string.char_len()),
                    kCTFontAttributeName,
                    &native,
                );
            }
            let theirs = CTLine::new_with_attributed_string(string.as_concrete_TypeRef())
                .get_typographic_bounds()
                .width;
            assert_eq!(ours, gpui::Pixels::from(theirs), "at {size} pt");
        }
    }

    /// Fonts are made once per size and kept, so a line laid out with a kept
    /// font has to come out exactly as it did with a new one: the same glyphs
    /// in the same places, however many runs split it, and never mixed up
    /// with the same font at another size.
    #[test]
    fn lines_laid_out_with_kept_fonts_match_the_first_layout() {
        let fonts = AppleTextSystem::new();
        let font_id = fonts.font_id(&font("Helvetica")).unwrap();
        let line = "AVAWAY fi ffi 12.5 office";
        // Several runs of one font, which Core Text shapes as one.
        let runs = [5, 3, 4, 5, 8].map(|len| FontRun { font_id, len });
        let shape = |size| {
            let layout = fonts.layout_line(line, px(size), &runs);
            let glyphs = layout
                .runs
                .iter()
                .flat_map(|run| run.glyphs.iter().map(|glyph| (glyph.id, glyph.position)))
                .collect::<Vec<_>>();
            (layout.width, glyphs)
        };

        let first = shape(16.);
        assert_eq!(shape(16.), first, "a kept font shapes differently");
        let larger = shape(24.);
        assert_ne!(larger.0, first.0, "another size has to be another font");
        assert_eq!(shape(16.), first, "a size laid out again after another");
        assert_eq!(shape(24.), larger);
    }

    /// Numbers that GPUI puts together from their glyphs rather than asking
    /// CoreText (`fast::number_shaping`) come out as CoreText shapes them:
    /// the same glyphs, in the same places, in the system's UI and mono
    /// faces at every weight and size an interface draws numbers at, long
    /// after the first lines are checked against CoreText and the rest are
    /// trusted.
    #[test]
    fn numbers_put_together_match_coretext() {
        let platform: Arc<dyn PlatformTextSystem> = Arc::new(AppleTextSystem::new());
        let text_system = WindowTextSystem::new(Arc::new(TextSystem::new(platform.clone())));
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        let characters = b"0123456789.,+-%$KMBT";
        let mut numbers = vec![
            "0".to_owned(),
            "59.94".to_owned(),
            "120".to_owned(),
            "-0.25".to_owned(),
            "+1,234.56".to_owned(),
            "99.9%".to_owned(),
            "$1.2K".to_owned(),
            "3.4M".to_owned(),
            "11111".to_owned(),
            "1.1.1.1".to_owned(),
        ];
        for _ in 0..600 {
            let len = 1 + next(14);
            numbers.push(
                (0..len)
                    .map(|_| characters[next(characters.len())] as char)
                    .collect(),
            );
        }
        let glyphs = |layout: &LineLayout| {
            layout
                .runs
                .iter()
                .flat_map(|run| run.glyphs.iter().map(|glyph| (glyph.id, glyph.position)))
                .collect::<Vec<_>>()
        };
        let mut compared = 0;
        for family in [".SystemUIFont", "Menlo", "Monaco", "Helvetica Neue"] {
            for weight in [
                FontWeight::NORMAL,
                FontWeight::MEDIUM,
                FontWeight::SEMIBOLD,
                FontWeight::BOLD,
            ] {
                let face = Font {
                    weight,
                    ..font(family)
                };
                let font_id = platform
                    .font_id(&face)
                    .unwrap_or_else(|error| panic!("{family} {weight:?}: {error}"));
                for size in [9., 10., 11., 12., 13., 14., 16., 22.] {
                    for number in &numbers {
                        let ours = text_system.layout_line(
                            number,
                            px(size),
                            &[TextRun {
                                len: number.len(),
                                font: face.clone(),
                                ..TextRun::default()
                            }],
                            None,
                        );
                        let theirs = platform.layout_line(
                            number,
                            px(size),
                            &[FontRun {
                                font_id,
                                len: number.len(),
                            }],
                        );
                        let (ours_glyphs, theirs_glyphs) = (glyphs(&ours), glyphs(&theirs));
                        assert_eq!(
                            ours_glyphs.len(),
                            theirs_glyphs.len(),
                            "{family} {weight:?} {size} {number:?}"
                        );
                        for ((id, at), (their_id, their_at)) in
                            ours_glyphs.iter().zip(&theirs_glyphs)
                        {
                            assert_eq!(id, their_id, "{family} {weight:?} {size} {number:?}");
                            assert!(
                                (at.x - their_at.x).abs() < px(1e-3)
                                    && (at.y - their_at.y).abs() < px(1e-3),
                                "{family} {weight:?} {size} {number:?}: {at:?} against {their_at:?}"
                            );
                        }
                        assert!(
                            (ours.width - theirs.width).abs() < px(1e-3),
                            "{family} {weight:?} {size} {number:?}"
                        );
                        compared += 1;
                    }
                }
            }
        }
        assert_eq!(compared, 4 * 4 * 8 * numbers.len());
    }
}
