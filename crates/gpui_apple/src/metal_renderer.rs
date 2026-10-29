use crate::{metal_atlas::MetalAtlas, presentation_pacing::PresentationPacer};
use anyhow::{Context as _, Result};
use block2::RcBlock;
use gpui::{
    AtlasTextureId, Background, Bounds, ContentMask, DevicePixels, PaintSurface, Path, Point,
    PresentedFrame, PresentedFrameSink, PrimitiveBatch, ScaledPixels, Scene, Size,
    composition::ComposedBatch, point, size,
};
#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
use image::RgbaImage;
use objc2::{Message, rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::{CFRetained, CFString, CGSize};
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceDisplayP3};
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane, kCVImageBufferYCbCrMatrix_ITU_R_601_4,
    kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVImageBufferYCbCrMatrixKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    kCVPixelFormatType_420YpCbCr10BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange,
    kCVPixelFormatType_422YpCbCr10BiPlanarFullRange,
    kCVPixelFormatType_422YpCbCr10BiPlanarVideoRange,
    kCVPixelFormatType_444YpCbCr10BiPlanarFullRange,
    kCVPixelFormatType_444YpCbCr10BiPlanarVideoRange, kCVReturnSuccess,
};
use objc2_foundation::{NSRange, NSString};
#[cfg(target_os = "macos")]
use objc2_metal::MTLCopyAllDevices;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLDrawable,
    MTLGPUFamily, MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLResourceOptions, MTLStorageMode, MTLStoreAction, MTLTexture,
    MTLTextureDescriptor, MTLTextureType, MTLTextureUsage, MTLViewport,
};
#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
use objc2_metal::{MTLBlitCommandEncoder, MTLOrigin, MTLRegion, MTLSize};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use parking_lot::Mutex;

use std::{
    cell::Cell,
    mem,
    mem::MaybeUninit,
    ops::Range,
    ptr, slice,
    sync::Arc,
    time::{Duration, Instant},
};

// Exported to metal
pub(crate) type PointF = gpui::Point<f32>;

#[cfg(not(feature = "runtime_shaders"))]
const SHADERS_METALLIB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shaders.metallib"));
#[cfg(feature = "runtime_shaders")]
const SHADERS_SOURCE_FILE: &str = include_str!(concat!(env!("OUT_DIR"), "/stitched_shaders.metal"));
// Use 4x MSAA, all devices support it.
// https://developer.apple.com/documentation/metal/mtldevice/1433355-supportstexturesamplecount
const PATH_SAMPLE_COUNT: u32 = 4;
/// Metal requires the offset a buffer is bound at to be 256-byte aligned.
const INSTANCE_BUFFER_ALIGNMENT: usize = 256;
const MAX_INSTANCE_BUFFER_SIZE: usize = 256 * 1024 * 1024;

pub type Context = Arc<Mutex<InstanceBufferPool>>;
pub type Renderer = MetalRenderer;

pub fn new_renderer(
    context: self::Context,
    transparent: bool,
    use_display_p3: bool,
) -> Result<Renderer> {
    MetalRenderer::new(context, transparent, use_display_p3)
}

pub struct InstanceBufferPool {
    buffer_size: usize,
    buffers: Vec<InstanceBuffer>,
}

impl Default for InstanceBufferPool {
    fn default() -> Self {
        Self {
            buffer_size: 2 * 1024 * 1024,
            buffers: Vec::new(),
        }
    }
}

pub(crate) struct InstanceBuffer {
    metal_buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    size: usize,
}

// SAFETY: Each buffer has one CPU writer and returns to the mutex-protected pool only after GPU completion.
unsafe impl Send for InstanceBuffer {}

impl InstanceBufferPool {
    pub(crate) fn reset(&mut self, buffer_size: usize) {
        self.buffer_size = buffer_size;
        self.buffers.clear();
    }

    pub(crate) fn acquire(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        unified_memory: bool,
    ) -> Result<InstanceBuffer> {
        if let Some(buffer) = self.buffers.pop() {
            return Ok(buffer);
        }
        let options = if unified_memory {
            MTLResourceOptions::StorageModeShared
                // Buffers are write only which can benefit from the combined cache
                // https://developer.apple.com/documentation/metal/mtlresourceoptions/cpucachemodewritecombined
                | MTLResourceOptions::CPUCacheModeWriteCombined
        } else {
            MTLResourceOptions::StorageModeManaged
        };
        let buffer = device
            .newBufferWithLength_options(self.buffer_size, options)
            .with_context(|| format!("allocating {}-byte instance buffer", self.buffer_size))?;
        Ok(InstanceBuffer {
            metal_buffer: buffer,
            size: self.buffer_size,
        })
    }

    pub(crate) fn release(&mut self, buffer: InstanceBuffer) {
        if buffer.size == self.buffer_size {
            self.buffers.push(buffer)
        }
    }
}

/// The pixel format of the drawables the renderer draws into.
pub(crate) fn drawable_pixel_format() -> MTLPixelFormat {
    #[cfg(test)]
    if let Some(format) = composition_tests::DRAWABLE_PIXEL_FORMAT.get() {
        return format;
    }
    MTLPixelFormat::BGRA8Unorm
}

/// Until the platform names the display's refresh, assume the fastest Apple panel's.
const DEFAULT_REFRESH: Duration = Duration::from_micros(8_333);

pub struct MetalRenderer {
    pub(crate) device: Retained<ProtocolObject<dyn MTLDevice>>,
    layer: Option<Retained<CAMetalLayer>>,
    is_apple_gpu: bool,
    pub(crate) is_unified_memory: bool,
    presents_with_transaction: bool,
    /// Receives each drawn frame once the display has shown it; see [`Self::draw`].
    presented_frame_sink: Option<PresentedFrameSink>,
    /// Frames the display has shown or dropped since [`Self::ready_for_vsync_frame`] last
    /// looked, filled from a Metal thread.
    presentations: Arc<Mutex<Vec<PresentedFrame>>>,
    pacer: PresentationPacer,
    /// Whether this layer's drawables report their presentation, learned from the first one.
    /// The iOS simulator's `CAMetalDrawable` implements neither `addPresentedHandler:` nor
    /// `presentedTime`; there no frame is reported, rather than a made-up instant, and the
    /// pacer never sees a queue.
    drawables_report_presentation: Option<bool>,
    /// For headless rendering, tracks whether output should be opaque
    pub(crate) opaque: bool,
    pub(crate) command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pub(crate) paths_rasterization_pipeline_state:
        Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    path_sprites_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    shadows_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    quads_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// Draws the holes natives cut: quads that clear what is under them.
    holes_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    pub(crate) fast_occlusion: crate::fast::occlusion::Occlusion,
    pub(crate) fast_backdrop: crate::fast::backdrop::Backdrop,
    underlines_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    monochrome_sprites_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    pub(crate) polychrome_sprites_pipeline_state:
        Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    surfaces_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    pub(crate) unit_vertices: Retained<ProtocolObject<dyn MTLBuffer>>,
    pub(crate) instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    sprite_atlas: Arc<MetalAtlas>,
    /// The textures of each frame still on the GPU, released on this thread once it is done
    /// (`fast::surfaces`). Before the cache, so they drop before it.
    surfaces_in_flight: crate::fast::surfaces::SurfacesInFlight,
    core_video_texture_cache: CFRetained<CVMetalTextureCache>,
    /// A surface was skipped and said so; later ones stay quiet.
    surface_skip_logged: bool,
    pub(crate) path_intermediate_texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    pub(crate) path_intermediate_msaa_texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    path_sample_count: u32,
    /// Offscreen render target reused across `render_scene` calls when
    /// rendering headlessly without reading pixels back.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    headless_render_target: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    pub(crate) fast_layers: crate::fast::layers::TileCache,
}

/// Records the frame in `drawable` for the pacer, and hands it to `sink`, once the display
/// shows it or learns it was dropped. Must run before the drawable is presented, as
/// `addPresentedHandler:` requires.
fn observe_presentation(
    drawable: &ProtocolObject<dyn MTLDrawable>,
    presentations: Arc<Mutex<Vec<PresentedFrame>>>,
    sink: Option<PresentedFrameSink>,
) {
    let submitted_at = Instant::now();
    let handler = RcBlock::new(
        move |drawable: ptr::NonNull<ProtocolObject<dyn MTLDrawable>>| {
            // SAFETY: `addPresentedHandler:` calls the block with the drawable it was added to,
            // alive for the duration of the call.
            let drawable = unsafe { drawable.as_ref() };
            let frame = PresentedFrame {
                submitted_at,
                presented_at: host_time_to_instant(drawable.presentedTime())
                    .or_else(|| presented_at_callback().then(Instant::now)),
            };
            presentations.lock().push(frame);
            if let Some(sink) = &sink {
                sink(frame);
            }
        },
    );
    // SAFETY: `addPresentedHandler:` copies the block before this one is released, and the
    // block's captures are `Send` values its Metal thread may use.
    unsafe {
        drawable.addPresentedHandler(RcBlock::as_ptr(&handler));
    }
}

/// Whether a drawable shown without a presentation time counts as shown when its presented
/// handler runs (`GPUI_PRESENTED_AT_CALLBACK=1`). A display that never reports scan-out, such
/// as a virtual display a remote-desktop host drives, gives every drawable a `presentedTime` of
/// zero; measurements there can only take the callback as the glass. Not for production: a
/// dropped drawable's handler runs too.
fn presented_at_callback() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var("GPUI_PRESENTED_AT_CALLBACK").is_ok_and(|value| value == "1"))
}

/// Converts a Core Animation host time (`CACurrentMediaTime` seconds, as `presentedTime`
/// reports) to an `Instant`. `presentedTime` is zero for a drawable that was never shown.
fn host_time_to_instant(host_time: f64) -> Option<Instant> {
    if host_time <= 0.0 {
        return None;
    }
    let now = Instant::now();
    let age = objc2_quartz_core::CACurrentMediaTime() - host_time;
    if age >= 0.0 {
        now.checked_sub(Duration::from_secs_f64(age))
    } else {
        now.checked_add(Duration::from_secs_f64(-age))
    }
}

#[repr(C)]
pub struct PathRasterizationVertex {
    pub xy_position: Point<ScaledPixels>,
    pub st_position: Point<f32>,
    pub color: Background,
    pub bounds: Bounds<ScaledPixels>,
}

impl MetalRenderer {
    /// Creates a new MetalRenderer with a CAMetalLayer for window-based rendering.
    pub fn new(
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
        transparent: bool,
        use_display_p3: bool,
    ) -> Result<Self> {
        let device = Self::create_device()?;
        let layer = CAMetalLayer::new();
        Self::configure_layer(&layer, &device, transparent, use_display_p3);
        Self::new_internal(device, Some(layer), !transparent, instance_buffer_pool)
    }

    /// Creates a renderer for a CAMetalLayer owned by a platform view, such as
    /// the backing layer UIKit creates for a view whose `layerClass` is
    /// `CAMetalLayer`. The renderer retains the layer.
    pub fn from_layer(
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
        layer: &CAMetalLayer,
        transparent: bool,
    ) -> Result<Self> {
        let device = Self::create_device()?;
        let layer = layer.retain();
        Self::configure_layer(&layer, &device, transparent, false);
        Self::new_internal(device, Some(layer), !transparent, instance_buffer_pool)
    }

