//! A line of the GPU memory a window's renderer holds (`GPUI_GPU_STATS=1`).
//!
//! Every ten seconds while it draws, each renderer writes to standard error
//! what the Metal device has allocated in all (`MTLDevice.currentAllocatedSize`:
//! every window's resources and the process's together) and what this
//! renderer keeps of it: its sprite atlas textures, the instance buffer pool
//! it shares with the other windows, backdrop blur scratch, the path
//! intermediates and scroll layer tiles. Each texture is measured by its
//! `allocatedSize`, so a memoryless MSAA texture counts as nothing. The line
//! goes to standard error rather than to `log` because it only exists when
//! asked for, and an application may run `tracing` without a log bridge.
//!
//! It is the measurement for memory work (the instance pool's shrink,
//! `fast::instance_pool`; the drawable count) without Instruments.
//!
//! Ported from wingleeio/zed 9b672fa and ac135eb (`ZERON_GPU_STATS`),
//! carried in zui 0b18592c5, Apache-2.0.

use std::{
    fmt,
    sync::OnceLock,
    time::{Duration, Instant},
};

use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLDevice as _, MTLResource, MTLTexture};

use crate::metal_renderer::MetalRenderer;

/// How often a renderer writes its line.
const INTERVAL: Duration = Duration::from_secs(10);

/// When a renderer last wrote its line.
#[derive(Debug, Default)]
pub(crate) struct GpuStats {
    last: Option<Instant>,
}

/// Whether `GPUI_GPU_STATS` asks for the line: set, and neither empty nor `0`.
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("GPUI_GPU_STATS").is_some_and(|value| !value.is_empty() && value != "0")
    })
}

/// Writes the renderer's line after a frame, at most once per [`INTERVAL`].
pub(crate) fn after_frame(renderer: &mut MetalRenderer) {
    if enabled() && due(&mut renderer.fast_gpu_stats, Instant::now()) {
        eprintln!("[gpu-stats] {}", Holdings::of(renderer));
    }
}

/// Whether the line is due at `now`, noting it as written if so.
fn due(stats: &mut GpuStats, now: Instant) -> bool {
    if stats
        .last
        .is_some_and(|last| now.saturating_duration_since(last) < INTERVAL)
    {
        return false;
    }
    stats.last = Some(now);
    true
}

/// The bytes `texture` takes on the GPU: nothing for a memoryless texture.
pub(crate) fn allocated(texture: &ProtocolObject<dyn MTLTexture>) -> usize {
    MTLResource::allocatedSize(texture)
}

/// GPU memory, in bytes.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Holdings {
    /// Everything the device has allocated, for every window.
    pub(crate) device: usize,
    pub(crate) atlas_textures: usize,
    pub(crate) atlas: usize,
    /// Buffers waiting in the pool, not those a frame in flight holds.
    pub(crate) pooled_buffers: usize,
    pub(crate) buffer_size: usize,
    pub(crate) backdrop: usize,
    pub(crate) paths: usize,
    pub(crate) tiles: usize,
}

impl Holdings {
    pub(crate) fn of(renderer: &MetalRenderer) -> Self {
        let atlas = renderer.sprite_atlas.fast_textures();
        let (pooled_buffers, buffer_size) = renderer.instance_buffer_pool.lock().fast_stats();
        let paths = [
            &renderer.path_intermediate_texture,
            &renderer.path_intermediate_msaa_texture,
        ];
        Self {
            device: renderer.device.currentAllocatedSize(),
            atlas_textures: atlas.len(),
            atlas: atlas.iter().map(|texture| allocated(texture)).sum(),
            pooled_buffers,
            buffer_size,
            backdrop: renderer.fast_backdrop.resident_bytes(),
            paths: paths.into_iter().flatten().map(|t| allocated(t)).sum(),
            tiles: renderer.fast_layers.resident_bytes(),
        }
    }
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024. * 1024.)
}

impl fmt::Display for Holdings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "device: {:.1} MiB; atlas: {} textures, {:.1} MiB; instance pool: {} × {:.1} MiB; \
             backdrop scratch: {:.1} MiB; path intermediates: {:.1} MiB; layer tiles: {:.1} MiB",
            mib(self.device),
            self.atlas_textures,
            mib(self.atlas),
            self.pooled_buffers,
            mib(self.buffer_size),
            mib(self.backdrop),
            mib(self.paths),
            mib(self.tiles),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    use gpui::{DevicePixels, Scene, size};
    use parking_lot::Mutex;

    use super::{GpuStats, Holdings, INTERVAL, due};
    use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

    const MIB: usize = 1024 * 1024;

    #[test]
    fn the_line_names_each_holding() {
        let holdings = Holdings {
            device: 300 * MIB,
            atlas_textures: 3,
            atlas: 48 * MIB,
            pooled_buffers: 2,
            buffer_size: 2 * MIB,
            backdrop: 3 * MIB / 2,
            paths: 0,
            tiles: 64 * MIB,
        };
        assert_eq!(
            holdings.to_string(),
            "device: 300.0 MiB; atlas: 3 textures, 48.0 MiB; instance pool: 2 × 2.0 MiB; \
             backdrop scratch: 1.5 MiB; path intermediates: 0.0 MiB; layer tiles: 64.0 MiB"
        );
    }

    #[test]
    fn the_line_is_due_once_per_interval() {
        let start = Instant::now();
        let mut stats = GpuStats::default();
        assert!(due(&mut stats, start), "the first frame writes the line");
        assert!(!due(&mut stats, start + INTERVAL / 2));
        assert!(!due(
            &mut stats,
            start + INTERVAL - Duration::from_millis(1)
        ));
        assert!(due(&mut stats, start + INTERVAL));
        assert!(!due(&mut stats, start + INTERVAL + Duration::from_secs(1)));
    }

    /// A rendered frame's path intermediate counts, and the device total
    /// includes it.
    #[test]
    fn holdings_measure_a_renderers_textures() {
        if objc2_metal::MTLCreateSystemDefaultDevice().is_none() {
            eprintln!("skipped: no Metal device");
            return;
        }
        let mut renderer =
            MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())))
                .unwrap();
        let mut scene = Scene::default();
        scene.finish();
        renderer
            .render_scene_to_image(&scene, size(DevicePixels(256), DevicePixels(128)))
            .expect("scene rendered");
        let holdings = Holdings::of(&renderer);
        assert!(
            holdings.paths >= 256 * 128 * 4,
            "path intermediates take {} bytes",
            holdings.paths
        );
        assert!(holdings.device >= holdings.paths + holdings.atlas);
        assert_eq!(
            holdings.buffer_size,
            crate::fast::instance_pool::DEFAULT_BUFFER_SIZE
        );
    }
}
