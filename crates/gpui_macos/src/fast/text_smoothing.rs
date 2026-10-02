//! What the dilation `gpui::TextSmoothing` chooses does to Core Text's
//! glyphs: how much ink each level adds, and what rasterising costs.

#[cfg(test)]
mod tests {
    use crate::MacTextSystem;
    use gpui::{
        DevicePixels, Font, FontWeight, PlatformTextSystem, RenderGlyphParams, font, point, px,
    };
    use std::time::Instant;

    const SCALE: f32 = 2.;

    fn params(
        text: &MacTextSystem,
        face: &Font,
        ch: char,
        size: f32,
        dilation: u8,
    ) -> Option<RenderGlyphParams> {
        let font_id = text.font_id(face).ok()?;
        Some(RenderGlyphParams {
            font_id,
            glyph_id: text.glyph_for_char(font_id, ch)?,
            font_size: px(size),
            subpixel_variant: point(0, 0),
            scale_factor: SCALE,
            is_emoji: false,
            subpixel_rendering: false,
            dilation,
        })
    }

    /// The glyph's coverage, summed over its raster, in full pixels.
    fn ink(text: &MacTextSystem, params: &RenderGlyphParams) -> f64 {
        let bounds = text.glyph_raster_bounds(params).unwrap();
        if bounds.size.width == DevicePixels(0) || bounds.size.height == DevicePixels(0) {
            return 0.;
        }
        let (_, bytes) = text.rasterize_glyph(params, bounds).unwrap();
        bytes.iter().map(|&b| f64::from(b)).sum::<f64>() / 255.
    }

    /// Antialiased text (dilation 0) is the lighter raster: Core Graphics'
    /// smoothing at a light colour's level inks a stem past its outline, so
    /// the policy decides what weight light text shows.
    #[test]
    fn a_dilated_glyph_inks_more_than_an_antialiased_one() {
        let text = MacTextSystem::new();
        let menlo = font("Menlo");
        let at = |dilation| ink(&text, &params(&text, &menlo, 'l', 13., dilation).unwrap());
        let (antialiased, dilated) = (at(0), at(4));
        assert!(antialiased > 0., "Menlo's l rasterises");
        assert!(
            dilated > antialiased * 1.05,
            "dilation 4 inks {dilated:.1} px against {antialiased:.1} px antialiased"
        );
    }

    /// For each face an interface draws in, how much ink every dilation
    /// level adds to printable ASCII over dilation 0, and what rasterising a
    /// glyph costs at each level. The semibold face's own ink, against the
    /// regular's, sizes a level's gain in weights. Prints the numbers MEASUREMENTS records; run
    /// by hand, in release:
    /// `cargo test -p gpui_macos --release --lib measure_dilation -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement, run by hand"]
    fn measure_dilation() {
        const ROUNDS: usize = 31;
        let text = MacTextSystem::new();
        let faces = [
            (".SystemUIFont 13 regular", font(".SystemUIFont"), 13.),
            (
                ".SystemUIFont 13 semibold",
                Font {
                    weight: FontWeight::SEMIBOLD,
                    ..font(".SystemUIFont")
                },
                13.,
            ),
            (".SystemUIFont 15 regular", font(".SystemUIFont"), 15.),
            ("Menlo 13", font("Menlo"), 13.),
        ];
        for (name, face, size) in faces {
            let glyphs: Vec<char> = ('!'..='~').collect();
            let base: f64 = glyphs
                .iter()
                .filter_map(|&ch| params(&text, &face, ch, size, 0))
                .map(|p| ink(&text, &p))
                .sum();
            let mut line = format!(
                "MEASURE {name} at {SCALE}x, {} glyphs, {base:.0} px of ink antialiased:",
                glyphs.len()
            );
            for dilation in 0..=4 {
                let all: Vec<_> = glyphs
                    .iter()
                    .filter_map(|&ch| params(&text, &face, ch, size, dilation))
                    .collect();
                let inked: f64 = all.iter().map(|p| ink(&text, p)).sum();
                let mut rounds: Vec<f64> = (0..ROUNDS)
                    .map(|_| {
                        let started = Instant::now();
                        for p in &all {
                            let bounds = text.glyph_raster_bounds(p).unwrap();
                            if bounds.size.width.0 > 0 && bounds.size.height.0 > 0 {
                                std::hint::black_box(text.rasterize_glyph(p, bounds).unwrap());
                            }
                        }
                        started.elapsed().as_secs_f64() * 1e6 / all.len() as f64
                    })
                    .collect();
                rounds.sort_by(f64::total_cmp);
                line.push_str(&format!(
                    " | d{dilation}: ink {:+.1}%, {:.2} µs/glyph p50 ({:.2} p90)",
                    (inked / base - 1.) * 100.,
                    rounds[ROUNDS / 2],
                    rounds[ROUNDS * 9 / 10],
                ));
            }
            println!("{line}");
        }
    }
}