    fn configure_layer(
        layer: &CAMetalLayer,
        device: &ProtocolObject<dyn MTLDevice>,
        transparent: bool,
        use_display_p3: bool,
    ) {
        layer.setDevice(Some(device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        if use_display_p3 {
            // SAFETY: The constant is extern, from Core Graphics
            let display_p3_name = unsafe { kCGColorSpaceDisplayP3 };
            if let Some(display_p3_color_space) = CGColorSpace::with_name(Some(display_p3_name)) {
                layer.setColorspace(Some(&display_p3_color_space));
            } else {
                log::error!("unable to create the Display P3 color space");
            }
        }
        // Support direct-to-display rendering if the window is not transparent
        // https://developer.apple.com/documentation/metal/managing-your-game-window-for-metal-in-macos
        layer.setOpaque(!transparent);
        layer.setMaximumDrawableCount(3);
        // Allow texture reading for visual tests and UI automation screenshots
        // (captures without ScreenCaptureKit). Only debug builds pay the
        // presentation cost, even when `test-support` is compiled in.
        #[cfg(all(feature = "test-support", debug_assertions))]
        layer.setFramebufferOnly(false);
        layer.setAllowsNextDrawableTimeout(false);
        layer.setNeedsDisplayOnBoundsChange(true);
        // UIKit sizes a view's backing layer itself; only AppKit-hosted
        // layers need to track their superlayer's bounds.
        #[cfg(target_os = "macos")]
        layer.setAutoresizingMask(
            objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
                | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
        );
    }

    /// Creates a new headless MetalRenderer for offscreen rendering without a window.
    ///
    /// This renderer can render scenes to images without requiring a CAMetalLayer,
    /// window, or AppKit. Use `render_scene_to_image()` to render scenes.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn new_headless(instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>) -> Result<Self> {
        let device = Self::create_device()?;
        Self::new_internal(device, None, true, instance_buffer_pool)
    }

    #[cfg(target_os = "macos")]
    fn create_device() -> Result<Retained<ProtocolObject<dyn MTLDevice>>> {
        // Prefer low‐power integrated GPUs on Intel Mac. On Apple
        // Silicon, there is only ever one GPU, so this is equivalent to
        // `metal::Device::system_default()`.
        if let Some(device) = MTLCopyAllDevices()
            .iter()
            .min_by_key(|device| (device.isRemovable(), !device.isLowPower()))
        {
            Ok(device)
        } else {
            // For some reason `all()` can return an empty list, see https://github.com/zed-industries/zed/issues/37689
            // In that case, we fall back to the system default device.
            log::error!(
                "Unable to enumerate Metal devices; attempting to use system default device"
            );
            Self::system_default_device()
        }
    }

    #[cfg(target_os = "ios")]
    fn create_device() -> Result<Retained<ProtocolObject<dyn MTLDevice>>> {
        Self::system_default_device()
    }

    fn system_default_device() -> Result<Retained<ProtocolObject<dyn MTLDevice>>> {
        MTLCreateSystemDefaultDevice().context("accessing a compatible Metal device")
    }

    fn new_internal(
        device: Retained<ProtocolObject<dyn MTLDevice>>,
        layer: Option<Retained<CAMetalLayer>>,
        opaque: bool,
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    ) -> Result<Self> {
        #[cfg(feature = "runtime_shaders")]
        let library = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(SHADERS_SOURCE_FILE),
                Some(&objc2_metal::MTLCompileOptions::new()),
            )
            .context("building Metal library")?;
        #[cfg(not(feature = "runtime_shaders"))]
        let library = device
            .newLibraryWithData_error(&dispatch2::DispatchData::from_static_bytes(
                SHADERS_METALLIB,
            ))
            .context("loading Metal library")?;

        fn to_float2_bits(point: PointF) -> u64 {
            let mut output = point.y.to_bits() as u64;
            output <<= 32;
            output |= point.x.to_bits() as u64;
            output
        }

        // Shared memory can be used only if CPU and GPU share the same memory space.
        // https://developer.apple.com/documentation/metal/setting-resource-storage-modes
        // iOS does not support managed resources. Its simulator may report a
        // non-unified host GPU even though resources must still use shared storage.
        let is_unified_memory = cfg!(target_os = "ios") || device.hasUnifiedMemory();
        // Apple GPU families support memoryless textures, which can significantly reduce
        // memory usage by keeping render targets in on-chip tile memory instead of
        // allocating backing store in system memory.
        // https://developer.apple.com/documentation/metal/mtlgpufamily
        let is_apple_gpu = device.supportsFamily(MTLGPUFamily::Apple1);

        let unit_vertices = [
            to_float2_bits(point(0., 0.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(1., 1.)),
        ];
        // SAFETY: Metal copies the live array using its exact byte length.
        let unit_vertices = unsafe {
            device.newBufferWithBytes_length_options(
                ptr::NonNull::from(&unit_vertices).cast(),
                mem::size_of_val(&unit_vertices),
                if is_unified_memory {
                    MTLResourceOptions::StorageModeShared
                        | MTLResourceOptions::CPUCacheModeWriteCombined
                } else {
                    MTLResourceOptions::StorageModeManaged
                },
            )
        }
        .context("allocating unit vertex buffer")?;

        let paths_rasterization_pipeline_state = build_path_rasterization_pipeline_state(
            &device,
            &library,
            "paths_rasterization",
            "path_rasterization_vertex",
            "path_rasterization_fragment",
            MTLPixelFormat::BGRA8Unorm,
            PATH_SAMPLE_COUNT,
        )?;
        let target_format = drawable_pixel_format();
        let path_sprites_pipeline_state = build_path_sprite_pipeline_state(
            &device,
            &library,
            "path_sprites",
            "path_sprite_vertex",
            "path_sprite_fragment",
            target_format,
        )?;
        let shadows_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "shadows",
            "shadow_vertex",
            "shadow_fragment",
            target_format,
        )?;
        let quads_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "quads",
            "quad_vertex",
            "quad_fragment",
            target_format,
        )?;
        let holes_pipeline_state = build_hole_pipeline_state(&device, &library, target_format)?;
        let underlines_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "underlines",
            "underline_vertex",
            "underline_fragment",
            target_format,
        )?;
        let monochrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "monochrome_sprites",
            "monochrome_sprite_vertex",
            "monochrome_sprite_fragment",
            target_format,
        )?;
        let polychrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "polychrome_sprites",
            "polychrome_sprite_vertex",
            "polychrome_sprite_fragment",
            target_format,
        )?;
        let surfaces_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "surfaces",
            "surface_vertex",
            "surface_fragment",
            target_format,
        )?;

        let fast_occlusion =
            crate::fast::occlusion::Occlusion::new(&device, &library, target_format, is_apple_gpu)?;
        let fast_backdrop = crate::fast::backdrop::Backdrop::new(&device, &library, target_format)?;
        let command_queue = device.newCommandQueue().context("creating command queue")?;
        let supports_shared_storage = cfg!(target_os = "ios") || is_apple_gpu;
        #[expect(
            clippy::arc_with_non_send_sync,
            reason = "PlatformWindow::sprite_atlas requires Arc, not Send or Sync"
        )]
        let sprite_atlas = Arc::new(MetalAtlas::new(device.clone(), supports_shared_storage));
        let mut cache = ptr::null_mut();
        // SAFETY: The output pointer is writable and both optional attribute dictionaries are absent.
        let status = unsafe {
            CVMetalTextureCache::create(None, None, &device, None, ptr::NonNull::from(&mut cache))
        };
        anyhow::ensure!(
            status == kCVReturnSuccess,
            "creating CoreVideo Metal texture cache: status {status}"
        );
        let cache = ptr::NonNull::new(cache).context("retrieving CoreVideo Metal texture cache")?;
        // SAFETY: The successful create call transfers ownership of this non-null cache.
        let core_video_texture_cache = unsafe { CFRetained::from_raw(cache) };

        Ok(Self {
            device,
            layer,
            presents_with_transaction: false,
            presented_frame_sink: None,
            presentations: Arc::default(),
            drawables_report_presentation: None,
            pacer: PresentationPacer::new(DEFAULT_REFRESH),
            is_apple_gpu,
            is_unified_memory,
            opaque,
            command_queue,
            paths_rasterization_pipeline_state,
            path_sprites_pipeline_state,
            shadows_pipeline_state,
            quads_pipeline_state,
            holes_pipeline_state,
            fast_occlusion,
            fast_backdrop,
            underlines_pipeline_state,
            monochrome_sprites_pipeline_state,
            polychrome_sprites_pipeline_state,
            surfaces_pipeline_state,
            unit_vertices,
            instance_buffer_pool,
            sprite_atlas,
            surfaces_in_flight: Default::default(),
            core_video_texture_cache,
            surface_skip_logged: false,
            path_intermediate_texture: None,
            path_intermediate_msaa_texture: None,
            path_sample_count: PATH_SAMPLE_COUNT,
            #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
            headless_render_target: None,
            fast_layers: crate::fast::layers::TileCache::default(),
        })
    }

    pub fn layer(&self) -> Option<&CAMetalLayer> {
        self.layer.as_deref()
    }

    pub fn layer_ptr(&self) -> *mut CAMetalLayer {
        self.layer
            .as_ref()
            .map(|layer| Retained::as_ptr(layer).cast_mut())
            .unwrap_or(ptr::null_mut())
    }

    pub fn sprite_atlas(&self) -> &Arc<MetalAtlas> {
        &self.sprite_atlas
    }

    pub fn presents_with_transaction(&self) -> bool {
        self.presents_with_transaction
    }

    pub fn set_presents_with_transaction(&mut self, presents_with_transaction: bool) {
        self.presents_with_transaction = presents_with_transaction;
        if let Some(layer) = &self.layer {
            layer.setPresentsWithTransaction(presents_with_transaction);
        }
    }

    /// Reports every frame [`Self::draw`] presents to `sink`, or stops with `None`. The sink
    /// runs on a Metal thread.
    pub fn set_presented_frame_sink(&mut self, sink: Option<PresentedFrameSink>) {
        self.presented_frame_sink = sink;
    }

    /// The display's shortest refresh interval, which paces [`Self::ready_for_vsync_frame`].
    pub fn set_refresh_interval(&mut self, refresh: Duration) {
        self.pacer.set_refresh(refresh);
    }

    /// Whether a display-link tick should draw, or leave its demand to the next tick so the
    /// frames already queued for the display can drain; see `presentation_pacing`.
    pub fn ready_for_vsync_frame(&mut self) -> bool {
        for frame in self.presentations.lock().drain(..) {
            self.pacer.presented(frame);
        }
        self.pacer.should_draw(Instant::now())
    }

    /// Whether no frame began within the last refresh, so one drawn now, off the vsync
    /// tick, cannot land in the same refresh as the previous one.
    pub fn idle_for_a_refresh(&self) -> bool {
        self.pacer.idle_for_a_refresh(Instant::now())
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        if let Some(layer) = &self.layer {
            layer.setDrawableSize(CGSize::new(size.width.0 as f64, size.height.0 as f64));
        }
        if let Err(error) = self.update_path_intermediate_textures(size) {
            log::error!("failed to resize path textures: {error:#}");
        }
        crate::fast::layers::TileCache::clear(&mut self.fast_layers);
    }

    pub(crate) fn update_path_intermediate_textures(
        &mut self,
        size: Size<DevicePixels>,
    ) -> Result<()> {
        // We are uncertain when this happens, but sometimes size can be 0 here. Most likely before
        // the layout pass on window creation. Zero-sized texture creation causes SIGABRT.
        // https://github.com/zed-industries/zed/issues/36229
        self.path_intermediate_texture = None;
        self.path_intermediate_msaa_texture = None;
        if size.width.0 <= 0 || size.height.0 <= 0 {
            return Ok(());
        }

        let texture_descriptor = MTLTextureDescriptor::new();
        // SAFETY: The dimensions are positive and this descriptor creates a 2D texture.
        unsafe {
            texture_descriptor.setWidth(size.width.0 as usize);
            texture_descriptor.setHeight(size.height.0 as usize);
        }
        texture_descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        texture_descriptor.setStorageMode(MTLStorageMode::Private);
        texture_descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        let intermediate_texture = self
            .device
            .newTextureWithDescriptor(&texture_descriptor)
            .context("allocating path intermediate texture")?;

        let msaa_texture = if self.path_sample_count > 1 {
            // https://developer.apple.com/documentation/metal/choosing-a-resource-storage-mode-for-apple-gpus
            // Rendering MSAA textures are done in a single pass, so we can use memory-less storage on Apple Silicon
            let storage_mode = if self.is_apple_gpu {
                MTLStorageMode::Memoryless
            } else {
                MTLStorageMode::Private
            };

            let msaa_descriptor = texture_descriptor;
            msaa_descriptor.setTextureType(MTLTextureType::Type2DMultisample);
            msaa_descriptor.setStorageMode(storage_mode);
            // SAFETY: The multisample texture uses the same sample count as its render pipeline.
            unsafe {
                msaa_descriptor.setSampleCount(self.path_sample_count as _);
            }
            Some(
                self.device
                    .newTextureWithDescriptor(&msaa_descriptor)
                    .context("allocating multisample path texture")?,
            )
        } else {
            None
        };
        self.path_intermediate_texture = Some(intermediate_texture);
        self.path_intermediate_msaa_texture = msaa_texture;
        Ok(())
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        self.opaque = !transparent;
        if let Some(layer) = &self.layer {
            layer.setOpaque(!transparent);
        }
    }

    pub fn destroy(&self) {
        // nothing to do
    }

    pub fn draw(&mut self, scene: &Scene) {
        let layer = match &self.layer {
            Some(l) => l.clone(),
            None => {
                log::error!(
                    "draw() called on headless renderer - use render_scene_to_image() instead"
                );
                return;
            }
        };
        let viewport_size = layer.drawableSize();
        let viewport_size: Size<DevicePixels> = size(
            (viewport_size.width.ceil() as i32).into(),
            (viewport_size.height.ceil() as i32).into(),
        );
        crate::fast::backdrop::prepare_layer(self, &layer, scene);
        let drawable = if let Some(drawable) = layer.nextDrawable() {
            drawable
        } else {
            log::error!(
                "failed to retrieve next drawable, drawable size: {:?}",
                viewport_size
            );
            return;
        };

        let command_buffer = match self.render_frame(scene, &drawable.texture(), viewport_size) {
            Ok(command_buffer) => command_buffer,
            Err(error) => {
                log::error!("failed to render: {error:#}");
                return;
            }
        };

        let reports_presentation = *self.drawables_report_presentation.get_or_insert_with(|| {
            objc2::runtime::NSObjectProtocol::respondsToSelector(
                &*drawable,
                objc2::sel!(addPresentedHandler:),
            )
        });
        if reports_presentation {
            observe_presentation(
                ProtocolObject::from_ref(&*drawable),
                self.presentations.clone(),
                self.presented_frame_sink.clone(),
            );
        }
        self.pacer.submitted(Instant::now());
        if self.presents_with_transaction {
            command_buffer.commit();
            command_buffer.waitUntilScheduled();
            drawable.present();
        } else {
            command_buffer.presentDrawable(ProtocolObject::from_ref(&*drawable));
            command_buffer.commit();
        }
    }

    pub(crate) fn render_frame(
        &mut self,
        scene: &Scene,
        texture: &ProtocolObject<dyn MTLTexture>,
        viewport_size: Size<DevicePixels>,
    ) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        if self.path_intermediate_texture.is_none() {
            self.update_path_intermediate_textures(viewport_size)?;
        }
        crate::fast::backdrop::Backdrop::begin_frame(&mut self.fast_backdrop, scene);
        let mut writer = InstanceBufferWriter::new(
            &self.device,
            &self.instance_buffer_pool,
            self.is_unified_memory,
        )?;
        let instance_bindings = write_instances(scene, &mut writer).with_context(|| {
            format!(
                "writing instance data: {} paths, {} shadows, {} quads, {} underlines, {} mono, {} poly, {} surfaces",
                scene.paths.len(),
                scene.shadows.len(),
                scene.quads.len(),
                scene.underlines.len(),
                scene.monochrome_sprites.len(),
                scene.polychrome_sprites.len(),
                scene.surfaces.len(),
            )
        })?;
        crate::fast::surfaces::SurfacesInFlight::release_finished(&mut self.surfaces_in_flight);
        // Housekeeping CoreVideo asks for periodically: textures no command buffer holds any
        // more give their buffers back.
        self.core_video_texture_cache.flush(0);
        let mut surface_textures = Vec::new();
        crate::fast::layers::raster::rasterize_tiles(self, scene, &mut surface_textures);
        let command_buffer = self.draw_primitives_to_texture(
            scene,
            &instance_bindings,
            &mut writer,
            texture,
            viewport_size,
            &mut surface_textures,
        )?;
        let surfaces_done = crate::fast::surfaces::SurfacesInFlight::hold(
            &mut self.surfaces_in_flight,
            surface_textures,
        );

        let instance_buffer_pool = self.instance_buffer_pool.clone();
        let frame_resources = Cell::new(Some(writer.finish()));
        let block = RcBlock::new(
            move |_: ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                if let Some(instance_buffer) = frame_resources.take() {
                    instance_buffer_pool.lock().release(instance_buffer);
                }
                if let Some(done) = &surfaces_done {
                    crate::fast::surfaces::FrameDone::set(done);
                }
            },
        );
        // SAFETY: Metal copies the live block and invokes it once after this frame completes.
        // Its captures own the frame resources and use the pool mutex when returning the buffer.
        unsafe {
            command_buffer.addCompletedHandler(RcBlock::as_ptr(&block));
        }

        Ok(command_buffer)
    }

    /// Renders the scene to a texture and returns the pixel data as an RGBA image.
    /// This does not present the frame to screen - useful for visual testing
    /// where we want to capture what would be rendered without displaying it.
    ///
    /// Note: This requires a layer-backed renderer. For headless rendering,
    /// use `render_scene_to_image()` instead.
    #[cfg(any(test, feature = "test-support"))]
    pub fn render_to_image(&mut self, scene: &Scene) -> Result<RgbaImage> {
        let layer = self
            .layer
            .clone()
            .ok_or_else(|| anyhow::anyhow!("render_to_image requires a layer-backed renderer"))?;
        let viewport_size = layer.drawableSize();
        let viewport_size: Size<DevicePixels> = size(
            (viewport_size.width.ceil() as i32).into(),
            (viewport_size.height.ceil() as i32).into(),
        );
        let drawable = layer
            .nextDrawable()
            .ok_or_else(|| anyhow::anyhow!("Failed to get drawable for render_to_image"))?;

        let command_buffer = self.render_frame(scene, &drawable.texture(), viewport_size)?;

        // Commit and wait for completion without presenting
        command_buffer.commit();
        command_buffer.waitUntilCompleted();

        read_texture_to_image(&drawable.texture())
    }

    /// Renders a scene to an image without requiring a window or CAMetalLayer.
    ///
    /// This is the primary method for headless rendering. It creates an offscreen
    /// texture, renders the scene to it, and returns the pixel data as an RGBA image.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> Result<RgbaImage> {
        if size.width.0 <= 0 || size.height.0 <= 0 {
            anyhow::bail!("Invalid size for render_scene_to_image: {:?}", size);
        }

        // Headless callers do not have a Cocoa event-loop pool to release
        // autoreleased command buffers and render-pass descriptors.
        objc2::rc::autoreleasepool(|_| {
            // Update path intermediate textures for this size
            self.update_path_intermediate_textures(size)?;

            // Create an offscreen texture as render target
            let texture_descriptor = MTLTextureDescriptor::new();
            unsafe {
                texture_descriptor.setWidth(size.width.0 as usize);
                texture_descriptor.setHeight(size.height.0 as usize);
            }
            texture_descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            texture_descriptor
                .setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            // Like the atlas, only Apple GPUs can create shared textures on macOS;
            // Intel Macs cannot, even with unified memory. iOS has no managed storage.
            let uses_shared_storage = cfg!(target_os = "ios") || self.is_apple_gpu;
            texture_descriptor.setStorageMode(if uses_shared_storage {
                MTLStorageMode::Shared
            } else {
                MTLStorageMode::Managed
            });
            let target_texture = self
                .device
                .newTextureWithDescriptor(&texture_descriptor)
                .context("allocating headless render target")?;

            let command_buffer = self.render_frame(scene, &target_texture, size)?;

            // Managed textures require an explicit blit synchronize before the CPU
            // can read back the rendered data. Without this, get_bytes returns
            // stale zeros.
            if !uses_shared_storage {
                let blit = command_buffer
                    .blitCommandEncoder()
                    .context("creating blit command encoder")?;
                blit.synchronizeResource(ProtocolObject::from_ref(&*target_texture));
                blit.endEncoding();
            }

            // Commit and wait for completion
            command_buffer.commit();
            command_buffer.waitUntilCompleted();

            read_texture_to_image(&target_texture)
        })
    }

    /// Renders a scene to a reused offscreen texture without reading pixels
    /// back or blocking on GPU completion.
    ///
    /// This mirrors the CPU cost of presenting a frame to a window (scene
    /// encoding, instance buffer writes, command submission) and is used by
    /// headless benchmark rendering, where the produced pixels are never
    /// inspected.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> Result<()> {
        if size.width.0 <= 0 || size.height.0 <= 0 {
            anyhow::bail!("Invalid size for render_scene: {:?}", size);
        }

        objc2::rc::autoreleasepool(|_| {
            self.update_path_intermediate_textures(size)?;

            let needs_new_target = self.headless_render_target.as_ref().is_none_or(|texture| {
                texture.width() != size.width.0 as usize
                    || texture.height() != size.height.0 as usize
            });
            if needs_new_target {
                let texture_descriptor = MTLTextureDescriptor::new();
                unsafe {
                    texture_descriptor.setWidth(size.width.0 as usize);
                    texture_descriptor.setHeight(size.height.0 as usize);
                }
                texture_descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                texture_descriptor
                    .setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
                texture_descriptor.setStorageMode(MTLStorageMode::Private);
                self.headless_render_target = Some(
                    self.device
                        .newTextureWithDescriptor(&texture_descriptor)
                        .context("allocating headless render target")?,
                );
            }
            let target_texture = self
                .headless_render_target
                .clone()
                .context("retrieving headless render target")?;

            let command_buffer = self.render_frame(scene, &target_texture, size)?;

            // Commit without waiting, mirroring presentation to a real window where
            // the CPU doesn't block on the GPU.
            command_buffer.commit();
            Ok(())
        })
    }

    fn draw_primitives_to_texture(
        &mut self,
        scene: &Scene,
        instance_bindings: &InstanceBindings,
        writer: &mut InstanceBufferWriter,
        texture: &ProtocolObject<dyn MTLTexture>,
        viewport_size: Size<DevicePixels>,
        surface_textures: &mut Vec<CFRetained<CVMetalTexture>>,
    ) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        if let Some(command_buffer) = crate::fast::paths::draw_primitives_to_texture(
            self,
            scene,
            instance_bindings,
            writer,
            texture,
            viewport_size,
            surface_textures,
        )? {
            return Ok(command_buffer);
        }
        let command_queue = self.command_queue.clone();
        let command_buffer = command_queue
            .commandBuffer()
            .context("creating command buffer")?;
        let alpha = if self.opaque { 1. } else { 0. };

        let stencil = crate::fast::occlusion::Occlusion::stencil_for(
            &mut self.fast_occlusion,
            &self.device,
            texture,
        )?;
        let mut command_encoder = new_command_encoder_for_texture(
            &command_buffer,
            texture,
            &stencil,
            viewport_size,
            Some(MTLClearColor {
                red: 0.,
                green: 0.,
                blue: 0.,
                alpha,
            }),
        )?;

        for batch in scene.composed_batches() {
            let batch = match batch {
                ComposedBatch::Primitives(batch) => batch,
                ComposedBatch::Holes(range) => {
                    self.draw_holes(range, instance_bindings, viewport_size, &command_encoder);
                    continue;
                }
                // Frames with blurs are drawn by fast::paths.
                ComposedBatch::Blurs(_) => continue,
            };
            match batch {
                PrimitiveBatch::Shadows(range) => {
                    self.draw_shadows(range, instance_bindings, viewport_size, &command_encoder)
                }
                PrimitiveBatch::Quads(range) => {
                    self.draw_quads(range, instance_bindings, viewport_size, &command_encoder)
                }
                PrimitiveBatch::Paths(range) => {
                    let paths = &scene.paths[range];
                    command_encoder.endEncoding();

                    let did_draw = self.draw_paths_to_intermediate(
                        paths,
                        writer,
                        viewport_size,
                        &command_buffer,
                        &instance_bindings.fades,
                    )?;

                    command_encoder = new_command_encoder_for_texture(
                        &command_buffer,
                        texture,
                        &stencil,
                        viewport_size,
                        None,
                    )?;

                    if did_draw {
                        if let Err(error) = self.draw_paths_from_intermediate(
                            paths,
                            writer,
                            viewport_size,
                            &command_encoder,
                        ) {
                            command_encoder.endEncoding();
                            return Err(error);
                        }
                    }
                }
                PrimitiveBatch::Underlines(range) => {
                    self.draw_underlines(range, instance_bindings, viewport_size, &command_encoder)
                }
                PrimitiveBatch::MonochromeSprites { texture_id, range } => self
                    .draw_monochrome_sprites(
                        texture_id,
                        range,
                        instance_bindings,
                        viewport_size,
                        &command_encoder,
                    ),
                PrimitiveBatch::PolychromeSprites { texture_id, range } => self
                    .draw_polychrome_sprites(
                        texture_id,
                        range,
                        instance_bindings,
                        viewport_size,
                        &command_encoder,
                    ),
                PrimitiveBatch::Surfaces(range) => {
                    if let Err(error) = self.draw_surfaces(
                        &scene.surfaces[range.clone()],
                        range.start,
                        instance_bindings,
                        viewport_size,
                        &command_encoder,
                        surface_textures,
                    ) {
                        command_encoder.endEncoding();
                        return Err(error);
                    }
                }
                PrimitiveBatch::SubpixelSprites { .. } => unreachable!(),
            }
        }

        command_encoder.endEncoding();

        Ok(command_buffer)
    }

    fn draw_paths_to_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        writer: &mut InstanceBufferWriter,
        viewport_size: Size<DevicePixels>,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        fast_fades: &InstanceBinding,
    ) -> Result<bool> {
        if paths.is_empty() {
            return Ok(false);
        }
        let intermediate_texture = self
            .path_intermediate_texture
            .as_ref()
            .context("missing path intermediate texture")?;

        let mut vertices = Vec::new();
        for path in paths {
            vertices.extend(path.vertices.iter().map(|v| PathRasterizationVertex {
                xy_position: v.xy_position,
                st_position: v.st_position,
                color: path.color,
                bounds: path.bounds.intersect(&path.content_mask.bounds),
            }));
        }
        let vertex_instance_bindings = writer.write(&vertices)?;

        let render_pass_descriptor = MTLRenderPassDescriptor::new();
        // SAFETY: Index zero is within the render pass's color attachment array.
        let color_attachment = unsafe {
            render_pass_descriptor
                .colorAttachments()
                .objectAtIndexedSubscript(0)
        };
        color_attachment.setLoadAction(MTLLoadAction::Clear);
        color_attachment.setClearColor(MTLClearColor {
            red: 0.,
            green: 0.,
            blue: 0.,
            alpha: 0.,
        });

        if let Some(msaa_texture) = &self.path_intermediate_msaa_texture {
            color_attachment.setTexture(Some(msaa_texture));
            color_attachment.setResolveTexture(Some(intermediate_texture));
            color_attachment.setStoreAction(MTLStoreAction::MultisampleResolve);
        } else {
            color_attachment.setTexture(Some(intermediate_texture));
            color_attachment.setStoreAction(MTLStoreAction::Store);
        }

        let command_encoder = command_buffer
            .renderCommandEncoderWithDescriptor(&render_pass_descriptor)
            .context("creating path render command encoder")?;
        command_encoder.setRenderPipelineState(&self.paths_rasterization_pipeline_state);
        crate::fast::edge_fade::bind(&command_encoder, fast_fades);
        // SAFETY: The binding contains vertices.len() shader-compatible vertices at an aligned offset.
        // Metal retains the buffer, and writer.finish publishes CPU writes before submission.
        // The viewport bytes are copied from a live value into the matching shader slot.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&vertex_instance_bindings.buffer),
                vertex_instance_bindings.offset,
                PathRasterizationInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                PathRasterizationInputIndex::ViewportSize as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&vertex_instance_bindings.buffer),
                vertex_instance_bindings.offset,
                PathRasterizationInputIndex::Vertices as usize,
            );
            command_encoder.drawPrimitives_vertexStart_vertexCount(
                MTLPrimitiveType::Triangle,
                0,
                vertices.len(),
            );
        }

        command_encoder.endEncoding();
        Ok(true)
    }

    pub(crate) fn draw_shadows(
        &self,
        shadows: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if shadows.is_empty() {
            return;
        }

        command_encoder.setRenderPipelineState(&self.shadows_pipeline_state);
        crate::fast::edge_fade::bind(command_encoder, &instance_bindings.fades);
        // SAFETY: Six unit vertices and the scene's shadow range match the uploaded shader layouts and slots.
        // Metal retains the buffers, writer.finish publishes their writes, and viewport bytes are copied.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                ShadowInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.shadows.buffer),
                instance_bindings.shadows.offset,
                ShadowInputIndex::Shadows as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.shadows.buffer),
                instance_bindings.shadows.offset,
                ShadowInputIndex::Shadows as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                ShadowInputIndex::ViewportSize as usize,
            );
            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                shadows.len(),
                shadows.start,
            );
        }
    }

    pub(crate) fn draw_quads(
        &self,
        quads: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if quads.is_empty() {
            return;
        }

        command_encoder.setRenderPipelineState(&self.quads_pipeline_state);
        crate::fast::edge_fade::bind(command_encoder, &instance_bindings.fades);
        // SAFETY: Six unit vertices and the scene's quad range match the uploaded shader layouts and slots.
        // Metal retains the buffers, writer.finish publishes their writes, and viewport bytes are copied.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                QuadInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.quads.buffer),
                instance_bindings.quads.offset,
                QuadInputIndex::Quads as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.quads.buffer),
                instance_bindings.quads.offset,
                QuadInputIndex::Quads as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                QuadInputIndex::ViewportSize as usize,
            );
            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                quads.len(),
                quads.start,
            );
        }
    }

    /// Draws the holes `range` of the scene's natives: each clears GPUI's pixels under it
    /// by its coverage times the native's opacity, so the native under GPUI's layer shows.
    pub(crate) fn draw_holes(
        &self,
        holes: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if holes.is_empty() {
            return;
        }
        command_encoder.setRenderPipelineState(&self.holes_pipeline_state);
        crate::fast::edge_fade::bind(command_encoder, &instance_bindings.fades);
        // SAFETY: Six unit vertices and the scene's hole range match the quad shader's layouts
        // and slots. Metal retains the buffers, writer.finish publishes their writes, and the
        // viewport bytes are copied.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                QuadInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.holes.buffer),
                instance_bindings.holes.offset,
                QuadInputIndex::Quads as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.holes.buffer),
                instance_bindings.holes.offset,
                QuadInputIndex::Quads as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                QuadInputIndex::ViewportSize as usize,
            );
            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                holes.len(),
                holes.start,
            );
        }
    }

    pub(crate) fn draw_paths_from_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        writer: &mut InstanceBufferWriter,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) -> Result<()> {
        let (Some(first_path), Some(last_path)) = (paths.first(), paths.last()) else {
            return Ok(());
        };
        let intermediate_texture = self
            .path_intermediate_texture
            .as_ref()
            .context("missing path intermediate texture")?;

        command_encoder.setRenderPipelineState(&self.path_sprites_pipeline_state);
        // SAFETY: The shader slots match the unit vertices, copied viewport, and retained path texture.
        // The preceding render pass writes this tracked texture before it is sampled.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                SpriteInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                SpriteInputIndex::ViewportSize as usize,
            );
            command_encoder.setFragmentTexture_atIndex(
                Some(intermediate_texture),
                SpriteInputIndex::AtlasTexture as usize,
            );
        }

        // When copying paths from the intermediate texture to the drawable,
        // each pixel must only be copied once, in case of transparent paths.
        //
        // If all paths have the same draw order, then their bounds are all
        // disjoint, so we can copy each path's bounds individually. If this
        // batch combines different draw orders, we perform a single copy
        // for a minimal spanning rect.
        let sprites;
        if last_path.order == first_path.order {
            sprites = paths
                .iter()
                .map(|path| PathSprite {
                    bounds: path.clipped_bounds(),
                })
                .collect();
        } else {
            let mut bounds = first_path.clipped_bounds();
            for path in paths.iter().skip(1) {
                bounds = bounds.union(&path.clipped_bounds());
            }
            sprites = vec![PathSprite { bounds }];
        }

        let sprite_instance_bindings = writer.write(&sprites)?;
        // SAFETY: The draw uses six unit vertices and exactly the sprites written at this aligned binding.
        // Metal retains the buffer, and writer.finish publishes CPU writes before submission.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&sprite_instance_bindings.buffer),
                sprite_instance_bindings.offset,
                SpriteInputIndex::Sprites as usize,
            );
            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(
                MTLPrimitiveType::Triangle,
                0,
                6,
                sprites.len(),
            );
        }
        Ok(())
    }

    pub(crate) fn draw_underlines(
        &self,
        underlines: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if underlines.is_empty() {
            return;
        }

        command_encoder.setRenderPipelineState(&self.underlines_pipeline_state);
        crate::fast::edge_fade::bind(command_encoder, &instance_bindings.fades);
        // SAFETY: Six unit vertices and the scene's underline range match the uploaded shader layouts and slots.
        // Metal retains the buffers, writer.finish publishes their writes, and viewport bytes are copied.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                UnderlineInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.underlines.buffer),
                instance_bindings.underlines.offset,
                UnderlineInputIndex::Underlines as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.underlines.buffer),
                instance_bindings.underlines.offset,
                UnderlineInputIndex::Underlines as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                UnderlineInputIndex::ViewportSize as usize,
            );
            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                underlines.len(),
                underlines.start,
            );
        }
    }

    pub(crate) fn draw_monochrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if sprites.is_empty() {
            return;
        }

        let Some(texture) = self.sprite_atlas.metal_texture(texture_id) else {
            return;
        };
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.setRenderPipelineState(&self.monochrome_sprites_pipeline_state);
        crate::fast::edge_fade::bind(command_encoder, &instance_bindings.fades);
        // SAFETY: The scene's sprite range, six unit vertices, and copied sizes match the shader layouts and slots.
        // Metal retains the buffers and texture, and writer.finish publishes the instance writes.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                SpriteInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.monochrome_sprites.buffer),
                instance_bindings.monochrome_sprites.offset,
                SpriteInputIndex::Sprites as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                SpriteInputIndex::ViewportSize as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&texture_size).cast(),
                mem::size_of_val(&texture_size),
                SpriteInputIndex::AtlasTextureSize as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.monochrome_sprites.buffer),
                instance_bindings.monochrome_sprites.offset,
                SpriteInputIndex::Sprites as usize,
            );
            command_encoder.setFragmentTexture_atIndex(
                Some(&texture),
                SpriteInputIndex::AtlasTexture as usize,
            );
            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                sprites.len(),
                sprites.start,
            );
        }
    }

    pub(crate) fn draw_polychrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if sprites.is_empty() {
            return;
        }
        if crate::fast::layers::composite::draw_tiles(
            self,
            texture_id,
            &sprites,
            instance_bindings,
            viewport_size,
            command_encoder,
            &mut binds::Binds::default(),
        ) {
            return;
        }

        let Some(texture) = self.sprite_atlas.metal_texture(texture_id) else {
            return;
        };
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.setRenderPipelineState(&self.polychrome_sprites_pipeline_state);
        crate::fast::edge_fade::bind(command_encoder, &instance_bindings.fades);
        // SAFETY: The scene's sprite range, six unit vertices, and copied sizes match the shader layouts and slots.
        // Metal retains the buffers and texture, and writer.finish publishes the instance writes.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                SpriteInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.polychrome_sprites.buffer),
                instance_bindings.polychrome_sprites.offset,
                SpriteInputIndex::Sprites as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                SpriteInputIndex::ViewportSize as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&texture_size).cast(),
                mem::size_of_val(&texture_size),
                SpriteInputIndex::AtlasTextureSize as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.polychrome_sprites.buffer),
                instance_bindings.polychrome_sprites.offset,
                SpriteInputIndex::Sprites as usize,
            );
            command_encoder.setFragmentTexture_atIndex(
                Some(&texture),
                SpriteInputIndex::AtlasTexture as usize,
            );
            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                sprites.len(),
                sprites.start,
            );
        }
    }

    pub(crate) fn draw_surfaces(
        &mut self,
        surfaces: &[PaintSurface],
        first_surface: usize,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        surface_textures: &mut Vec<CFRetained<CVMetalTexture>>,
    ) -> Result<()> {
        if surfaces.is_empty() {
            return Ok(());
        }

        command_encoder.setRenderPipelineState(&self.surfaces_pipeline_state);
        // SAFETY: The unit vertices, SurfaceBounds instances, and copied viewport match the shader layouts and slots.
        // Metal retains the buffers, and writer.finish publishes CPU writes before submission.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                SurfaceInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.surfaces.buffer),
                instance_bindings.surfaces.offset,
                SurfaceInputIndex::Surfaces as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                SurfaceInputIndex::ViewportSize as usize,
            );
        }

        for (index, surface) in surfaces.iter().enumerate() {
            let image_buffer = &surface.image_buffer;
            let texture_size = size(
                DevicePixels::from(CVPixelBufferGetWidth(image_buffer) as i32),
                DevicePixels::from(CVPixelBufferGetHeight(image_buffer) as i32),
            );

            let format = CVPixelBufferGetPixelFormatType(image_buffer);
            let Some(layout) = SurfaceLayout::of(format) else {
                self.skip_surface(format_args!(
                    "pixel format {format:#x} is not an 8- or 10-bit Y′CbCr bi-planar one"
                ));
                continue;
            };
            let ycbcr_to_rgb = ycbcr_to_rgb(surface_matrix(image_buffer), layout);
            let (luma_format, chroma_format) = layout.depth.plane_formats();

            let plane_texture = |plane, format| -> Result<CFRetained<CVMetalTexture>> {
                let width = CVPixelBufferGetWidthOfPlane(image_buffer, plane);
                let height = CVPixelBufferGetHeightOfPlane(image_buffer, plane);
                let mut texture = ptr::null_mut();
                // SAFETY: The buffer has two planes matching these formats and dimensions, and the output pointer is writable.
                let status = unsafe {
                    CVMetalTextureCache::create_texture_from_image(
                        None,
                        &self.core_video_texture_cache,
                        image_buffer,
                        None,
                        format,
                        width,
                        height,
                        plane,
                        ptr::NonNull::from(&mut texture),
                    )
                };
                anyhow::ensure!(
                    status == kCVReturnSuccess,
                    "creating CoreVideo Metal texture for plane {plane}: status {status}"
                );
                let texture =
                    ptr::NonNull::new(texture).context("retrieving CoreVideo Metal texture")?;
                // SAFETY: The successful create call transfers ownership of this non-null texture.
                Ok(unsafe { CFRetained::from_raw(texture) })
            };
            let textures = (|| {
                let y_texture = plane_texture(0, luma_format)?;
                let cb_cr_texture = plane_texture(1, chroma_format)?;
                let y_metal_texture = CVMetalTextureGetTexture(&y_texture)
                    .context("retrieving luma Metal texture")?;
                let cb_cr_metal_texture = CVMetalTextureGetTexture(&cb_cr_texture)
                    .context("retrieving chroma Metal texture")?;
                anyhow::Ok((
                    y_texture,
                    cb_cr_texture,
                    y_metal_texture,
                    cb_cr_metal_texture,
                ))
            })();
            let (y_texture, cb_cr_texture, y_metal_texture, cb_cr_metal_texture) = match textures {
                Ok(textures) => textures,
                Err(error) => {
                    self.skip_surface(format_args!("{error:#}"));
                    continue;
                }
            };

            // SAFETY: The size and matrix are copied from live values, and both planes match their shader texture slots.
            // fast::surfaces holds the CoreVideo wrappers until GPU reads finish.
            unsafe {
                command_encoder.setVertexBytes_length_atIndex(
                    ptr::NonNull::from(&texture_size).cast(),
                    mem::size_of_val(&texture_size),
                    SurfaceInputIndex::TextureSize as usize,
                );
                command_encoder.setFragmentTexture_atIndex(
                    Some(&y_metal_texture),
                    SurfaceInputIndex::YTexture as usize,
                );
                command_encoder.setFragmentTexture_atIndex(
                    Some(&cb_cr_metal_texture),
                    SurfaceInputIndex::CbCrTexture as usize,
                );
                command_encoder.setFragmentBytes_length_atIndex(
                    ptr::NonNull::from(&ycbcr_to_rgb).cast(),
                    mem::size_of_val(&ycbcr_to_rgb),
                    SurfaceInputIndex::YCbCrToRgb as usize,
                );
            }
            surface_textures.extend([y_texture, cb_cr_texture]);

            // SAFETY: Six unit vertices draw one instance within the uploaded scene's surface range.
            unsafe {
                command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                    MTLPrimitiveType::Triangle,
                    0,
                    6,
                    1,
                    first_surface + index,
                );
            }
        }
        Ok(())
    }

    /// Leave a surface out of this frame rather than abort the app over it, and say why once.
    fn skip_surface(&mut self, why: std::fmt::Arguments<'_>) {
        if !self.surface_skip_logged {
            self.surface_skip_logged = true;
            log::error!("skipping a video surface: {why}");
        }
    }
}

