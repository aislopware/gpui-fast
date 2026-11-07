//! What a Metal renderer tells GPUI about what it draws with besides a
//! scene, so that GPUI can leave a frame unpresented when it draws what the
//! screen already shows (gpui's `fast::same_frame`,
//! `PlatformWindow::presentation_epoch`).
//!
//! A frame's pixels follow from its scene and from three things the scene
//! doesn't hold: the atlas tiles its sprites name (a removed tile's place can
//! go to another image), the drawable's size, and whether the layer is opaque.
//! The epoch changes whenever any of them does: the atlas's own generation
//! plus a count of the size and opacity changes.

use gpui::{DevicePixels, Size};

use crate::metal_renderer::MetalRenderer;

/// The size and opacity changes a renderer has seen.
#[derive(Default)]
pub(crate) struct PresentationEpoch {
    changes: u64,
    size: Option<Size<DevicePixels>>,
    transparent: Option<bool>,
}

impl MetalRenderer {
    /// Changes whenever something besides the scene that the next frame draws
    /// with changed: the atlas, the drawable's size, the layer's opacity.
    pub fn fast_presentation_epoch(&self) -> u64 {
        self.fast_presentation
            .changes
            .wrapping_add(self.sprite_atlas.fast_generation())
    }
}

/// The drawable is now `size`.
pub(crate) fn drawable_size(renderer: &mut MetalRenderer, size: Size<DevicePixels>) {
    let epoch = &mut renderer.fast_presentation;
    if epoch.size.replace(size) != Some(size) {
        epoch.changes = epoch.changes.wrapping_add(1);
    }
}

/// The layer is now `transparent` or opaque.
pub(crate) fn transparency(renderer: &mut MetalRenderer, transparent: bool) {
    let epoch = &mut renderer.fast_presentation;
    if epoch.transparent.replace(transparent) != Some(transparent) {
        epoch.changes = epoch.changes.wrapping_add(1);
    }
}
