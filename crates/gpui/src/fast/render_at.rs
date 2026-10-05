//! A window drawn offscreen at a display scale it is not on: a Retina picture of a window on a
//! 1x display, for a visual test that has to see both.

use anyhow::Result;
use image::RgbaImage;

use crate::{App, DevicePixels, Window, size};

impl Window {
    /// Draws the window at `scale` device pixels to the point, offscreen at its own size in
    /// points, and gives the picture; the window's own scale is put back and drawn again after.
    ///
    /// Everything scale-dependent (glyphs, SVGs, masks, snapping) is drawn afresh at `scale`.
    pub fn render_to_image_at(&mut self, scale: f32, cx: &mut App) -> Result<RgbaImage> {
        let own = self.scale_factor();
        self.set_scale_factor(scale);
        self.draw(cx).clear(cx);
        let points = self.viewport_size;
        let device = size(
            DevicePixels((f32::from(points.width) * scale).ceil() as i32),
            DevicePixels((f32::from(points.height) * scale).ceil() as i32),
        );
        let image = self
            .platform_window
            .fast_render_scene_to_image(&self.rendered_frame.scene, device);
        self.set_scale_factor(own);
        self.draw(cx).clear(cx);
        image
    }
}
