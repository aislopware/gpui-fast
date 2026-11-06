//! Image alpha masks: an image painted with a rounded rectangle feathered
//! out of it and its lower part faded, as a hero wallpaper clears the space
//! around a composer laid over it. Moving or resizing the mask changes only
//! what the shader is handed: not the image, its atlas tile, or anything
//! rasterized.
//!
//! The mask rides in the table of edge fades ([`crate::fast::edge_fade`]):
//! [`EdgeFadeRamps`]'s `mask`, `mask_shape` and `mask_bottom`, which every
//! Metal fragment shader applies with the fade and the rounded clip. The
//! image's sprite indexes the entry as a faded primitive does, so it adds
//! no byte to a sprite, and retention and moved stretches carry it as they
//! carry a fade. Only Metal draws it; elsewhere the image is drawn whole.
//!
//! The API and the mask's shape are ported from zui
//! (<https://github.com/zeronsh/zui>, `aa00941`, "Add paint-time rounded
//! image alpha masks"; Apache-2.0, by Gaël Cado; zui is a fork of Zed's
//! GPUI). Changed from zui: the mask is an entry of the fade table rather
//! than 40 bytes added to every image sprite, and it nests with the fades
//! and rounded clips around the image.

use std::sync::Arc;

use anyhow::Result;

use crate::{Bounds, Corners, EdgeFadeRamps, Pixels, RenderImage, ScaledPixels, Window};

/// Smoothly excludes a rounded rectangle from an image without changing
/// its texture. Coordinates are in window-space logical pixels,
/// independent of the image's.
#[derive(Clone, Copy, Debug)]
pub struct ImageAlphaMask {
    /// The rounded rectangle to exclude.
    pub bounds: Bounds<Pixels>,
    /// Corner radius, clamped to half the shorter side.
    pub radius: Pixels,
    /// Smoothstep distance outside the exclusion. Must be positive: a mask
    /// of no feather masks nothing.
    pub feather: Pixels,
    /// Extra transparent clearance outside the rounded rectangle.
    pub clearance: Pixels,
    /// Optional bottom edge (window y) and smoothstep fade-band height.
    pub bottom_fade: Option<(Pixels, Pixels)>,
}

impl ImageAlphaMask {
    /// The mask as the fade table holds it, in device pixels.
    fn ramps(&self, scale_factor: f32) -> EdgeFadeRamps {
        let (bottom_y, bottom_feather) = self.bottom_fade.unwrap_or_default();
        let bounds: Bounds<ScaledPixels> = self.bounds.scale(scale_factor);
        let shorter = f32::from(self.bounds.size.width)
            .min(f32::from(self.bounds.size.height))
            .max(0.);
        let finite = |value: f32| if value.is_finite() { value } else { 0. };
        let mut ramps = EdgeFadeRamps::NONE;
        ramps.mask = [
            bounds.left().0,
            bounds.top().0,
            bounds.right().0,
            bounds.bottom().0,
        ]
        .map(finite);
        ramps.mask_shape = [
            f32::from(self.radius).clamp(0., shorter * 0.5) * scale_factor,
            f32::from(self.feather).max(0.) * scale_factor,
            f32::from(self.clearance).max(0.) * scale_factor,
            f32::from(bottom_feather).max(0.) * scale_factor,
        ]
        .map(finite);
        ramps.mask_bottom = [finite(f32::from(bottom_y) * scale_factor), 0., 0., 0.];
        ramps
    }
}

impl Window {
    /// Paints the `visible` part of an image whose fitted box is `fitted`
    /// (equal or larger, as `ObjectFit::Cover` fits it), as
    /// [`Window::paint_image`] does, with `alpha_mask`, if any, feathered
    /// out of it: moving or resizing the mask reuses the image's atlas tile.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_image_fitted_masked(
        &mut self,
        visible: Bounds<Pixels>,
        fitted: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: Arc<RenderImage>,
        frame_index: usize,
        grayscale: bool,
        alpha_mask: Option<ImageAlphaMask>,
    ) -> Result<()> {
        let Some(mask) = alpha_mask.filter(|mask| mask.feather > Pixels::ZERO) else {
            return self.paint_image(visible, fitted, corner_radii, data, frame_index, grayscale);
        };
        let ramps = mask.ramps(self.scale_factor());
        crate::fast::edge_fade::with_ramps(self, &ramps, |window| {
            window.paint_image(visible, fitted, corner_radii, data, frame_index, grayscale)
        })
    }
}