/// The Y′CbCr matrix a surface's buffer is tagged with. Untagged is BT.709, the HD default.
fn surface_matrix(buffer: &CVPixelBuffer) -> YCbCrMatrix {
    // SAFETY: the keys and values are CoreVideo's own constant strings, which live forever, and
    // a null attachment mode is allowed (CVBuffer.h).
    let Some(matrix) =
        (unsafe { buffer.attachment(kCVImageBufferYCbCrMatrixKey, ptr::null_mut()) })
            .and_then(|value| value.downcast::<CFString>().ok())
    else {
        return YCbCrMatrix::Bt709;
    };
    // SAFETY: as above.
    let (bt601, bt2020) = unsafe {
        (
            kCVImageBufferYCbCrMatrix_ITU_R_601_4,
            kCVImageBufferYCbCrMatrix_ITU_R_2020,
        )
    };
    if *matrix == *bt601 {
        YCbCrMatrix::Bt601
    } else if *matrix == *bt2020 {
        YCbCrMatrix::Bt2020
    } else {
        YCbCrMatrix::Bt709
    }
}

/// The Y′CbCr → R′G′B′ matrices a surface may be tagged with.
#[derive(Clone, Copy, Debug, PartialEq)]
enum YCbCrMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

/// How a surface's two planes hold their samples, read off its pixel format. Chroma may be
/// subsampled or not: the shader samples both planes at the same normalised coordinates, so
/// 4:2:0, 4:2:2 and 4:4:4 differ only in the plane sizes CoreVideo reports.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SurfaceLayout {
    depth: SampleDepth,
    full_range: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SampleDepth {
    Eight,
    /// Ten significant bits at the top of each 16-bit sample.
    Ten,
}

