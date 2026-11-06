//! Transformed and blurred glyphs: one glyph raster scaled, rotated or
//! moved as it is composited, and blurred by a gaussian, as rolling text
//! blurs each digit while it slides in.
//!
//! [`Window::paint_glyph_transformed`] paints a monochrome sprite with the
//! caller's transformation, which the renderers already apply to sprites
//! (an SVG's rotation) and scroll layers already carry. The blur's standard
//! deviation, in device pixels, rides in the sprite's tile's `padding` as
//! the bits of an `f32`: no atlas sets that field, and a sprite of upstream's
//! layout then holds the blur without a byte more, where zui adds a field
//! to every glyph sprite and to each renderer's mirror of it. Metal widens
//! a blurred sprite's quad by three standard deviations a side and taps the
//! tile with the gaussian around each pixel; elsewhere the glyph is drawn
//! sharp. Scroll layers count the widened quad as what the sprite draws
//! ([`crate::fast::layers::scene::drawn_bounds`]), and a stretch holding a
//! blurred glyph is painted again rather than moved.
//!
//! The API, the sampling and the kernel are ported from zui
//! (<https://github.com/zeronsh/zui>, `0966d06`, "Transformed and blurred
//! monochrome glyphs"; Apache-2.0, by Wing; zui is a fork of Zed's GPUI).

use std::borrow::Cow;

use anyhow::Result;

use crate::{
    Bounds, FontId, GlyphId, Hsla, IsZero as _, MonochromeSprite, Pixels, Point, RenderGlyphParams,
    ScaledPixels, TransformationMatrix, Window,
};

/// The blur's standard deviation a sprite is drawn with, in device pixels:
/// 0 for one drawn sharp.
pub(crate) fn sigma(sprite: &MonochromeSprite) -> f32 {
    let sigma = f32::from_bits(sprite.tile.padding);
    if sigma.is_finite() && sigma > 0. {
        sigma
    } else {
        0.
    }
}

/// What a sprite draws, before its transformation: its bounds, widened by
/// three standard deviations a side when it is blurred.
pub(crate) fn drawn_bounds(sprite: &MonochromeSprite) -> Bounds<ScaledPixels> {
    let sigma = sigma(sprite);
    if sigma == 0. {
        return sprite.bounds;
    }
    sprite.bounds.dilate(ScaledPixels(3. * sigma))
}

impl Window {
    /// Paints a monochrome glyph as [`Window::paint_glyph`] does, without
    /// subpixel rendering, through `transformation`, applied to device
    /// pixels as it is composited (scale or rotate it about any point
    /// without rasterizing it again), and blurred by a gaussian of standard
    /// deviation `blur`; a blur of 0 draws it sharp.
    ///
    /// Only Metal blurs the glyph; elsewhere it is drawn sharp. This method
    /// should only be called as part of the paint phase of element drawing.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_glyph_transformed(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Hsla,
        transformation: TransformationMatrix,
        blur: Pixels,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);
        let (integer_origin, subpixel_variant) = crate::fast::glyphs::quantize_origin(glyph_origin);
        let dilation = crate::fast::text_smoothing::dilation(self, color);
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant,
            scale_factor,
            is_emoji: false,
            subpixel_rendering: false,
            dilation,
        };
        let raster_bounds = self.text_system().raster_bounds(&params)?;
        if raster_bounds.is_zero() {
            return Ok(());
        }
        let mut tile = self
            .sprite_atlas
            .get_or_insert_with(params.clone().into(), &mut || {
                let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                Ok(Some((size, Cow::Owned(bytes))))
            })?
            .expect("Callback above only errors or returns Some");
        let sigma = blur.0 * scale_factor;
        tile.padding = if sigma.is_finite() && sigma > 0. {
            sigma.to_bits()
        } else {
            0
        };
        let bounds = Bounds {
            origin: integer_origin + raster_bounds.origin.map(Into::into),
            size: tile.bounds.size.map(Into::into),
        };
        let content_mask = self.snapped_content_mask();
        self.next_frame.scene.insert_primitive(MonochromeSprite {
            order: 0,
            pad: 0,
            bounds,
            content_mask,
            color: color.opacity(element_opacity),
            tile,
            transformation,
        });
        Ok(())
    }
}
