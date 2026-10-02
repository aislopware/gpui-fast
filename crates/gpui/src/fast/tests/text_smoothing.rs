//! Tests of the text smoothing policy: which dilation glyphs are rasterised
//! with, and that glyphs drawn under one policy never show another's raster.

use std::{borrow::Cow, sync::Arc};

use parking_lot::Mutex;

use crate::{
    App, AppContext, AtlasTile, Bounds, Context, DevicePixels, Font, FontId, FontMetrics, FontRun,
    GlyphId, Hsla, IntoElement, LineLayout, NoopTextSystem, ParentElement, Pixels,
    PlatformTextSystem, Render, RenderGlyphParams, Result, Rgba, ShapedGlyph, ShapedRun, Size,
    Styled, TestAppContext, TextRenderingMode, TextSmoothing, Window, WindowHandle, canvas, div,
    point, px, size,
};

const WHITE: Hsla = Hsla {
    h: 0.,
    s: 0.,
    l: 1.,
    a: 1.,
};
const BLACK: Hsla = Hsla {
    h: 0.,
    s: 0.,
    l: 0.,
    a: 1.,
};

/// The no-op text system, except that every glyph rasterises to a small box,
/// colours are dilated by their luminance as on macOS, and every glyph
/// rasterised is noted with its dilation.
#[derive(Default)]
struct DilatingTextSystem {
    rasterised: Mutex<Vec<(GlyphId, u8)>>,
}

impl DilatingTextSystem {
    fn take_rasterised(&self) -> Vec<(GlyphId, u8)> {
        let mut rasterised = std::mem::take(&mut *self.rasterised.lock());
        rasterised.sort_by_key(|&(glyph, dilation)| (glyph.0, dilation));
        rasterised
    }
}

impl PlatformTextSystem for DilatingTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        NoopTextSystem.add_fonts(fonts)
    }

    fn all_font_names(&self) -> Vec<String> {
        NoopTextSystem.all_font_names()
    }

    fn font_id(&self, descriptor: &Font) -> Result<FontId> {
        NoopTextSystem.font_id(descriptor)
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        NoopTextSystem.font_metrics(font_id)
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        NoopTextSystem.typographic_bounds(font_id, glyph_id)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        NoopTextSystem.advance(font_id, glyph_id)
    }

    fn glyph_for_char(&self, _: FontId, ch: char) -> Option<GlyphId> {
        Some(glyph(ch))
    }

    fn glyph_raster_bounds(&self, _: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        Ok(Bounds {
            origin: point(DevicePixels(0), DevicePixels(-8)),
            size: size(DevicePixels(5), DevicePixels(9)),
        })
    }

    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        self.rasterised
            .lock()
            .push((params.glyph_id, params.dilation));
        let area = raster_bounds.size.width.0 * raster_bounds.size.height.0;
        Ok((raster_bounds.size, vec![u8::MAX; area as usize]))
    }

    /// A glyph per character, each 8 pixels on from the last, on a line
    /// whose baseline falls on a whole pixel: every glyph is drawn at the
    /// same subpixel variant, so glyphs differ only by what they are and how
    /// they are dilated.
    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        let glyphs = text
            .char_indices()
            .enumerate()
            .map(|(n, (index, ch))| ShapedGlyph {
                id: glyph(ch),
                position: point(px(8. * n as f32), px(0.)),
                index,
                is_emoji: false,
            })
            .collect();
        LineLayout {
            font_size,
            width: px(8. * text.chars().count() as f32),
            ascent: px(12.),
            descent: px(4.),
            runs: vec![ShapedRun {
                font_id: runs[0].font_id,
                glyphs,
            }],
            len: text.len(),
        }
    }

    fn recommended_rendering_mode(&self, _: FontId, _: Pixels) -> TextRenderingMode {
        TextRenderingMode::Grayscale
    }

    fn glyph_dilation_for_color(&self, color: Hsla) -> u8 {
        let rgba: Rgba = color.into();
        let luminance = 0.2126 * rgba.r + 0.7152 * rgba.g + 0.0722 * rgba.b;
        ((4. * luminance + 0.5).floor() as i32).clamp(0, 4) as u8
    }
}

/// The same word in white and in black, laid out as text.
struct Words;

impl Render for Words {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .text_size(px(16.))
            .line_height(px(20.))
            .child(div().text_color(WHITE).child("ab"))
            .child(div().text_color(BLACK).child("ab"))
    }
}

fn setup() -> (TestAppContext, Arc<DilatingTextSystem>) {
    let text_system = Arc::new(DilatingTextSystem::default());
    let cx = TestAppContext::with_text_system(text_system.clone());
    (cx, text_system)
}