impl SurfaceLayout {
    #[allow(non_upper_case_globals)]
    fn of(format: u32) -> Option<Self> {
        let (depth, full_range) = match format {
            kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange => (SampleDepth::Eight, false),
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange => (SampleDepth::Eight, true),
            kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange
            | kCVPixelFormatType_422YpCbCr10BiPlanarVideoRange
            | kCVPixelFormatType_444YpCbCr10BiPlanarVideoRange => (SampleDepth::Ten, false),
            kCVPixelFormatType_420YpCbCr10BiPlanarFullRange
            | kCVPixelFormatType_422YpCbCr10BiPlanarFullRange
            | kCVPixelFormatType_444YpCbCr10BiPlanarFullRange => (SampleDepth::Ten, true),
            _ => return None,
        };
        Some(Self { depth, full_range })
    }
}

impl SampleDepth {
    /// The luma and interleaved-chroma texture formats a plane of this depth is viewed as.
    fn plane_formats(self) -> (MTLPixelFormat, MTLPixelFormat) {
        match self {
            Self::Eight => (MTLPixelFormat::R8Unorm, MTLPixelFormat::RG8Unorm),
            Self::Ten => (MTLPixelFormat::R16Unorm, MTLPixelFormat::RG16Unorm),
        }
    }

