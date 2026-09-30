//! Cursors with the application's own picture: [`crate::CursorStyle::Image`].
//!
//! A remote-desktop view shows the far side's pointer. Drawn by the view, it
//! trails the person's hand by a frame on every move; handed to the platform
//! as the system cursor, it moves with the hand as a local pointer does, and
//! the view draws nothing when the pointer moves.
//!
//! The application names a picture with a [`CursorImageId`] of its choosing
//! and points the id at a picture with [`App::set_cursor_image`]. An element
//! asks for `CursorStyle::Image(id)` as for any other style, so a steady
//! cursor costs nothing per frame. Pointing an id that is showing at a new
//! picture changes the pointer at once, without waiting for a frame, which is
//! how a view follows the far side's I-beam and hand. Each platform keeps the
//! cursors it built by the picture's content, so flipping an id between
//! pictures it has shown before builds nothing.
//!
//! macOS builds an `NSCursor`. Other platforms keep the arrow for an image
//! style, as they do for an id nothing was pointed at.

use crate::{App, DevicePixels, Point, Size};
use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::hash::Hasher;
use std::sync::Arc;

/// Names a cursor picture for [`crate::CursorStyle::Image`]. The number is
/// the application's to choose; [`App::set_cursor_image`] points it at a
/// picture.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct CursorImageId(pub u64);

/// The largest picture a cursor may have, in pixels on a side. macOS draws a
/// cursor of up to 256 points at up to 4 pixels a point for the largest
/// pointer size.
pub const CURSOR_IMAGE_MAX_SIDE: i32 = 1024;

/// A cursor's picture: premultiplied BGRA pixels in rows packed from the top,
/// the hotspot in pixels from the top left, and how many pixels make a point.
///
/// The cursor is shown `size / scale` points large, as a local cursor is: it
/// does not follow the element's zoom.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorImage {
    bgra: Arc<[u8]>,
    size: Size<DevicePixels>,
    hotspot: Point<DevicePixels>,
    scale: f32,
    key: u64,
}

impl CursorImage {
    /// A picture of `size` pixels, `bgra` premultiplied and packed at four
    /// bytes a pixel, whose `hotspot` is the pixel that points. `scale` is
    /// pixels per point: 2 for a picture made for a Retina display.
    ///
    /// Fails when the pixels do not fill `size` exactly, when a side is empty
    /// or past [`CURSOR_IMAGE_MAX_SIDE`], when the hotspot lies outside the
    /// picture, or when `scale` is not a positive number.
    pub fn new(
        bgra: impl Into<Arc<[u8]>>,
        size: Size<DevicePixels>,
        hotspot: Point<DevicePixels>,
        scale: f32,
    ) -> Result<Self> {
        let bgra = bgra.into();
        let (width, height) = (size.width.0, size.height.0);
        ensure!(
            (1..=CURSOR_IMAGE_MAX_SIDE).contains(&width)
                && (1..=CURSOR_IMAGE_MAX_SIDE).contains(&height),
            "a cursor of {width}×{height} pixels"
        );
        ensure!(
            (0..width).contains(&hotspot.x.0) && (0..height).contains(&hotspot.y.0),
            "a hotspot at {:?} outside {width}×{height} pixels",
            hotspot
        );
        ensure!(scale.is_finite() && scale > 0., "a cursor at {scale} pixels a point");
        let pixels = width as usize * height as usize;
        ensure!(bgra.len() == pixels * 4, "{} bytes for {pixels} pixels", bgra.len());
        let mut hasher = collections::FxHasher::default();
        hasher.write_i32(width);
        hasher.write_i32(height);
        hasher.write_i32(hotspot.x.0);
        hasher.write_i32(hotspot.y.0);
        hasher.write_u32(scale.to_bits());
        hasher.write(&bgra);
        let key = hasher.finish();
        Ok(Self { bgra, size, hotspot, scale, key })
    }

    /// The pixels, premultiplied BGRA, rows packed from the top.
    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }

    /// The picture's size in pixels.
    pub fn size(&self) -> Size<DevicePixels> {
        self.size
    }

    /// The pixel that points, from the top left.
    pub fn hotspot(&self) -> Point<DevicePixels> {
        self.hotspot
    }

    /// Pixels per point.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The size the cursor is shown at, in points.
    pub fn size_in_points(&self) -> Size<f32> {
        Size {
            width: self.size.width.0 as f32 / self.scale,
            height: self.size.height.0 as f32 / self.scale,
        }
    }

    /// The hotspot in points from the top left; a hotspot on an odd pixel of a
    /// 2× picture lies between two points.
    pub fn hotspot_in_points(&self) -> Point<f32> {
        Point { x: self.hotspot.x.0 as f32 / self.scale, y: self.hotspot.y.0 as f32 / self.scale }
    }

    /// A number standing for the whole picture, hotspot and scale included:
    /// equal pictures have equal keys, so a platform reuses the cursor it
    /// built for one.
    pub fn key(&self) -> u64 {
        self.key
    }
}

impl App {
    /// Points `id` at `image`, the picture every element styled
    /// `CursorStyle::Image(id)` shows as the cursor, or forgets it with
    /// `None`, after which such an element shows the arrow.
    ///
    /// When the cursor showing is `id`'s, it changes at once, before the next
    /// frame.
    pub fn set_cursor_image(&mut self, id: CursorImageId, image: Option<CursorImage>) {
        self.platform.set_cursor_image(id, image);
    }
}

/// What the test platform was told about cursor pictures.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub(crate) struct TestCursorImages {
    /// Each id's picture.
    pub(crate) images: collections::HashMap<CursorImageId, CursorImage>,
    /// How many times an id was pointed at a picture or forgotten.
    pub(crate) sets: usize,
}

#[cfg(any(test, feature = "test-support"))]
impl TestCursorImages {
    pub(crate) fn set(
        this: &std::cell::RefCell<Self>,
        id: CursorImageId,
        image: Option<CursorImage>,
    ) {
        let mut this = this.borrow_mut();
        this.sets += 1;
        match image {
            Some(image) => this.images.insert(id, image),
            None => this.images.remove(&id),
        };
    }
}