/// Draws a frame and returns its glyph sprites' tiles, in paint order.
fn draw<V: 'static>(cx: &mut TestAppContext, window: WindowHandle<V>) -> Vec<AtlasTile> {
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window
            .rendered_frame
            .scene
            .monochrome_sprites
            .iter()
            .map(|sprite| sprite.tile)
            .collect()
    })
    .unwrap()
}

fn glyph(ch: char) -> GlyphId {
    GlyphId(ch as u32)
}

/// Under the platform's smoothing a white glyph is rasterised dilated and a
/// black one is not. Antialiased, no glyph is dilated, so white text shows
/// the black text's raster and nothing is rasterised again; and a window
/// drawn before the policy changed is drawn again under the new one rather
/// than from what it kept.
#[test]
fn antialiased_text_is_never_dilated_and_shares_one_raster_across_colours() {
    let (mut cx, text_system) = setup();
    assert_eq!(cx.update(|cx| cx.text_smoothing()), TextSmoothing::Native);
    let window = cx.add_window(|_, _| Words);

    let native = draw(&mut cx, window);
    let (a, b) = (glyph('a'), glyph('b'));
    assert_eq!(
        text_system.take_rasterised(),
        [(a, 0), (a, 4), (b, 0), (b, 4)],
        "white dilated to the top level, black not at all"
    );
    let [white_a, white_b, black_a, black_b] = native.as_slice() else {
        panic!("four glyphs, got {}", native.len());
    };
    assert_ne!(white_a.tile_id, black_a.tile_id);
    assert_ne!(white_b.tile_id, black_b.tile_id);

    cx.update(|cx| cx.set_text_smoothing(TextSmoothing::Antialiased));
    assert_eq!(
        cx.update(|cx| cx.text_smoothing()),
        TextSmoothing::Antialiased
    );
    let antialiased = draw(&mut cx, window);
    assert_eq!(
        text_system.take_rasterised(),
        [],
        "every colour takes the raster black already has"
    );
    let tiles: Vec<_> = antialiased.iter().map(|tile| tile.tile_id).collect();
    assert_eq!(
        tiles,
        [
            black_a.tile_id,
            black_b.tile_id,
            black_a.tile_id,
            black_b.tile_id
        ]
    );

    cx.update(|cx| cx.set_text_smoothing(TextSmoothing::Native));
    let again = draw(&mut cx, window);
    assert_eq!(
        text_system.take_rasterised(),
        [],
        "the dilated rasters were kept"
    );
    assert_eq!(
        again.iter().map(|tile| tile.tile_id).collect::<Vec<_>>(),
        native.iter().map(|tile| tile.tile_id).collect::<Vec<_>>()
    );
}

/// Glyphs an element paints inside `with_text_smoothing` follow its policy,
/// whichever the application's is, through both of GPUI's glyph paths, and
/// the policy outside is back once the scope ends.
#[test]
fn a_scoped_smoothing_overrides_the_applications() {
    struct Painted;

    impl Render for Painted {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            canvas(
                |_, _, _| {},
                |bounds, _, window, _| {
                    let at = bounds.origin + point(px(0.), px(12.));
                    let paint = |window: &mut Window, ch| {
                        window
                            .paint_glyph(at, FontId(0), glyph(ch), px(12.), WHITE)
                            .unwrap();
                        window
                            .paint_glyph_scaled(at, FontId(0), glyph(ch), px(18.), px(12.), WHITE)
                            .unwrap();
                    };
                    paint(window, 'a');
                    let outer = window.text_smoothing();
                    let inner = match outer {
                        TextSmoothing::Native => TextSmoothing::Antialiased,
                        TextSmoothing::Antialiased => TextSmoothing::Native,
                    };
                    window.with_text_smoothing(inner, |window| {
                        assert_eq!(window.text_smoothing(), inner);
                        paint(window, 'b');
                    });
                    assert_eq!(window.text_smoothing(), outer);
                },
            )
            .size_full()
        }
    }

    let (mut cx, text_system) = setup();
    let window = cx.add_window(|_, _| Painted);
    let (a, b) = (glyph('a'), glyph('b'));

    draw(&mut cx, window);
    assert_eq!(
        text_system.take_rasterised(),
        [(a, 4), (a, 4), (b, 0), (b, 0)],
        "the application's smoothing dilates, the scope's does not"
    );

    cx.update(|cx: &mut App| cx.set_text_smoothing(TextSmoothing::Antialiased));
    draw(&mut cx, window);
    assert_eq!(
        text_system.take_rasterised(),
        [(a, 0), (a, 0), (b, 4), (b, 4)],
        "the scope dilates, the application's smoothing does not"
    );
}
