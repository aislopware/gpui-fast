//! Shrinking the Metal instance buffer pool after a large frame.
//!
//! The pool grows its buffer size to the next power of two whenever a frame
//! needs more instance data than fits, and never shrank it again. One
//! unusually complex frame (a large table, a long scroll through dense text)
//! therefore pinned every pooled buffer, about one per drawable in flight, at
//! that size for the life of the process.
//!
//! The pool now counts consecutive window frames that wrote at most half of
//! the buffer size, and after [`SHRINK_AFTER_FRAMES`] of them shrinks to the
//! next power of two of the largest of those frames, never below
//! [`DEFAULT_BUFFER_SIZE`]. A frame writing more than half restarts the count,
//! so a workload alternating between large and small frames keeps its
//! buffers. Pooled buffers of the old size go at once; those in flight go when
//! they are released, as when the pool grows. Scroll layer tile rasterization
//! draws from the same pool but doesn't count as a frame.
//!
//! Ported from zui 30d78f65b (wingleeio), Apache-2.0. It supersedes the
//! halving of wingleeio/zed 9b672fa, carried in zui 0b18592c5: shrinking
//! straight to the run's peak releases a large buffer in one step.

/// The size of each pooled instance buffer until a frame needs more.
pub(crate) const DEFAULT_BUFFER_SIZE: usize = 2 * 1024 * 1024;

/// Consecutive frames that must fit in half the pooled buffer size before the
/// pool shrinks its buffers.
pub(crate) const SHRINK_AFTER_FRAMES: u32 = 120;

/// The run of frames that fit in half the pooled buffer size.
#[derive(Debug, Default)]
pub(crate) struct Shrink {
    frames_below_half: u32,
    peak_below_half: usize,
}

impl Shrink {
    /// Notes a frame that wrote `written` bytes into buffers of
    /// `buffer_size`, returning the size to shrink them to once a run of
    /// frames fit in half of it.
    pub(crate) fn note_frame(&mut self, buffer_size: usize, written: usize) -> Option<usize> {
        if buffer_size <= DEFAULT_BUFFER_SIZE || written > buffer_size / 2 {
            *self = Self::default();
            return None;
        }
        self.peak_below_half = self.peak_below_half.max(written);
        self.frames_below_half += 1;
        if self.frames_below_half < SHRINK_AFTER_FRAMES {
            return None;
        }
        let size = self
            .peak_below_half
            .next_power_of_two()
            .max(DEFAULT_BUFFER_SIZE);
        *self = Self::default();
        Some(size)
    }
}

#[cfg(test)]
mod tests {
    use std::{mem, sync::Arc};

    use gpui::{Bounds, ContentMask, DevicePixels, Hsla, Quad, ScaledPixels, Scene, point, size};
    use parking_lot::Mutex;

    use super::{DEFAULT_BUFFER_SIZE, SHRINK_AFTER_FRAMES};
    use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

    const MIB: usize = 1024 * 1024;

    fn buffer_size(pool: &InstanceBufferPool) -> usize {
        pool.fast_stats().1
    }

    /// Grows the pool as a frame writing `written` bytes would.
    fn grow(pool: &mut InstanceBufferPool, written: usize) {
        if written > buffer_size(pool) {
            pool.reset(written.next_power_of_two());
        }
        pool.fast_note_frame(written);
    }

