//! The current platform's text system, made without the platform.

use gpui::PlatformTextSystem;
use std::sync::Arc;

/// Returns the current platform's text system without making the platform.
///
/// On macOS and iOS the platform has to be made on the main thread, which a
/// test never runs on, while their text systems (Core Text) may be made and
/// used on any thread. There this builds the text system alone, as the
/// platform builds it: Core Text through font-kit on macOS (GPUI's no-op text
/// system without the `font-kit` feature, as the platform then has), Core
/// Text on iOS. Elsewhere the text system comes with its platform's devices
/// and is taken from a headless platform.
pub fn text_system() -> Arc<dyn PlatformTextSystem> {
    #[cfg(all(target_os = "macos", feature = "font-kit"))]
    {
        gpui_macos::text_system()
    }

    #[cfg(all(target_os = "macos", not(feature = "font-kit")))]
    {
        Arc::new(gpui::NoopTextSystem::new())
    }

    #[cfg(target_os = "ios")]
    {
        gpui_ios::text_system()
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        crate::current_platform(true).text_system()
    }
}

#[cfg(all(test, target_os = "macos", feature = "font-kit"))]
mod tests {
    use super::text_system;
    use gpui::{FontRun, RenderGlyphParams, font, point, px};

    /// Made on a thread that is not the main one, where the platform panics,
    /// the text system resolves a system font, shapes a line in it and
    /// rasterises one of its glyphs into ink.
    #[test]
    fn the_text_system_is_made_and_used_off_the_main_thread() {
        std::thread::spawn(|| {
            let text = text_system();
            let font_id = text
                .font_id(&font("Menlo"))
                .expect("Menlo is a system font");
            let line = text.layout_line("gpui", px(16.), &[FontRun { font_id, len: 4 }]);
            let glyphs: Vec<_> = line.runs.iter().flat_map(|run| &run.glyphs).collect();
            assert_eq!(glyphs.len(), 4, "one glyph a letter");
            assert!(line.width > px(0.), "the line has a width");
            let params = RenderGlyphParams {
                font_id,
                glyph_id: glyphs[0].id,
                font_size: px(16.),
                subpixel_variant: point(0, 0),
                scale_factor: 2.,
                is_emoji: false,
                subpixel_rendering: false,
                dilation: 0,
            };
            let bounds = text
                .glyph_raster_bounds(&params)
                .expect("the glyph's bounds");
            assert!(
                bounds.size.width.0 > 0 && bounds.size.height.0 > 0,
                "a letter has ink"
            );
            let (size, pixels) = text.rasterize_glyph(&params, bounds).expect("a raster");
            assert_eq!(size, bounds.size);
            assert!(
                pixels.iter().any(|&alpha| alpha != 0),
                "the raster holds the ink"
            );
        })
        .join()
        .expect("the thread made and used the text system");
    }
}
