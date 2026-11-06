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
//! An application reads the same holdings across its windows with
//! [`snapshot`], at any time, for a memory log of its own: each renderer
//! publishes what it holds at most once a [`PUBLISH_INTERVAL`] after a frame,
//! and the device's total is read live. Onicode logs it in place of
//! monocode's `gpu_memory()`.
//!
//! Ported from wingleeio/zed 9b672fa and ac135eb (`ZERON_GPU_STATS`),
//! carried in zui 0b18592c5, Apache-2.0.

use std::{
    fmt,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLDevice as _, MTLResource, MTLTexture};
use parking_lot::Mutex;

use crate::metal_renderer::MetalRenderer;

/// How often a renderer writes its line.
const INTERVAL: Duration = Duration::from_secs(10);

/// How often a renderer publishes its holdings for [`snapshot`].
pub const PUBLISH_INTERVAL: Duration = Duration::from_secs(1);

/// When a renderer last wrote its line and published its holdings, and its
/// entry among the published ones, which goes with it.
#[derive(Debug, Default)]
pub(crate) struct GpuStats {
    last: Option<Instant>,
    published: Option<Instant>,
    /// Its key in [`PUBLISHED`], 0 until it first publishes.
    id: u64,
}

impl Drop for GpuStats {
    fn drop(&mut self) {
        if self.id != 0 {
            PUBLISHED.lock().retain(|(id, _)| *id != self.id);
        }
    }
}

/// What each live renderer last published, by its key.
static PUBLISHED: Mutex<Vec<(u64, Holdings)>> = Mutex::new(Vec::new());

/// The next renderer's key.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Whether `GPUI_GPU_STATS` asks for the line: set, and neither empty nor `0`.
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("GPUI_GPU_STATS").is_some_and(|value| !value.is_empty() && value != "0")
    })
}

/// Writes the renderer's line after a frame, at most once per [`INTERVAL`],
/// and publishes its holdings at most once per [`PUBLISH_INTERVAL`].
pub(crate) fn after_frame(renderer: &mut MetalRenderer) {
    let now = Instant::now();
    let line = enabled() && due(&mut renderer.fast_gpu_stats, now);
    let publish = due_every(
        &mut renderer.fast_gpu_stats.published,
        now,
        PUBLISH_INTERVAL,
    );
    if !(line || publish) {
        return;
    }
    let holdings = Holdings::of(renderer);
    if line {
        eprintln!("[gpu-stats] {holdings}");
    }
    if publish {
        publish_holdings(&mut renderer.fast_gpu_stats, holdings);
    }
}

/// Publishes `holdings` as the renderer `stats` belongs to holds them.
fn publish_holdings(stats: &mut GpuStats, holdings: Holdings) {
    if stats.id == 0 {
        stats.id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    }
    let mut published = PUBLISHED.lock();
    match published.iter_mut().find(|(id, _)| *id == stats.id) {
        Some((_, entry)) => *entry = holdings,
        None => published.push((stats.id, holdings)),
    }
}

/// What the process's renderers hold on the GPU, across its windows: the
/// device's total read now, and what each renderer last published (at most a
/// [`PUBLISH_INTERVAL`] old, as of its last frame) summed. The instance
/// buffer pool, which the windows share, is counted once.
pub fn snapshot() -> Holdings {
    let mut total = Holdings {
        device: objc2_metal::MTLCreateSystemDefaultDevice()
            .map_or(0, |device| device.currentAllocatedSize()),
        ..Holdings::default()
    };
    for (_, holdings) in PUBLISHED.lock().iter() {
        total.add(holdings);
    }
    total
}

/// Whether the line is due at `now`, noting it as written if so.
fn due(stats: &mut GpuStats, now: Instant) -> bool {
    due_every(&mut stats.last, now, INTERVAL)
}

/// Whether `interval` has passed at `now` since `last`, noting `now` as the
/// last if so.
fn due_every(last: &mut Option<Instant>, now: Instant, interval: Duration) -> bool {
    if last.is_some_and(|last| now.saturating_duration_since(last) < interval) {
        return false;
    }
    *last = Some(now);
    true
}

/// The bytes `texture` takes on the GPU: nothing for a memoryless texture.
pub(crate) fn allocated(texture: &ProtocolObject<dyn MTLTexture>) -> usize {
    MTLResource::allocatedSize(texture)
}

