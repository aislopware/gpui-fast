//! Pixel tests of scroll layers: the same content drawn directly and through
//! layer tiles must come out byte for byte the same.
//!
//! [`Harness`] draws scenes without a window: it owns the GPU objects the
//! renderer would, built by the renderer's own constructors, and records
//! through `fast::frame` as the renderer does, into a texture it reads back.

use std::num::NonZeroU64;
use std::sync::Arc;

use anyhow::Result;
use gpui::{
    Bounds, ContentMask, DevicePixels, Hsla, Quad, Rgba, ScaledPixels, Scene, Size, point, rgba,
    size,
};

use crate::WgpuAtlas;
use crate::fast::frame::{FrameHost, FrameState, FrameTarget};
use crate::wgpu_renderer::{
    GammaParams, GlobalParams, RenderingParameters, WgpuBindGroupLayouts, WgpuPipelines,
    WgpuRenderer,
};

#[test]
fn harness_renders_a_quad_at_the_right_pixels() {
    let Some(mut harness) = Harness::new() else {
        eprintln!("skipped: no wgpu adapter");
        return;
    };
    let mut scene = Scene::default();
    scene.insert_primitive(quad(sp(4., 4., 8., 8.), Hsla::from(rgba(0xff0000ff))));
    scene.finish();
    let pixels = harness.render(&scene, device_size(32, 32), rgba(0xffffffff));
    assert_eq!(pixel(&pixels, 32, 8, 8), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 32, 1, 1), [255, 255, 255, 255]);
}

// --- helpers --- //

pub(super) fn sp(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(w), ScaledPixels(h)),
    }
}

/// A content mask that clips nothing the tests draw.
pub(super) fn no_mask() -> ContentMask<ScaledPixels> {
    ContentMask {
        bounds: sp(-10_000., -10_000., 20_000., 20_000.),
    }
}

pub(super) fn quad(bounds: Bounds<ScaledPixels>, color: Hsla) -> Quad {
    Quad {
        bounds,
        content_mask: no_mask(),
        background: color.into(),
        ..Default::default()
    }
}

pub(super) fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    size(DevicePixels(width), DevicePixels(height))
}

/// The RGBA bytes of the pixel at (`x`, `y`) of a `width`-wide readback.
pub(super) fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let at = (y * width + x) * 4;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

const INITIAL_INSTANCE_CAPACITY: u64 = 2 * 1024 * 1024;

/// Draws scenes into textures without a surface, through the renderer's
/// pipelines and frame recording.
pub(super) struct Harness {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    format: wgpu::TextureFormat,
    atlas: WgpuAtlas,
    pipelines: WgpuPipelines,
    bind_group_layouts: WgpuBindGroupLayouts,
    atlas_sampler: wgpu::Sampler,
    rendering_params: RenderingParameters,
    globals_buffer: wgpu::Buffer,
    path_globals_offset: u64,
    gamma_offset: u64,
    globals_bind_group: wgpu::BindGroup,
    path_globals_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    instance_alignment: u64,
    path_targets: Option<PathTargets>,
    state: FrameState,
}

/// The path intermediate texture, and its multisampled twin, of one size.
struct PathTargets {
    size: Size<DevicePixels>,
    _intermediate: wgpu::Texture,
    intermediate_view: wgpu::TextureView,
    _msaa: Option<wgpu::Texture>,
    msaa_view: Option<wgpu::TextureView>,
}