    fn bits(self) -> i32 {
        match self {
            Self::Eight => 8,
            Self::Ten => 10,
        }
    }

    /// What a unorm texture reads for `code` at this depth.
    fn sampled(self, code: f32) -> f32 {
        match self {
            Self::Eight => code / 255.0,
            Self::Ten => code * 64.0 / 65535.0,
        }
    }
}

/// The `float4x4` (columns, as Metal lays it out) the surface shader multiplies
/// `(y, cb, cr, 1)` by, for `matrix` at the depth and range of `layout`.
fn ycbcr_to_rgb(matrix: YCbCrMatrix, layout: SurfaceLayout) -> [[f32; 4]; 4] {
    let (kr, kb) = match matrix {
        YCbCrMatrix::Bt601 => (0.299, 0.114),
        YCbCrMatrix::Bt709 => (0.2126, 0.0722),
        YCbCrMatrix::Bt2020 => (0.2627, 0.0593),
    };
    let kg = 1.0 - kr - kb;
    let depth = layout.depth;
    // Full range spans every code; video range puts black at 16 and white at 235, chroma
    // between 16 and 240, each scaled by 4 at 10 bits (64, 940, 64 and 960). Chroma is centred
    // on 128 (512) either way.
    let step = 2f32.powi(depth.bits() - 8);
    let max = 2f32.powi(depth.bits()) - 1.0;
    let (y_black, y_span, c_span) = if layout.full_range {
        (0.0, max, max)
    } else {
        (16.0 * step, 219.0 * step, 224.0 * step)
    };
    let y_offset = depth.sampled(y_black);
    let y_scale = 1.0 / depth.sampled(y_span);
    let c_scale = 1.0 / depth.sampled(c_span);
    let c_offset = depth.sampled(128.0 * step);
    let r_cr = 2.0 * (1.0 - kr) * c_scale;
    let g_cb = 2.0 * kb * (1.0 - kb) / kg * c_scale;
    let g_cr = 2.0 * kr * (1.0 - kr) / kg * c_scale;
    let b_cb = 2.0 * (1.0 - kb) * c_scale;
    let y0 = -y_offset * y_scale;
    [
        [y_scale, y_scale, y_scale, 0.0],
        [0.0, -g_cb, b_cb, 0.0],
        [r_cr, -g_cr, 0.0, 0.0],
        [
            y0 - r_cr * c_offset,
            y0 + (g_cb + g_cr) * c_offset,
            y0 - b_cb * c_offset,
            1.0,
        ],
    ]
}