/// GPU memory, in bytes: one renderer's, or the process's from [`snapshot`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Holdings {
    /// Everything the device has allocated, for every window.
    pub device: usize,
    /// The renderers counted: one a window.
    pub renderers: usize,
    /// The sprite atlas textures.
    pub atlas_textures: usize,
    /// The bytes they take.
    pub atlas: usize,
    /// Buffers waiting in the pool, not those a frame in flight holds.
    pub pooled_buffers: usize,
    /// Each pooled buffer's size.
    pub buffer_size: usize,
    /// Backdrop blur scratch.
    pub backdrop: usize,
    /// The path intermediates.
    pub paths: usize,
    /// Scroll layer tiles.
    pub tiles: usize,
}

impl Holdings {
    /// Adds a renderer's `holdings` to these, the shared pool once.
    fn add(&mut self, holdings: &Holdings) {
        self.renderers += holdings.renderers;
        self.atlas_textures += holdings.atlas_textures;
        self.atlas += holdings.atlas;
        self.pooled_buffers = self.pooled_buffers.max(holdings.pooled_buffers);
        self.buffer_size = self.buffer_size.max(holdings.buffer_size);
        self.backdrop += holdings.backdrop;
        self.paths += holdings.paths;
        self.tiles += holdings.tiles;
    }

    pub(crate) fn of(renderer: &MetalRenderer) -> Self {
        let atlas = renderer.sprite_atlas.fast_textures();
        let (pooled_buffers, buffer_size) = renderer.instance_buffer_pool.lock().fast_stats();
        let paths = [
            &renderer.path_intermediate_texture,
            &renderer.path_intermediate_msaa_texture,
        ];
        Self {
            device: renderer.device.currentAllocatedSize(),
            renderers: 1,
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

    use super::{GpuStats, Holdings, INTERVAL, due, snapshot};
    use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

    const MIB: usize = 1024 * 1024;

    #[test]
    fn the_line_names_each_holding() {
        let holdings = Holdings {
            device: 300 * MIB,
            renderers: 1,
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

    /// Renderers' holdings add up, the pool the windows share counted once.
    #[test]
    fn holdings_add_up_with_the_shared_pool_once() {
        let window = |atlas: usize, pooled_buffers: usize| Holdings {
            device: 300 * MIB,
            renderers: 1,
            atlas_textures: 2,
            atlas,
            pooled_buffers,
            buffer_size: 2 * MIB,
            backdrop: MIB,
            paths: 4 * MIB,
            tiles: 0,
        };
        let mut total = Holdings::default();
        total.add(&window(8 * MIB, 3));
        total.add(&window(16 * MIB, 2));
        assert_eq!(
            total,
            Holdings {
                device: 0,
                renderers: 2,
                atlas_textures: 4,
                atlas: 24 * MIB,
                pooled_buffers: 3,
                buffer_size: 2 * MIB,
                backdrop: 2 * MIB,
                paths: 8 * MIB,
                tiles: 0,
            }
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

    /// A renderer's holdings are in the snapshot once it has drawn a frame,
    /// and leave it with the renderer.
    #[test]
    fn the_snapshot_sums_live_renderers() {
        if objc2_metal::MTLCreateSystemDefaultDevice().is_none() {
            eprintln!("skipped: no Metal device");
            return;
        }
        let pool = Arc::new(Mutex::new(InstanceBufferPool::default()));
        let draw = |renderer: &mut MetalRenderer| {
            let mut scene = Scene::default();
            scene.finish();
            renderer
                .render_scene_to_image(&scene, size(DevicePixels(64), DevicePixels(64)))
                .expect("scene rendered");
        };
        let mut first = MetalRenderer::new_headless(pool.clone()).unwrap();
        let mut second = MetalRenderer::new_headless(pool).unwrap();
        let published = |renderer: &MetalRenderer| {
            let id = renderer.fast_gpu_stats.id;
            super::PUBLISHED
                .lock()
                .iter()
                .find(|(published, _)| id != 0 && *published == id)
                .map(|(_, holdings)| *holdings)
        };
        assert_eq!(published(&first), None, "nothing before a frame");
        draw(&mut first);
        draw(&mut second);
        // Other tests' renderers may be live in this process too, and only
        // add to the sums.
        let both = snapshot();
        let (a, b) = (published(&first).unwrap(), published(&second).unwrap());
        assert_eq!((a.renderers, b.renderers), (1, 1));
        assert!(a.paths >= 64 * 64 * 4 && b.paths >= 64 * 64 * 4);
        assert!(both.device > 0);
        assert!(both.renderers >= 2, "{both:?}");
        assert!(both.paths >= a.paths + b.paths, "{both:?}");
        let id = first.fast_gpu_stats.id;
        drop(first);
        assert!(
            !super::PUBLISHED
                .lock()
                .iter()
                .any(|(published, _)| *published == id),
            "gone with its renderer"
        );
        assert!(published(&second).is_some());
    }
}
