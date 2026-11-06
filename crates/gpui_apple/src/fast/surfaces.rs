//! The video surfaces a frame samples, released on the render thread once the GPU is done.
//!
//! CoreVideo may hand a `CVMetalTexture`'s backing buffer to a new picture once the texture is
//! released, so each frame's textures are held until its command buffer completes. They are
//! released here, on the thread that renders, never in Metal's completion handler. Releasing a
//! texture tells the texture cache it came from that its backing is free
//! (`CVMetalTextureCache::bufferBackingNotInUse`). The cache is not safe to touch from Metal's
//! thread while the render thread flushes it, and it is gone once the renderer is dropped with a
//! frame still on the GPU: either crashed in that call.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2_core_foundation::CFRetained;
use objc2_core_video::CVMetalTexture;

/// Each frame's surface textures, until the GPU finished with that frame.
#[derive(Default)]
pub(crate) struct SurfacesInFlight {
    frames: VecDeque<(Arc<FrameDone>, Vec<CFRetained<CVMetalTexture>>)>,
}

/// Set by a frame's completion handler: its textures may go.
#[derive(Default)]
pub(crate) struct FrameDone(AtomicBool);

impl FrameDone {
    /// The GPU is done with the frame. Safe on any thread: it touches no CoreVideo object.
    pub(crate) fn set(&self) {
        self.0.store(true, Ordering::Release);
    }
}

impl SurfacesInFlight {
    /// Hold `textures` for the frame being committed, until the flag handed back is set; no
    /// flag for a frame that samples none.
    pub(crate) fn hold(
        &mut self,
        textures: Vec<CFRetained<CVMetalTexture>>,
    ) -> Option<Arc<FrameDone>> {
        if textures.is_empty() {
            return None;
        }
        let done = Arc::new(FrameDone::default());
        self.frames.push_back((done.clone(), textures));
        Some(done)
    }

    /// Release the textures of every frame the GPU has finished with.
    pub(crate) fn release_finished(&mut self) {
        self.frames
            .retain(|(done, _)| !done.0.load(Ordering::Acquire));
    }

    /// How many frames still hold textures.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.frames.len()
    }
}