#[path = "fast/binds.rs"]
pub(crate) mod binds;

#[cfg(test)]
#[path = "fast/composition_tests.rs"]
mod composition_tests;

#[cfg(test)]
mod ycbcr_tests {
    use super::{MetalRenderer, SampleDepth, SurfaceLayout, YCbCrMatrix, ycbcr_to_rgb};
    use gpui::{Bounds, ContentMask, DevicePixels, PaintSurface, Scene, point, px, size};
    use image::RgbaImage;
    use objc2_core_foundation::{CFDictionary, CFRetained, CFType};
    use objc2_core_video::{
        CVAttachmentMode, CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeightOfPlane,
        CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
        kCVImageBufferYCbCrMatrixKey, kCVPixelBufferIOSurfacePropertiesKey,
        kCVPixelFormatType_32BGRA, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
        kCVPixelFormatType_444YpCbCr10BiPlanarFullRange,
        kCVPixelFormatType_444YpCbCr10BiPlanarVideoRange, kCVReturnSuccess,
    };
    use objc2_metal::MTLCommandBuffer as _;

    const SIDE: usize = 32;
    const EIGHT_FULL: SurfaceLayout = SurfaceLayout {
        depth: SampleDepth::Eight,
        full_range: true,
    };
    const EIGHT_VIDEO: SurfaceLayout = SurfaceLayout {
        depth: SampleDepth::Eight,
        full_range: false,
    };
    const TEN_FULL: SurfaceLayout = SurfaceLayout {
        depth: SampleDepth::Ten,
        full_range: true,
    };
    const TEN_VIDEO: SurfaceLayout = SurfaceLayout {
        depth: SampleDepth::Ten,
        full_range: false,
    };

    fn rgb(m: [[f32; 4]; 4], y: f32, cb: f32, cr: f32) -> [f32; 3] {
        let v = [y, cb, cr, 1.0];
        let mut out = [0.0; 3];
        for (row, value) in out.iter_mut().enumerate() {
            *value = (0..4).map(|col| m[col][row] * v[col]).sum();
        }
        out
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3)
    }

    /// What an R16Unorm texel reads for a 10-bit code held in the top bits.
    fn ten(code: u16) -> f32 {
        f32::from(code << 6) / 65535.0
    }

    /// An IOSurface-backed buffer, as a decoder hands over, tagged BT.709.
    fn surface(format: u32) -> CFRetained<CVPixelBuffer> {
        // SAFETY: CoreVideo's own constant strings, which live forever.
        let (surface_key, matrix_key, bt709) = unsafe {
            (
                kCVPixelBufferIOSurfacePropertiesKey,
                kCVImageBufferYCbCrMatrixKey,
                kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            )
        };
        let options =
            CFDictionary::from_slices(&[surface_key], &[&*CFDictionary::<CFType, CFType>::empty()]);
        let mut buffer = std::ptr::null_mut();
        // SAFETY: a valid out-pointer, which CoreVideo fills with a +1 buffer on success.
        let status = unsafe {
            CVPixelBufferCreate(
                None,
                SIDE,
                SIDE,
                format,
                Some(options.as_opaque()),
                std::ptr::NonNull::from(&mut buffer),
            )
        };
        assert_eq!(status, kCVReturnSuccess);
        // SAFETY: the buffer CoreVideo created, owned from here.
        let buffer = unsafe { CFRetained::from_raw(std::ptr::NonNull::new(buffer).unwrap()) };
        // SAFETY: a CFString key and value, as the attachment takes.
        unsafe { buffer.set_attachment(matrix_key, bt709, CVAttachmentMode::ShouldPropagate) };
        buffer
    }

    /// Locks `buffer`'s base address for `write`, then unlocks it.
    fn locked(buffer: &CVPixelBuffer, write: impl FnOnce()) {
        // SAFETY: a live buffer, locked for writing and unlocked with the same flags.
        unsafe {
            assert_eq!(
                CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
            write();
            assert_eq!(
                CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
        }
    }

    /// Writes `texel(x)` to every texel of each row of `plane`, as samples of type `T`.
    fn fill<T: Copy>(buffer: &CVPixelBuffer, plane: usize, texel: impl Fn(usize) -> Vec<T>) {
        let base = CVPixelBufferGetBaseAddressOfPlane(buffer, plane);
        let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, plane);
        let row_len = stride / size_of::<T>();
        // SAFETY: the base address is locked, and a plane spans `bytes per row × height`.
        let samples = unsafe {
            std::slice::from_raw_parts_mut(
                base.cast::<T>(),
                row_len * CVPixelBufferGetHeightOfPlane(buffer, plane),
            )
        };
        for row in samples.chunks_exact_mut(row_len) {
            let mut at = 0;
            for x in 0..CVPixelBufferGetWidthOfPlane(buffer, plane) {
                let value = texel(x);
                row[at..at + value.len()].copy_from_slice(&value);
                at += value.len();
            }
        }
    }

    fn render(renderer: &mut MetalRenderer, image_buffer: CFRetained<CVPixelBuffer>) -> RgbaImage {
        let bounds = Bounds::new(
            point(px(0.), px(0.)),
            size(px(SIDE as f32), px(SIDE as f32)),
        )
        .scale(1.0);
        let mut scene = Scene::default();
        scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            image_buffer,
        });
        scene.finish();
        let target = size(DevicePixels(SIDE as i32), DevicePixels(SIDE as i32));
        renderer.render_scene_to_image(&scene, target).unwrap()
    }

    /// Black, white and the primaries come out where BT.709 puts them, at both ranges; the
    /// BT.601 matrix is the one the shader hard-coded before.
    #[test]
    fn the_matrices_map_codes_to_the_right_colours() {
        let c = 128.0 / 255.0;
        let full = ycbcr_to_rgb(YCbCrMatrix::Bt709, EIGHT_FULL);
        assert!(close(rgb(full, 0.0, c, c), [0.0; 3]));
        assert!(close(rgb(full, 1.0, c, c), [1.0; 3]));
        // Pure red in BT.709: Y = Kr, Cb = −Kr / (2 (1 − Kb)), Cr = 1/2.
        let red = rgb(full, 0.2126, c - 0.2126 / (2.0 * (1.0 - 0.0722)), c + 0.5);
        assert!(close(red, [1.0, 0.0, 0.0]), "{red:?}");

        let video = ycbcr_to_rgb(YCbCrMatrix::Bt709, EIGHT_VIDEO);
        assert!(close(rgb(video, 16.0 / 255.0, c, c), [0.0; 3]));
        assert!(close(rgb(video, 235.0 / 255.0, c, c), [1.0; 3]));

        let bt601 = ycbcr_to_rgb(YCbCrMatrix::Bt601, EIGHT_FULL);
        let old = [
            [1.0, 1.0, 1.0, 0.0],
            [0.0, -0.3441, 1.7720, 0.0],
            [1.4020, -0.7141, 0.0, 0.0],
        ];
        for (column, want) in bt601.iter().zip(old) {
            assert!(
                column.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3),
                "{bt601:?}"
            );
        }
        assert_ne!(ycbcr_to_rgb(YCbCrMatrix::Bt2020, EIGHT_FULL), full);
    }

    /// 10-bit video range puts black at code 64, white at 940 and chroma's extremes at 64 and
    /// 960; full range spans 0 to 1023. The codes sit in the top of 16-bit samples, so they
    /// read differently from the 8-bit ones and need their own offsets.
    #[test]
    fn ten_bit_codes_map_to_the_right_colours() {
        let video = ycbcr_to_rgb(YCbCrMatrix::Bt709, TEN_VIDEO);
        let c = ten(512);
        assert!(close(rgb(video, ten(64), c, c), [0.0; 3]));
        assert!(close(rgb(video, ten(940), c, c), [1.0; 3]));
        let red = rgb(video, ten(250), ten(409), ten(960));
        assert!(close(red, [1.0, 0.0, 0.0]), "{red:?}");

        let full = ycbcr_to_rgb(YCbCrMatrix::Bt709, TEN_FULL);
        assert!(close(rgb(full, 0.0, c, c), [0.0; 3]));
        assert!(close(rgb(full, ten(1023), c, c), [1.0; 3]));
        assert_ne!(video, ycbcr_to_rgb(YCbCrMatrix::Bt709, EIGHT_VIDEO));
    }

    #[test]
    fn a_pixel_format_picks_its_depth_and_range() {
        for (format, layout) in [
            (kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, EIGHT_VIDEO),
            (kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, EIGHT_FULL),
            (kCVPixelFormatType_444YpCbCr10BiPlanarVideoRange, TEN_VIDEO),
            (kCVPixelFormatType_444YpCbCr10BiPlanarFullRange, TEN_FULL),
        ] {
            assert_eq!(SurfaceLayout::of(format), Some(layout), "{format:#x}");
        }
        assert_eq!(SurfaceLayout::of(kCVPixelFormatType_32BGRA), None);
    }

    /// A full-range BT.709 red surface renders red through the real pipeline, frame after
    /// frame, with its textures held to each command buffer's completion; a surface in a
    /// format the shader cannot sample is left out instead of aborting the renderer.
    #[test]
    fn a_tagged_surface_renders_its_colour_and_a_foreign_one_is_skipped() {
        let red = surface(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
        // BT.709 full-range red: Y = 0.2126, Cb = 0.5 − 0.2126 / 1.8556, Cr = 1.
        locked(&red, || {
            fill::<u8>(&red, 0, |_| vec![54]);
            fill::<u8>(&red, 1, |_| vec![99, 255]);
        });

        let mut renderer = MetalRenderer::new_headless(Default::default()).unwrap();
        for _ in 0..3 {
            let [r, g, b, _] = render(&mut renderer, red.clone()).get_pixel(16, 16).0;
            assert!(
                r >= 250 && g <= 5 && b <= 5,
                "BT.709 red came out {r} {g} {b}"
            );
        }

        let image = render(&mut renderer, surface(kCVPixelFormatType_32BGRA));
        assert_eq!(
            image.get_pixel(16, 16).0[..3],
            [0, 0, 0],
            "skipped, the clear colour shows"
        );
        assert!(renderer.surface_skip_logged);
    }

    /// A frame's surface textures outlive its completion handler and go on the render thread,
    /// at the next frame, never on Metal's: releasing one touches its texture cache, which the
    /// render thread flushes, and which a renderer dropped mid-frame takes with it.
    #[test]
    fn surface_textures_go_on_the_render_thread_once_their_frame_is_done() {
        let red = surface(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
        let bounds = Bounds::new(
            point(px(0.), px(0.)),
            size(px(SIDE as f32), px(SIDE as f32)),
        )
        .scale(1.0);
        let target = size(DevicePixels(SIDE as i32), DevicePixels(SIDE as i32));
        let mut scene = Scene::default();
        scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            image_buffer: red,
        });
        scene.finish();
        let mut renderer = MetalRenderer::new_headless(Default::default()).unwrap();
        let texture = crate::fast::render_target(&renderer.device, SIDE, SIDE);
        renderer.update_path_intermediate_textures(target).unwrap();
        let frame = |renderer: &mut MetalRenderer| {
            let frame = renderer.render_frame(&scene, &texture, target).unwrap();
            frame.commit();
            frame
        };
        // All three are encoded before any is committed: a frame done before the next one is
        // encoded would rightly go at that one, and the count would race the GPU.
        let frames = (0..3)
            .map(|_| renderer.render_frame(&scene, &texture, target).unwrap())
            .collect::<Vec<_>>();
        for frame in &frames {
            frame.commit();
        }
        for frame in frames {
            frame.waitUntilCompleted();
        }
        assert_eq!(
            renderer.surfaces_in_flight.held(),
            3,
            "none released by Metal's handler"
        );
        let last = frame(&mut renderer);
        assert_eq!(
            renderer.surfaces_in_flight.held(),
            1,
            "the done ones go at the next frame"
        );
        drop(renderer);
        last.waitUntilCompleted();
    }

    /// A 10-bit 4:4:4 surface, at video range and at full, renders columns that alternate red
    /// and blue each keeping its colour: chroma is sampled at full resolution, and the 16-bit
    /// planes are read with the 10-bit offsets.
    #[test]
    fn a_ten_bit_444_surface_keeps_every_column_its_colour() {
        let mut renderer = MetalRenderer::new_headless(Default::default()).unwrap();
        // BT.709 red and blue as 10-bit (Y, Cb, Cr) codes.
        for (format, red, blue) in [
            (
                kCVPixelFormatType_444YpCbCr10BiPlanarVideoRange,
                [250, 409, 960],
                [127, 960, 471],
            ),
            (
                kCVPixelFormatType_444YpCbCr10BiPlanarFullRange,
                [217, 395, 1023],
                [74, 1023, 465],
            ),
        ] {
            let buffer = surface(format);
            assert_eq!(
                CVPixelBufferGetWidthOfPlane(&buffer, 1),
                SIDE,
                "4:4:4 chroma is full width"
            );
            let code = |x: usize| if x.is_multiple_of(2) { red } else { blue };
            locked(&buffer, || {
                fill::<u16>(&buffer, 0, |x| vec![code(x)[0] << 6]);
                fill::<u16>(&buffer, 1, |x| vec![code(x)[1] << 6, code(x)[2] << 6]);
            });

            let image = render(&mut renderer, buffer);
            for x in 0..SIDE as u32 {
                let [r, g, b, _] = image.get_pixel(x, 16).0;
                let want_red = x.is_multiple_of(2);
                let (hi, lo) = if want_red { (r, b) } else { (b, r) };
                assert!(
                    hi >= 250 && g <= 5 && lo <= 5,
                    "{format:#x} column {x} came out {r} {g} {b}"
                );
            }
        }
        assert!(!renderer.surface_skip_logged);
    }
}