impl Harness {
    /// A harness on the first adapter wgpu offers without a surface, or
    /// `None` when there is none (the tests then skip).
    pub(super) fn new() -> Option<Harness> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            flags: wgpu::InstanceFlags::default(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });
        let adapter = gpui::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok()?;
        let (device, queue, dual_source_blending, color_texture_format) =
            gpui::block_on(crate::WgpuContext::create_device(&adapter)).ok()?;
        let (device, queue) = (Arc::new(device), Arc::new(queue));

        // The surface's choice when it offers both.
        let usages = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let format = [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
        ]
        .into_iter()
        .find(|format| {
            adapter
                .get_texture_format_features(*format)
                .allowed_usages
                .contains(usages)
        })?;

        let rendering_params = RenderingParameters::new(&adapter, format);
        let bind_group_layouts = WgpuRenderer::create_bind_group_layouts(&device, false);
        let pipelines = WgpuRenderer::create_pipelines(
            &device,
            &bind_group_layouts,
            format,
            wgpu::CompositeAlphaMode::Opaque,
            rendering_params.path_sample_count,
            dual_source_blending,
            false,
        );
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let globals_size = size_of::<GlobalParams>() as u64;
        let gamma_size = size_of::<GammaParams>() as u64;
        let path_globals_offset = globals_size.next_multiple_of(uniform_alignment);
        let gamma_offset = (path_globals_offset + globals_size).next_multiple_of(uniform_alignment);
        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals_buffer"),
            size: gamma_offset + gamma_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group = |label: &str, offset: u64| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &bind_group_layouts.globals,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &globals_buffer,
                            offset,
                            size: NonZeroU64::new(globals_size),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &globals_buffer,
                            offset: gamma_offset,
                            size: NonZeroU64::new(gamma_size),
                        }),
                    },
                ],
            })
        };
        let path_globals_bind_group =
            globals_bind_group("path_globals_bind_group", path_globals_offset);
        let globals_bind_group = globals_bind_group("globals_bind_group", 0);

        let instance_alignment = device.limits().min_storage_buffer_offset_alignment as u64;
        let instance_buffer = create_instance_buffer(&device, INITIAL_INSTANCE_CAPACITY);
        let atlas = WgpuAtlas::new(device.clone(), queue.clone(), color_texture_format);

        Some(Harness {
            device,
            queue,
            format,
            atlas,
            pipelines,
            bind_group_layouts,
            atlas_sampler,
            rendering_params,
            globals_buffer,
            path_globals_offset,
            gamma_offset,
            globals_bind_group,
            path_globals_bind_group,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            instance_alignment,
            path_targets: None,
            state: FrameState::default(),
        })
    }

    /// Draws `scene` into a `size` texture cleared to `clear`, as the
    /// renderer draws a frame, and returns its pixels as RGBA bytes, row by row.
    pub(super) fn render(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
        clear: Rgba,
    ) -> Vec<u8> {
        let (width, height) = (size.width.0 as u32, size.height.0 as u32);
        self.write_globals(width, height);
        self.ensure_path_targets(size);
        self.atlas.before_frame();

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("harness_target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let clear = wgpu::Color {
            r: clear.r as f64,
            g: clear.g as f64,
            b: clear.b as f64,
            a: clear.a as f64,
        };
        crate::fast::frame::record_into(self, scene, &view, clear).expect("frame recorded");
        read_back(&self.device, &self.queue, &texture, self.format)
    }

    fn write_globals(&self, width: u32, height: u32) {
        let globals = GlobalParams {
            viewport_size: [width as f32, height as f32],
            premultiplied_alpha: 0,
            pad: 0,
        };
        let gamma = GammaParams {
            gamma_ratios: self.rendering_params.gamma_ratios,
            grayscale_enhanced_contrast: self.rendering_params.grayscale_enhanced_contrast,
            subpixel_enhanced_contrast: self.rendering_params.subpixel_enhanced_contrast,
            is_bgr: 0,
            _pad: 0,
        };
        self.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));
        self.queue.write_buffer(
            &self.globals_buffer,
            self.path_globals_offset,
            bytemuck::bytes_of(&globals),
        );
        self.queue.write_buffer(
            &self.globals_buffer,
            self.gamma_offset,
            bytemuck::bytes_of(&gamma),
        );
    }

    fn ensure_path_targets(&mut self, size: Size<DevicePixels>) {
        if self
            .path_targets
            .as_ref()
            .is_some_and(|targets| targets.size == size)
        {
            return;
        }
        let (width, height) = (size.width.0 as u32, size.height.0 as u32);
        let (intermediate, intermediate_view) =
            WgpuRenderer::create_path_intermediate(&self.device, self.format, width, height);
        let (msaa, msaa_view) = WgpuRenderer::create_msaa_if_needed(
            &self.device,
            self.format,
            width,
            height,
            self.rendering_params.path_sample_count,
        )
        .unzip();
        self.path_targets = Some(PathTargets {
            size,
            _intermediate: intermediate,
            intermediate_view,
            _msaa: msaa,
            msaa_view,
        });
    }
}

impl FrameHost for Harness {
    fn frame_state(&mut self) -> &mut FrameState {
        &mut self.state
    }

    fn instance_data_alignment(&self) -> u64 {
        self.instance_alignment.max(1)
    }

    fn reserve_instance_data(&mut self, size: u64) -> Result<()> {
        if size > self.instance_capacity {
            self.instance_capacity = size.next_power_of_two();
            self.instance_buffer = create_instance_buffer(&self.device, self.instance_capacity);
        }
        Ok(())
    }

    fn target(&self) -> Result<FrameTarget<'_>> {
        let path_targets = self.path_targets.as_ref();
        Ok(FrameTarget {
            device: &self.device,
            queue: &self.queue,
            pipelines: &self.pipelines,
            bind_group_layouts: &self.bind_group_layouts,
            atlas: &self.atlas,
            atlas_sampler: &self.atlas_sampler,
            globals_bind_group: &self.globals_bind_group,
            path_globals_bind_group: &self.path_globals_bind_group,
            path_intermediate_view: path_targets.map(|targets| &targets.intermediate_view),
            path_msaa_view: path_targets.and_then(|targets| targets.msaa_view.as_ref()),
            instance_buffer: &self.instance_buffer,
        })
    }
}

fn create_instance_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instance_buffer"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The pixels of `texture` as RGBA bytes, row by row.
pub(super) fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let row = width * 4;
    let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("harness_readback"),
        size: (padded_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("harness_readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("readback mapped")
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device polled");
    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((row * height) as usize);
    for y in 0..height {
        let start = (y * padded_row) as usize;
        pixels.extend_from_slice(&mapped[start..start + row as usize]);
    }
    drop(mapped);
    buffer.unmap();
    if format == wgpu::TextureFormat::Bgra8Unorm {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
    pixels
}
