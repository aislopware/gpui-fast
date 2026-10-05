//! Masks a caller rasterises, painted at exact device pixels
//! ([`Window::paint_mask`]).
//!
//! [`Window::paint_svg`] rasterises at twice the device size and lets the
//! sampler halve it, which suits a drawing made on its own grid. A system
//! symbol is drawn by the operating system at the size it is shown, and
//! scaling it after costs crispness, most at 1x. A mask is the caller's
//! bytes, one alpha byte per device pixel, painted as they are.

use std::borrow::Cow;

use anyhow::Result;

use crate::{
    AtlasKey, AtlasTextureKind, Bounds, DevicePixels, Hsla, MonochromeSprite, Pixels, Point,
    ScaledPixels, SharedString, Size, TransformationMatrix, Window, util::round_half_toward_zero,
};

/// A monochrome mask its caller rasterises, at exact device pixels
/// ([`Window::paint_mask`]): an operating system's symbol, drawn by the
/// system at the size it is shown, never scaled after.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct RenderMaskParams {
    /// What the mask is, as its caller names it. It must tell apart every
    /// drawing of the same size: a symbol's name, weight and scale factor.
    pub key: SharedString,
    /// Its size in device pixels: one alpha byte per pixel, row by row.
    pub size: Size<DevicePixels>,
}

/// The texture a mask lands in: one channel, tinted when painted.
pub(crate) const TEXTURE_KIND: AtlasTextureKind = AtlasTextureKind::Monochrome;

impl From<RenderMaskParams> for AtlasKey {
    fn from(params: RenderMaskParams) -> Self {
        Self::Mask(params)
    }
}

impl Window {
    /// Paint a monochrome mask its caller rasterises, in `color`, its top
    /// left at `origin` rounded to a device pixel, at exactly `size` device
    /// pixels.
    ///
    /// Unlike [`Self::paint_svg`], nothing is supersampled or scaled: the
    /// mask's pixels are the screen's, as a system symbol drawn at the size
    /// it is shown wants. `rasterize` runs only when the atlas lacks `key`
    /// at `size`, and gives one alpha byte per device pixel, row by row;
    /// `None` paints nothing. `key` must tell apart every drawing of the
    /// same size, the scale factor included.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn paint_mask(
        &mut self,
        origin: Point<Pixels>,
        size: Size<DevicePixels>,
        key: SharedString,
        color: Hsla,
        rasterize: impl FnOnce() -> Result<Option<Vec<u8>>>,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();
        if size.width.0 <= 0 || size.height.0 <= 0 {
            return Ok(());
        }
        let element_opacity = self.element_opacity();
        let params = RenderMaskParams { key, size };
        let mut rasterize = Some(rasterize);
        let Some(tile) = self
            .sprite_atlas
            .get_or_insert_with(params.into(), &mut || {
                let Some(rasterize) = rasterize.take() else {
                    return Ok(None);
                };
                let Some(bytes) = rasterize()? else {
                    return Ok(None);
                };
                let wanted = size.width.0 as usize * size.height.0 as usize;
                anyhow::ensure!(
                    bytes.len() == wanted,
                    "a {}x{} mask takes {wanted} bytes, not {}",
                    size.width.0,
                    size.height.0,
                    bytes.len()
                );
                Ok(Some((size, Cow::Owned(bytes))))
            })?
        else {
            return Ok(());
        };
        let scale_factor = self.scale_factor();
        let origin = origin.scale(scale_factor);
        let bounds = Bounds {
            origin: Point::new(
                ScaledPixels(round_half_toward_zero(origin.x.0)),
                ScaledPixels(round_half_toward_zero(origin.y.0)),
            ),
            size: Size::new(
                ScaledPixels(size.width.0 as f32),
                ScaledPixels(size.height.0 as f32),
            ),
        };
        let content_mask = self.snapped_content_mask();
        self.next_frame.scene.insert_primitive(MonochromeSprite {
            order: 0,
            pad: 0,
            bounds,
            content_mask,
            color: color.opacity(element_opacity),
            tile,
            transformation: TransformationMatrix::unit(),
        });
        Ok(())
    }
}