pub(crate) fn new_command_encoder_for_texture(
    command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    texture: &ProtocolObject<dyn MTLTexture>,
    stencil: &ProtocolObject<dyn MTLTexture>,
    viewport_size: Size<DevicePixels>,
    clear_color: Option<MTLClearColor>,
) -> Result<Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>> {
    let render_pass_descriptor = MTLRenderPassDescriptor::new();
    crate::fast::occlusion::attach_stencil(&render_pass_descriptor, stencil);
    // SAFETY: Index zero is within the render pass's color attachment array.
    let color_attachment = unsafe {
        render_pass_descriptor
            .colorAttachments()
            .objectAtIndexedSubscript(0)
    };
    color_attachment.setTexture(Some(texture));
    color_attachment.setStoreAction(MTLStoreAction::Store);
    if let Some(clear_color) = clear_color {
        color_attachment.setLoadAction(MTLLoadAction::Clear);
        color_attachment.setClearColor(clear_color);
    } else {
        color_attachment.setLoadAction(MTLLoadAction::Load);
    }

    let command_encoder = command_buffer
        .renderCommandEncoderWithDescriptor(&render_pass_descriptor)
        .context("creating render command encoder")?;
    command_encoder.setViewport(MTLViewport {
        originX: 0.0,
        originY: 0.0,
        width: i32::from(viewport_size.width) as f64,
        height: i32::from(viewport_size.height) as f64,
        znear: 0.0,
        zfar: 1.0,
    });
    Ok(command_encoder)
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
fn read_texture_to_image(texture: &ProtocolObject<dyn MTLTexture>) -> Result<RgbaImage> {
    let width = texture.width() as u32;
    let height = texture.height() as u32;
    let bytes_per_row = width as usize * 4;
    let mut pixels = vec![0u8; height as usize * bytes_per_row];

    let region = MTLRegion {
        origin: MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize {
            width: width as usize,
            height: height as usize,
            depth: 1,
        },
    };
    unsafe {
        texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
            ptr::NonNull::from(pixels.as_mut_slice()).cast(),
            bytes_per_row,
            region,
            0,
        );
    }

    // Convert BGRA to RGBA (swap B and R channels)
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }

    RgbaImage::from_raw(width, height, pixels).context("creating RGBA image from pixel data")
}

fn build_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: MTLPixelFormat,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str(vertex_fn_name))
        .with_context(|| format!("locating vertex function {vertex_fn_name}"))?;
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str(fragment_fn_name))
        .with_context(|| format!("locating fragment function {fragment_fn_name}"))?;

    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(label)));
    descriptor.setVertexFunction(Some(vertex_fn.as_ref()));
    descriptor.setFragmentFunction(Some(fragment_fn.as_ref()));
    // SAFETY: Index zero is within the pipeline's color attachment array.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(pixel_format);
    color_attachment.setBlendingEnabled(true);
    color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
    color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
    color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
    color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
    color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);

    descriptor.setStencilAttachmentPixelFormat(crate::fast::occlusion::STENCIL_FORMAT);
    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .with_context(|| format!("creating {label} render pipeline"))
}

/// The quad shader drawing into `destination × (1 − source alpha)`, in colour and alpha:
/// a hole whose source alpha is its coverage times the native's opacity.
fn build_hole_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    pixel_format: MTLPixelFormat,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str("quad_vertex"))
        .context("locating vertex function quad_vertex")?;
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str("quad_fragment"))
        .context("locating fragment function quad_fragment")?;

    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str("holes")));
    descriptor.setVertexFunction(Some(vertex_fn.as_ref()));
    descriptor.setFragmentFunction(Some(fragment_fn.as_ref()));
    // SAFETY: Index zero is within the pipeline's color attachment array.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(pixel_format);
    color_attachment.setBlendingEnabled(true);
    color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
    color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
    color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::Zero);
    color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::Zero);
    color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);

    descriptor.setStencilAttachmentPixelFormat(crate::fast::occlusion::STENCIL_FORMAT);
    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .context("creating holes render pipeline")
}

fn build_path_sprite_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: MTLPixelFormat,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str(vertex_fn_name))
        .with_context(|| format!("locating vertex function {vertex_fn_name}"))?;
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str(fragment_fn_name))
        .with_context(|| format!("locating fragment function {fragment_fn_name}"))?;

    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(label)));
    descriptor.setVertexFunction(Some(vertex_fn.as_ref()));
    descriptor.setFragmentFunction(Some(fragment_fn.as_ref()));
    // SAFETY: Index zero is within the pipeline's color attachment array.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(pixel_format);
    color_attachment.setBlendingEnabled(true);
    color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
    color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
    color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
    color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
    color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);

    descriptor.setStencilAttachmentPixelFormat(crate::fast::occlusion::STENCIL_FORMAT);
    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .with_context(|| format!("creating {label} render pipeline"))
}

fn build_path_rasterization_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: MTLPixelFormat,
    path_sample_count: u32,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str(vertex_fn_name))
        .with_context(|| format!("locating vertex function {vertex_fn_name}"))?;
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str(fragment_fn_name))
        .with_context(|| format!("locating fragment function {fragment_fn_name}"))?;

    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(label)));
    descriptor.setVertexFunction(Some(vertex_fn.as_ref()));
    descriptor.setFragmentFunction(Some(fragment_fn.as_ref()));
    if path_sample_count > 1 {
        descriptor.setRasterSampleCount(path_sample_count as _);
        descriptor.setAlphaToCoverageEnabled(false);
    }
    // SAFETY: Index zero is within the pipeline's color attachment array.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(pixel_format);
    color_attachment.setBlendingEnabled(true);
    color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
    color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
    color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
    color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
    color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);

    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .with_context(|| format!("creating {label} render pipeline"))
}