/// Pictures for the surface tests: IOSurface-backed, as a decoder or an offscreen renderer hands
/// them over.
#[cfg(test)]
pub(crate) mod pictures {
    use objc2_core_foundation::{CFDictionary, CFRetained, CFType};
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress,
        CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRow,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferIOSurfacePropertiesKey,
        kCVPixelFormatType_32BGRA, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        kCVReturnSuccess,
    };

    /// A blank IOSurface-backed buffer `side` pixels square in `format`.
    fn blank(format: u32, side: usize) -> CFRetained<CVPixelBuffer> {
        // SAFETY: CoreVideo's own constant string, which lives forever.
        let key = unsafe { kCVPixelBufferIOSurfacePropertiesKey };
        let options =
            CFDictionary::from_slices(&[key], &[&*CFDictionary::<CFType, CFType>::empty()]);
        let mut buffer = std::ptr::null_mut();
        // SAFETY: a valid out-pointer, which CoreVideo fills with a +1 buffer on success.
        let status = unsafe {
            CVPixelBufferCreate(
                None,
                side,
                side,
                format,
                Some(options.as_opaque()),
                std::ptr::NonNull::from(&mut buffer),
            )
        };
        assert_eq!(status, kCVReturnSuccess);
        // SAFETY: the buffer CoreVideo created, owned from here.
        unsafe { CFRetained::from_raw(std::ptr::NonNull::new(buffer).unwrap()) }
    }

    /// Writes `row(y)` into each of `rows` rows `stride` bytes apart from `base`, locked.
    fn write_rows(
        buffer: &CVPixelBuffer,
        base: impl Fn(&CVPixelBuffer) -> (*mut u8, usize),
        rows: usize,
        row: impl Fn(usize, &mut [u8]),
    ) {
        // SAFETY: a live buffer, locked for writing and unlocked with the same flags; a locked
        // plane spans `bytes per row × rows` from its base address.
        unsafe {
            assert_eq!(
                CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
            let (base, stride) = base(buffer);
            for y in 0..rows {
                row(
                    y,
                    std::slice::from_raw_parts_mut(base.add(y * stride), stride),
                );
            }
            assert_eq!(
                CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
        }
    }

    /// A straight-alpha BGRA picture `side` pixels square, pixel `(x, y)` being `bgra(x, y)`,
    /// the top row first.
    pub(crate) fn bgra(
        side: usize,
        bgra: impl Fn(usize, usize) -> [u8; 4],
    ) -> CFRetained<CVPixelBuffer> {
        let buffer = blank(kCVPixelFormatType_32BGRA, side);
        let base = |b: &CVPixelBuffer| {
            (
                CVPixelBufferGetBaseAddress(b).cast::<u8>(),
                CVPixelBufferGetBytesPerRow(b),
            )
        };
        write_rows(&buffer, base, side, |y, row| {
            for (x, pixel) in row.chunks_exact_mut(4).take(side).enumerate() {
                pixel.copy_from_slice(&bgra(x, y));
            }
        });
        buffer
    }

    /// A white full-range Y′CbCr 4:2:0 picture `side` pixels square.
    pub(crate) fn ycbcr_white(side: usize) -> CFRetained<CVPixelBuffer> {
        let buffer = blank(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, side);
        let plane = |plane| {
            move |b: &CVPixelBuffer| {
                (
                    CVPixelBufferGetBaseAddressOfPlane(b, plane).cast::<u8>(),
                    CVPixelBufferGetBytesPerRowOfPlane(b, plane),
                )
            }
        };
        write_rows(&buffer, plane(0), side, |_, row| row.fill(255));
        write_rows(&buffer, plane(1), side / 2, |_, row| row.fill(128));
        buffer
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{
        Bounds, ContentMask, DevicePixels, Hsla, PaintSurface, Quad, ScaledPixels, Scene, point,
        rgba, size,
    };
    use parking_lot::Mutex;

    use super::pictures;
    use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

    const SIDE: usize = 32;

    /// `picture` drawn over opaque black at `opacity`, as RGBA.
    fn drawn(
        renderer: &mut MetalRenderer,
        picture: &objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
        opacity: f32,
    ) -> image::RgbaImage {
        let bounds = Bounds {
            origin: point(ScaledPixels(0.), ScaledPixels(0.)),
            size: size(ScaledPixels(SIDE as f32), ScaledPixels(SIDE as f32)),
        };
        let mut scene = Scene::default();
        scene.insert_primitive(Quad {
            bounds,
            content_mask: ContentMask { bounds },
            background: Hsla::from(rgba(0x000000ff)).into(),
            ..Default::default()
        });
        scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            opacity,
            pad: 0,
            image_buffer: picture.clone(),
        });
        scene.finish();
        let side = DevicePixels(SIDE as i32);
        renderer
            .render_scene_to_image(&scene, size(side, side))
            .unwrap()
    }

    /// A BGRA surface draws its pixels where they lie, the top row at the top, blended by its
    /// straight alpha and its opacity as an image is: an opaque red, a half-covering green, an
    /// opaque blue and white in its four quarters.
    #[test]
    fn a_bgra_surface_draws_its_straight_alpha_at_its_opacity() {
        if objc2_metal::MTLCreateSystemDefaultDevice().is_none() {
            eprintln!("skipped: no Metal device");
            return;
        }
        let half = SIDE / 2;
        let picture = pictures::bgra(SIDE, |x, y| match (x < half, y < half) {
            (true, true) => [0, 0, 255, 255],
            (false, true) => [0, 255, 0, 128],
            (true, false) => [255, 0, 0, 255],
            (false, false) => [255, 255, 255, 255],
        });
        let mut renderer =
            MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())))
                .unwrap();
        for (opacity, red, green, blue, white) in [
            (1.0, [255, 0, 0], [0, 128, 0], [0, 0, 255], [255; 3]),
            (0.5, [128, 0, 0], [0, 64, 0], [0, 0, 128], [128; 3]),
        ] {
            let image = drawn(&mut renderer, &picture, opacity);
            for ((x, y), want) in [
                ((8, 8), red),
                ((24, 8), green),
                ((8, 24), blue),
                ((24, 24), white),
            ] {
                let got = &image.get_pixel(x, y).0[..3];
                assert!(
                    got.iter()
                        .zip(want)
                        .all(|(g, w)| (i32::from(*g) - w).abs() <= 2),
                    "at ({x}, {y}), opacity {opacity}: {got:?}, not {want:?}"
                );
            }
        }
        assert!(!renderer.surface_skip_logged(), "no surface skipped");
    }
}