    #[test]
    fn the_pool_shrinks_after_a_run_of_smaller_frames() {
        let mut pool = InstanceBufferPool::default();
        grow(&mut pool, MIB);
        assert_eq!(buffer_size(&pool), DEFAULT_BUFFER_SIZE);

        grow(&mut pool, 20 * MIB);
        assert_eq!(buffer_size(&pool), 32 * MIB, "grows to fit a large frame");

        // A frame writing more than half the buffer size restarts the run.
        for _ in 1..SHRINK_AFTER_FRAMES {
            grow(&mut pool, MIB);
        }
        grow(&mut pool, 17 * MIB);
        for _ in 1..SHRINK_AFTER_FRAMES {
            grow(&mut pool, MIB);
        }
        assert_eq!(buffer_size(&pool), 32 * MIB);

        // Shrinks to fit the largest frame of the run.
        grow(&mut pool, 5 * MIB);
        assert_eq!(buffer_size(&pool), 8 * MIB);
        for _ in 0..SHRINK_AFTER_FRAMES {
            grow(&mut pool, MIB);
        }
        assert_eq!(buffer_size(&pool), DEFAULT_BUFFER_SIZE);

        // Never below the default size.
        for _ in 0..SHRINK_AFTER_FRAMES {
            grow(&mut pool, 0);
        }
        assert_eq!(buffer_size(&pool), DEFAULT_BUFFER_SIZE);
    }

    /// Over seeded runs of frame sizes: the buffer size is a power of two no
    /// smaller than the default, it holds the largest frame of the last
    /// [`SHRINK_AFTER_FRAMES`], and once frames stay small it comes back to
    /// the default.
    #[test]
    fn the_buffer_size_tracks_recent_frames() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..32 {
            let mut pool = InstanceBufferPool::default();
            let mut recent = std::collections::VecDeque::new();
            for _ in 0..1_000 {
                let written = match next() % 10 {
                    0 => (next() % (64 * MIB as u64)) as usize,
                    _ => (next() % MIB as u64) as usize,
                };
                grow(&mut pool, written);
                recent.push_back(written);
                if recent.len() > SHRINK_AFTER_FRAMES as usize {
                    recent.pop_front();
                }
                let size = buffer_size(&pool);
                let largest = recent.iter().copied().max().unwrap_or(0);
                assert!(size.is_power_of_two() && size >= DEFAULT_BUFFER_SIZE);
                assert!(size >= largest, "{size} holds a recent frame of {largest}");
            }
            for _ in 0..2 * SHRINK_AFTER_FRAMES {
                grow(&mut pool, MIB);
            }
            assert_eq!(buffer_size(&pool), DEFAULT_BUFFER_SIZE);
        }
    }

    /// Through the renderer: a frame of many quads grows the pool, and a run
    /// of empty frames shrinks it back.
    #[test]
    fn a_renderer_shrinks_its_pool_after_a_large_frame() {
        if objc2_metal::MTLCreateSystemDefaultDevice().is_none() {
            eprintln!("skipped: no Metal device");
            return;
        }
        let pool = Arc::new(Mutex::new(InstanceBufferPool::default()));
        let mut renderer = MetalRenderer::new_headless(pool.clone()).unwrap();
        let viewport = size(DevicePixels(64), DevicePixels(64));
        let mut render = |scene: &mut Scene| {
            scene.finish();
            renderer
                .render_scene_to_image(scene, viewport)
                .expect("scene rendered");
        };

        let mut large = Scene::default();
        let everywhere = Bounds {
            origin: point(ScaledPixels(0.), ScaledPixels(0.)),
            size: size(ScaledPixels(64.), ScaledPixels(64.)),
        };
        // Three quarters of the doubled buffer: more than half of it.
        for index in 0..3 * DEFAULT_BUFFER_SIZE / 2 / mem::size_of::<Quad>() {
            let x = (index % 64) as f32;
            large.insert_primitive(Quad {
                bounds: Bounds {
                    origin: point(ScaledPixels(x), ScaledPixels(0.)),
                    size: size(ScaledPixels(1.), ScaledPixels(1.)),
                },
                content_mask: ContentMask { bounds: everywhere },
                background: Hsla::white().into(),
                ..Default::default()
            });
        }
        render(&mut large);
        let grown = buffer_size(&pool.lock());
        assert_eq!(grown, 2 * DEFAULT_BUFFER_SIZE);

        for _ in 1..SHRINK_AFTER_FRAMES {
            render(&mut Scene::default());
        }
        assert_eq!(buffer_size(&pool.lock()), grown);
        render(&mut Scene::default());
        assert_eq!(buffer_size(&pool.lock()), DEFAULT_BUFFER_SIZE);
    }
}