#[derive(Clone)]
pub(crate) struct InstanceBinding {
    pub(crate) buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    pub(crate) offset: usize,
}

pub(crate) struct InstanceBindings {
    pub(crate) quads: InstanceBinding,
    pub(crate) holes: InstanceBinding,
    pub(crate) shadows: InstanceBinding,
    pub(crate) underlines: InstanceBinding,
    pub(crate) monochrome_sprites: InstanceBinding,
    pub(crate) polychrome_sprites: InstanceBinding,
    pub(crate) surfaces: InstanceBinding,
    pub(crate) fades: InstanceBinding,
    pub(crate) fast_blurs: InstanceBinding,
}

pub(crate) fn write_instances(
    scene: &Scene,
    writer: &mut InstanceBufferWriter,
) -> Result<InstanceBindings> {
    Ok(InstanceBindings {
        quads: writer.write(&scene.quads)?,
        holes: writer.write(&scene.natives().holes)?,
        shadows: writer.write(&scene.shadows)?,
        underlines: writer.write(&scene.underlines)?,
        monochrome_sprites: writer.write(&scene.monochrome_sprites)?,
        polychrome_sprites: writer.write(&scene.polychrome_sprites)?,
        surfaces: writer.write_iter(scene.surfaces.iter().map(|surface| SurfaceBounds {
            bounds: surface.bounds,
            content_mask: surface.content_mask,
        }))?,
        fades: crate::fast::edge_fade::write(scene, writer)?,
        fast_blurs: crate::fast::backdrop::write(scene, writer)?,
    })
}

pub(crate) struct InstanceBufferWriter {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    pool: Arc<Mutex<InstanceBufferPool>>,
    unified_memory: bool,
    filled: Vec<(InstanceBuffer, usize)>,
    current: InstanceBuffer,
    offset: usize,
}

impl InstanceBufferWriter {
    pub(crate) fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        pool: &Arc<Mutex<InstanceBufferPool>>,
        unified_memory: bool,
    ) -> Result<Self> {
        let current = pool.lock().acquire(device, unified_memory)?;
        Ok(Self {
            device: device.retain(),
            pool: pool.clone(),
            unified_memory,
            filled: Vec::new(),
            current,
            offset: 0,
        })
    }

    fn allocate<T>(&mut self, count: usize) -> Result<(InstanceBinding, &mut [MaybeUninit<T>])> {
        let size = mem::size_of::<T>() * count;
        let mut offset = self.offset.next_multiple_of(INSTANCE_BUFFER_ALIGNMENT);
        if offset + size > self.current.size {
            self.grow(size)?;
            offset = 0;
        }
        self.offset = offset + size;

        let binding = InstanceBinding {
            buffer: self.current.metal_buffer.clone(),
            offset,
        };
        // SAFETY: The aligned reservation lies within this frame's exclusively owned buffer and does not overlap prior writes.
        let values = unsafe {
            let start = self
                .current
                .metal_buffer
                .contents()
                .as_ptr()
                .cast::<u8>()
                .add(offset);
            slice::from_raw_parts_mut(start.cast::<MaybeUninit<T>>(), count)
        };
        Ok((binding, values))
    }

    pub(crate) fn write<T>(&mut self, values: &[T]) -> Result<InstanceBinding> {
        let (binding, destination) = self.allocate::<T>(values.len())?;
        // SAFETY: The disjoint source and destination hold the same number of T values, and the destination stays MaybeUninit.
        unsafe {
            ptr::copy_nonoverlapping(
                values.as_ptr(),
                destination.as_mut_ptr().cast::<T>(),
                values.len(),
            );
        }
        Ok(binding)
    }

    pub(crate) fn write_iter<T>(
        &mut self,
        values: impl ExactSizeIterator<Item = T>,
    ) -> Result<InstanceBinding> {
        let (binding, destination) = self.allocate::<T>(values.len())?;
        for (slot, value) in destination.iter_mut().zip(values) {
            slot.write(value);
        }
        Ok(binding)
    }

    fn grow(&mut self, required: usize) -> Result<()> {
        let mut pool = self.pool.lock();
        let buffer_size = (pool.buffer_size * 2)
            .max(required.next_power_of_two())
            .min(MAX_INSTANCE_BUFFER_SIZE);
        anyhow::ensure!(
            buffer_size >= required,
            "instance buffer needs {required} bytes, above the maximum of {MAX_INSTANCE_BUFFER_SIZE}"
        );
        anyhow::ensure!(
            buffer_size > self.current.size,
            "frame instance data exceeds the {MAX_INSTANCE_BUFFER_SIZE}-byte maximum"
        );
        if buffer_size != pool.buffer_size {
            log::info!("increased instance buffer size to {buffer_size}");
            pool.reset(buffer_size);
        }
        let buffer = pool.acquire(&self.device, self.unified_memory)?;
        drop(pool);

        let filled = mem::replace(&mut self.current, buffer);
        self.filled.push((filled, self.offset));
        self.offset = 0;
        Ok(())
    }

    pub(crate) fn finish(self) -> InstanceBuffer {
        let Self {
            unified_memory,
            filled,
            current,
            offset,
            ..
        } = self;

        if !unified_memory {
            for (buffer, written) in &filled {
                if *written == 0 {
                    continue;
                }
                buffer.metal_buffer.didModifyRange(NSRange {
                    location: 0,
                    length: *written,
                });
            }
            if offset > 0 {
                current.metal_buffer.didModifyRange(NSRange {
                    location: 0,
                    length: offset,
                });
            }
        }

        // Metal retains encoded resources until the command buffer completes.
        // Only the final, largest buffer is worth keeping in the pool.
        drop(filled);
        current
    }
}

#[repr(C)]
enum ShadowInputIndex {
    Vertices = 0,
    Shadows = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum QuadInputIndex {
    Vertices = 0,
    Quads = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum UnderlineInputIndex {
    Vertices = 0,
    Underlines = 1,
    ViewportSize = 2,
}

#[repr(C)]
pub(crate) enum SpriteInputIndex {
    Vertices = 0,
    Sprites = 1,
    ViewportSize = 2,
    AtlasTextureSize = 3,
    AtlasTexture = 4,
}

#[repr(C)]
#[doc(hidden)]
pub enum SurfaceInputIndex {
    Vertices = 0,
    Surfaces = 1,
    ViewportSize = 2,
    TextureSize = 3,
    YTexture = 4,
    CbCrTexture = 5,
    YCbCrToRgb = 6,
}

#[repr(C)]
pub(crate) enum PathRasterizationInputIndex {
    Vertices = 0,
    ViewportSize = 1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PathSprite {
    pub bounds: Bounds<ScaledPixels>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct SurfaceBounds {
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
pub struct MetalHeadlessRenderer {
    renderer: MetalRenderer,
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
impl MetalHeadlessRenderer {
    pub fn new() -> Result<Self> {
        let instance_buffer_pool = Arc::new(Mutex::new(InstanceBufferPool::default()));
        let renderer = MetalRenderer::new_headless(instance_buffer_pool)?;
        Ok(Self { renderer })
    }
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
impl gpui::PlatformHeadlessRenderer for MetalHeadlessRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<image::RgbaImage> {
        self.renderer.render_scene_to_image(scene, size)
    }

    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> anyhow::Result<()> {
        self.renderer.render_scene(scene, size)
    }

    fn sprite_atlas(&self) -> Arc<dyn gpui::PlatformAtlas> {
        self.renderer.sprite_atlas().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::{CFDictionary, CFType};
    use objc2_core_video::{
        CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferIOSurfacePropertiesKey,
    };
    use objc2_metal::MTLCommandBufferStatus;

    #[test]
    fn render_core_video_surface() {
        let surface_properties_key = unsafe { kCVPixelBufferIOSurfacePropertiesKey };
        let attributes = CFDictionary::from_slices(
            &[surface_properties_key],
            &[&*CFDictionary::<CFType, CFType>::empty()],
        );
        let mut image_buffer = ptr::null_mut();
        assert_eq!(
            unsafe {
                CVPixelBufferCreate(
                    None,
                    16,
                    16,
                    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                    Some(attributes.as_opaque()),
                    ptr::NonNull::from(&mut image_buffer),
                )
            },
            kCVReturnSuccess
        );
        let image_buffer = ptr::NonNull::new(image_buffer).unwrap();
        let image_buffer = unsafe { CFRetained::from_raw(image_buffer) };
        unsafe {
            assert_eq!(
                CVPixelBufferLockBaseAddress(&image_buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
            for plane in 0..2 {
                let bytes = CVPixelBufferGetBaseAddressOfPlane(&image_buffer, plane).cast::<u8>();
                let length = CVPixelBufferGetBytesPerRowOfPlane(&image_buffer, plane)
                    * CVPixelBufferGetHeightOfPlane(&image_buffer, plane);
                slice::from_raw_parts_mut(bytes, length).fill(128);
            }
            assert_eq!(
                CVPixelBufferUnlockBaseAddress(&image_buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
        }

        let bounds = Bounds {
            origin: Point::default(),
            size: size(ScaledPixels(16.0), ScaledPixels(16.0)),
        };
        let mut scene = Scene::default();
        scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            image_buffer: image_buffer.clone(),
        });
        drop(image_buffer);
        scene.finish();

        let mut renderer =
            MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())))
                .unwrap();
        let image = renderer
            .render_scene_to_image(&scene, size(DevicePixels(16), DevicePixels(16)))
            .unwrap();
        for image::Rgba([red, green, blue, alpha]) in image.pixels() {
            assert!(red.abs_diff(128) <= 2);
            assert!(green.abs_diff(128) <= 2);
            assert!(blue.abs_diff(128) <= 2);
            assert_eq!(*alpha, 255);
        }

        let descriptor = MTLTextureDescriptor::new();
        unsafe {
            descriptor.setWidth(16);
            descriptor.setHeight(16);
        }
        descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        descriptor.setUsage(MTLTextureUsage::RenderTarget);
        let uses_shared_storage = cfg!(target_os = "ios") || renderer.is_apple_gpu;
        descriptor.setStorageMode(if uses_shared_storage {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        });
        let target_texture = renderer
            .device
            .newTextureWithDescriptor(&descriptor)
            .unwrap();
        let command_buffer = renderer
            .render_frame(
                &scene,
                &target_texture,
                size(DevicePixels(16), DevicePixels(16)),
            )
            .unwrap();
        if !uses_shared_storage {
            let blit = command_buffer.blitCommandEncoder().unwrap();
            blit.synchronizeResource(ProtocolObject::from_ref(&*target_texture));
            blit.endEncoding();
        }
        drop(scene);
        drop(renderer);
        command_buffer.commit();
        command_buffer.waitUntilCompleted();
        assert_eq!(command_buffer.status(), MTLCommandBufferStatus::Completed);
        assert_eq!(read_texture_to_image(&target_texture).unwrap(), image);
    }
}
