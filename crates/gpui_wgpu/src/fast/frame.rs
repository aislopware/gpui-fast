//! A frame recorded with one instance upload, reused bind groups and the
//! paths of batches that don't overlap rasterized in one pass.
//!
//! Upstream records a frame like this:
//!
//! - six `Queue::write_buffer` calls, one per primitive kind, each with a bind
//!   group created for it;
//! - per sprite batch, a new texture bind group for its atlas texture;
//! - per batch, `set_pipeline` and every bind group again;
//! - per path batch, the main render pass ends, a pass rasterizes the batch's
//!   paths into the intermediate texture, a new main pass begins, and the
//!   batch's vertices and sprites get a `write_buffer` and a bind group each.
//!
//! On native backends every `write_buffer` allocates a staging buffer, and
//! wgpu-core encodes every render pass into a command buffer of its own, which
//! the driver has to begin, submit and later reset: those, not the draws, are
//! most of the renderer's CPU time. A frame with six path batches (six
//! sparklines) had thirteen render passes.
//!
//! Here all of the frame's instance data, path vertices and path sprites
//! included, goes through one staging buffer; bind groups are kept by
//! [`BindGroupCache`]; pass state is set only when it changes; and path
//! batches are rasterized in groups: consecutive batches that don't come near
//! each other share one rasterization pass, the first group's before the main
//! pass begins. Each batch's pixels in the intermediate texture are then the
//! same as if it had been rasterized alone into the cleared texture, so the
//! image is the same. A frame whose path batches don't overlap is drawn in two
//! passes; one whose batches all overlap still saves a pass, and every
//! `write_buffer` and bind group of its paths.
//!
//! The recording borrows the GPU objects it draws with through
//! [`FrameHost`], so the pixel tests' surfaceless harness records frames
//! through this same code as the on-screen renderer.

use std::ops::Range;

use anyhow::{Context as _, Result};
use gpui::{AtlasTextureId, Bounds, PrimitiveBatch, ScaledPixels, Scene};

use crate::WgpuAtlas;
use crate::fast::bind_groups::BindGroupCache;
use crate::fast::globals::UploadedGlobals;
use crate::fast::pass_state::PassState;
use crate::wgpu_renderer::{
    InstanceData, PathRasterizationVertex, PathSprite, WgpuBindGroupLayouts, WgpuPipelines,
};

/// How far apart, in device pixels, path batches must be to share the
/// intermediate texture. Rasterization is clipped to each path's bounds, and
/// the intermediate texture is sampled with linear filtering at texel
/// centers, so a pixel is plenty; two leave room for rounding.
const PATH_BATCH_MARGIN: f32 = 2.;

/// Past this many path batches in a group, checking that a batch overlaps
/// none of them costs more than the render passes it would save.
const MAX_GROUP_PATH_BATCHES: usize = 64;

/// The renderer's state that outlives a frame.
#[derive(Default)]
pub(crate) struct FrameState {
    pub(crate) bind_groups: BindGroupCache,
    pub(crate) globals: UploadedGlobals,
    vertices: Vec<PathRasterizationVertex>,
    sprites: Vec<PathSprite>,
    path_batches: Vec<PathBatch>,
}

/// The GPU objects a frame is recorded with, borrowed from whoever owns them:
/// the on-screen renderer, or the pixel tests' surfaceless harness.
pub(crate) struct FrameTarget<'a> {
    pub(crate) device: &'a wgpu::Device,
    pub(crate) queue: &'a wgpu::Queue,
    pub(crate) pipelines: &'a WgpuPipelines,
    pub(crate) bind_group_layouts: &'a WgpuBindGroupLayouts,
    pub(crate) atlas: &'a WgpuAtlas,
    pub(crate) atlas_sampler: &'a wgpu::Sampler,
    pub(crate) globals_bind_group: &'a wgpu::BindGroup,
    pub(crate) path_globals_bind_group: &'a wgpu::BindGroup,
    pub(crate) path_intermediate_view: Option<&'a wgpu::TextureView>,
    pub(crate) path_msaa_view: Option<&'a wgpu::TextureView>,
    pub(crate) instance_buffer: &'a wgpu::Buffer,
}

/// The owner of the GPU objects a frame is recorded with.
pub(crate) trait FrameHost {
    fn frame_state(&mut self) -> &mut FrameState;
    /// The alignment of each array in the instance buffer.
    fn instance_data_alignment(&self) -> u64;
    /// Makes the instance buffer hold at least `size` bytes.
    fn reserve_instance_data(&mut self, size: u64) -> Result<()>;
    fn target(&self) -> Result<FrameTarget<'_>>;
}

impl FrameHost for crate::WgpuRenderer {
    fn frame_state(&mut self) -> &mut FrameState {
        &mut self.fast_frame
    }

    fn instance_data_alignment(&self) -> u64 {
        self.instance_data_alignment.max(1)
    }

    fn reserve_instance_data(&mut self, size: u64) -> Result<()> {
        if size > self.instance_data_capacity {
            self.grow_instance_data(size)?;
        }
        Ok(())
    }

    fn target(&self) -> Result<FrameTarget<'_>> {
        let resources = self.resources();
        let InstanceData::Storage(instance_buffer) = &resources.instance_data else {
            anyhow::bail!("the storage buffer transport has no instance buffer");
        };
        Ok(FrameTarget {
            device: &resources.device,
            queue: &resources.queue,
            pipelines: &resources.pipelines,
            bind_group_layouts: &resources.bind_group_layouts,
            atlas: &self.atlas,
            atlas_sampler: &resources.atlas_sampler,
            globals_bind_group: &resources.globals_bind_group,
            path_globals_bind_group: &resources.path_globals_bind_group,
            path_intermediate_view: resources.path_intermediate_view.as_ref(),
            path_msaa_view: resources.path_msaa_view.as_ref(),
            instance_buffer,
        })
    }
}

/// A non-empty `PrimitiveBatch::Paths`: its vertices and sprites in the
/// frame's path uploads, and the bounds everything it draws stays within.
struct PathBatch {
    vertices: Range<u32>,
    sprites: Range<u32>,
    bounds: Bounds<ScaledPixels>,
    /// For the first batch of a group, the vertices of the whole group,
    /// rasterized together before the batch is drawn.
    group: Option<Range<u32>>,
}

/// Where the frame's instance data landed in the instance buffer.
struct Upload {
    quads: wgpu::BindGroup,
    shadows: wgpu::BindGroup,
    underlines: wgpu::BindGroup,
    monochrome_sprites: wgpu::BindGroup,
    subpixel_sprites: wgpu::BindGroup,
    polychrome_sprites: wgpu::BindGroup,
    path_vertices: wgpu::BindGroup,
    path_sprites: wgpu::BindGroup,
}

/// Forwarded to by `WgpuRenderer::record_frame`. Records and submits the
/// frame and returns true, or returns false for upstream to record it: the
/// WebGL instance texture keeps upstream's way.
pub(crate) fn record_frame(
    renderer: &mut crate::WgpuRenderer,
    scene: &Scene,
    frame_view: &wgpu::TextureView,
) -> Result<bool> {
    if renderer.uses_webgl_instance_data {
        return Ok(false);
    }
    record_into(renderer, scene, frame_view, wgpu::Color::TRANSPARENT).map(|()| true)
}

/// Records `scene` into `frame_view`, cleared to `clear` first, and submits it.
pub(crate) fn record_into(
    host: &mut impl FrameHost,
    scene: &Scene,
    frame_view: &wgpu::TextureView,
    clear: wgpu::Color,
) -> Result<()> {
    // The state is taken for the frame so that it can change while the host
    // lends out the GPU objects.
    let mut state = std::mem::take(host.frame_state());
    let result = record_with(host, &mut state, scene, frame_view, clear);
    *host.frame_state() = state;
    result
}

fn record_with(
    host: &mut impl FrameHost,
    state: &mut FrameState,
    scene: &Scene,
    frame_view: &wgpu::TextureView,
    clear: wgpu::Color,
) -> Result<()> {
    state.bind_groups.begin_frame();

    let mut vertices = std::mem::take(&mut state.vertices);
    let mut sprites = std::mem::take(&mut state.sprites);
    let mut path_batches = std::mem::take(&mut state.path_batches);
    plan_paths(scene, &mut vertices, &mut sprites, &mut path_batches);

    let result = upload(host, &state.bind_groups, scene, &vertices, &sprites)
        .with_context(|| {
            format!(
                "scene too large: {} paths, {} shadows, {} quads, {} underlines, {} monochrome sprites, {} subpixel sprites, {} polychrome sprites",
                scene.paths.len(),
                scene.shadows.len(),
                scene.quads.len(),
                scene.underlines.len(),
                scene.monochrome_sprites.len(),
                scene.subpixel_sprites.len(),
                scene.polychrome_sprites.len(),
            )
        })
        .and_then(|upload| {
            let target = host.target()?;
            record(&target, state, scene, frame_view, clear, &upload, &path_batches);
            Ok(())
        });

    vertices.clear();
    sprites.clear();
    path_batches.clear();
    state.vertices = vertices;
    state.sprites = sprites;
    state.path_batches = path_batches;
    result
}

/// Collects every path batch's rasterization vertices and sprites, as
/// upstream's `draw_paths_to_intermediate` and `draw_paths_from_intermediate`
/// build them, and groups the batches.
fn plan_paths(
    scene: &Scene,
    vertices: &mut Vec<PathRasterizationVertex>,
    sprites: &mut Vec<PathSprite>,
    path_batches: &mut Vec<PathBatch>,
) {
    if scene.paths.is_empty() {
        return;
    }
    let mut group_start = path_batches.len();
    for batch in scene.batches() {
        let PrimitiveBatch::Paths(range) = batch else {
            continue;
        };
        let paths = &scene.paths[range];
        let Some(first_path) = paths.first() else {
            continue;
        };

        let first_vertex = vertices.len() as u32;
        let mut union = first_path.clipped_bounds();
        for path in paths {
            let bounds = path.clipped_bounds();
            union = union.union(&bounds);
            vertices.extend(path.vertices.iter().map(|v| PathRasterizationVertex {
                xy_position: v.xy_position,
                st_position: v.st_position,
                color: path.color,
                bounds,
            }));
        }

        let first_sprite = sprites.len() as u32;
        if paths.last().map(|p| &p.order) == Some(&first_path.order) {
            sprites.extend(paths.iter().map(|p| PathSprite {
                bounds: p.clipped_bounds(),
            }));
        } else {
            sprites.push(PathSprite { bounds: union });
        }

        let near = union.dilate(ScaledPixels(PATH_BATCH_MARGIN));
        let group = &path_batches[group_start..];
        if group.is_empty()
            || group.len() >= MAX_GROUP_PATH_BATCHES
            || group.iter().any(|batch| near.intersects(&batch.bounds))
        {
            close_group(&mut path_batches[group_start..]);
            group_start = path_batches.len();
        }
        path_batches.push(PathBatch {
            vertices: first_vertex..vertices.len() as u32,
            sprites: first_sprite..sprites.len() as u32,
            bounds: union,
            group: None,
        });
    }
    close_group(&mut path_batches[group_start..]);
}

/// Gives the first batch of `group` the vertices of all of it.
fn close_group(group: &mut [PathBatch]) {
    if let Some(end) = group.last().map(|last| last.vertices.end)
        && let Some(first) = group.first_mut()
    {
        first.group = Some(first.vertices.start..end);
    }
}

/// Writes the frame's instance data through one staging buffer, laid out as
/// upstream's `write_instance_binding` lays out each array.
fn upload(
    host: &mut impl FrameHost,
    bind_groups: &BindGroupCache,
    scene: &Scene,
    vertices: &[PathRasterizationVertex],
    sprites: &[PathSprite],
) -> Result<Upload> {
    // SAFETY: the primitives and path records are `#[repr(C)]` plain data, as
    // upstream's `write_instance_binding` relies on too.
    let arrays: [&[u8]; 8] = unsafe {
        [
            bytes_of(&scene.quads),
            bytes_of(&scene.shadows),
            bytes_of(&scene.underlines),
            bytes_of(&scene.monochrome_sprites),
            bytes_of(&scene.subpixel_sprites),
            bytes_of(&scene.polychrome_sprites),
            bytes_of(vertices),
            bytes_of(sprites),
        ]
    };

    let alignment = host.instance_data_alignment();
    let mut offsets = [0u64; 8];
    let mut end = 0u64;
    for (offset, data) in offsets.iter_mut().zip(arrays) {
        *offset = end.next_multiple_of(alignment);
        // wgpu rejects zero-sized bindings, so empty arrays still reserve the
        // 16-byte minimum.
        end = *offset + binding_size(data);
    }
    let end = end.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
    host.reserve_instance_data(end)?;

    let target = host.target()?;
    let buffer = target.instance_buffer;
    if arrays.iter().any(|data| !data.is_empty()) {
        let size = wgpu::BufferSize::new(end).context("empty instance upload")?;
        let mut view = target
            .queue
            .write_buffer_with(buffer, 0, size)
            .context("instance upload rejected")?;
        for (&offset, data) in offsets.iter().zip(arrays) {
            if !data.is_empty() {
                let offset = offset as usize;
                view.slice(offset..offset + data.len())
                    .copy_from_slice(data);
            }
        }
    }

    let bind = |index: usize, label: &str| {
        bind_groups.storage(
            target.device,
            &target.bind_group_layouts.instances,
            label,
            buffer,
            offsets[index],
            binding_size(arrays[index]),
        )
    };
    Ok(Upload {
        quads: bind(0, "quads_bind_group"),
        shadows: bind(1, "shadows_bind_group"),
        underlines: bind(2, "underlines_bind_group"),
        monochrome_sprites: bind(3, "monochrome_sprites_bind_group"),
        subpixel_sprites: bind(4, "subpixel_sprites_bind_group"),
        polychrome_sprites: bind(5, "polychrome_sprites_bind_group"),
        path_vertices: bind(6, "path_rasterization_bind_group"),
        path_sprites: bind(7, "path_sprites_bind_group"),
    })
}

fn binding_size(data: &[u8]) -> u64 {
    (data.len() as u64).max(16)
}

unsafe fn bytes_of<T>(instances: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            instances.as_ptr() as *const u8,
            std::mem::size_of_val(instances),
        )
    }
}

fn record(
    target: &FrameTarget,
    state: &FrameState,
    scene: &Scene,
    frame_view: &wgpu::TextureView,
    clear: wgpu::Color,
    upload: &Upload,
    path_batches: &[PathBatch],
) {
    let mut encoder = target
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("main_encoder"),
        });
    draw_scene(
        target,
        state,
        &mut encoder,
        &SceneDraw {
            scene,
            upload,
            path_batches,
            view: frame_view,
            clear,
            label: "main_pass",
            globals: target.globals_bind_group,
            paths: PathTargets {
                globals: target.path_globals_bind_group,
                intermediate: target.path_intermediate_view,
                msaa: target.path_msaa_view,
            },
        },
    );
    target.queue.submit(std::iter::once(encoder.finish()));
}

/// A scene to draw, where, and with what.
struct SceneDraw<'a> {
    scene: &'a Scene,
    upload: &'a Upload,
    path_batches: &'a [PathBatch],
    view: &'a wgpu::TextureView,
    clear: wgpu::Color,
    label: &'a str,
    /// The globals the scene's primitives are drawn with.
    globals: &'a wgpu::BindGroup,
    paths: PathTargets<'a>,
}

/// What a scene's paths are rasterized with: the globals and the
/// intermediate texture, both sized like the texture the scene is drawn into.
#[derive(Clone, Copy)]
struct PathTargets<'a> {
    globals: &'a wgpu::BindGroup,
    intermediate: Option<&'a wgpu::TextureView>,
    msaa: Option<&'a wgpu::TextureView>,
}

/// Records the passes that draw `draw.scene` into `draw.view`.
fn draw_scene(
    target: &FrameTarget,
    state: &FrameState,
    encoder: &mut wgpu::CommandEncoder,
    draw: &SceneDraw,
) {
    let pipelines = target.pipelines;
    let upload = draw.upload;
    let texture_bind_group = |label: &str, view: &wgpu::TextureView| {
        state.bind_groups.texture(
            target.device,
            &target.bind_group_layouts.texture,
            target.atlas_sampler,
            label,
            view,
        )
    };
    let intermediate = draw
        .paths
        .intermediate
        .map(|view| texture_bind_group("path_intermediate_texture_bind_group", view));

    // The first group is rasterized before the main pass, which saves ending
    // the main pass for it.
    let mut rasterized = draw.path_batches.first().is_some_and(|first| {
        rasterize_group(target, encoder, draw.paths, upload, first.group.clone())
    });

    let mut pass = begin_main_pass(encoder, draw.view, draw.label, Some(draw.clear));
    let mut bound = PassState::default();
    let mut path_batches = draw.path_batches.iter().enumerate();

    for batch in draw.scene.batches() {
        match batch {
            PrimitiveBatch::Quads(range) => draw_batch(
                &mut pass,
                &mut bound,
                draw.globals,
                &pipelines.quads,
                &upload.quads,
                None,
                range,
            ),
            PrimitiveBatch::Shadows(range) => draw_batch(
                &mut pass,
                &mut bound,
                draw.globals,
                &pipelines.shadows,
                &upload.shadows,
                None,
                range,
            ),
            PrimitiveBatch::Paths(range) => {
                if range.is_empty() {
                    continue;
                }
                let Some((index, batch)) = path_batches.next() else {
                    continue;
                };
                if index > 0 && batch.group.is_some() {
                    drop(pass);
                    rasterized =
                        rasterize_group(target, encoder, draw.paths, upload, batch.group.clone());
                    pass = begin_main_pass(encoder, draw.view, "main_pass_continued", None);
                    bound.forget();
                }
                if !rasterized || batch.vertices.is_empty() {
                    continue;
                }
                let Some(intermediate) = &intermediate else {
                    continue;
                };
                bound.set_pipeline(&mut pass, &pipelines.paths);
                bound.set_bind_group(&mut pass, 0, draw.globals);
                bound.set_bind_group(&mut pass, 1, &upload.path_sprites);
                bound.set_bind_group(&mut pass, 2, intermediate);
                pass.draw(0..4, batch.sprites.clone());
            }
            PrimitiveBatch::Underlines(range) => draw_batch(
                &mut pass,
                &mut bound,
                draw.globals,
                &pipelines.underlines,
                &upload.underlines,
                None,
                range,
            ),
            PrimitiveBatch::MonochromeSprites { texture_id, range } => draw_batch(
                &mut pass,
                &mut bound,
                draw.globals,
                &pipelines.mono_sprites,
                &upload.monochrome_sprites,
                Some(&atlas_bind_group(
                    target.atlas,
                    &texture_bind_group,
                    texture_id,
                )),
                range,
            ),
            PrimitiveBatch::SubpixelSprites { texture_id, range } => draw_batch(
                &mut pass,
                &mut bound,
                draw.globals,
                pipelines
                    .subpixel_sprites
                    .as_ref()
                    .unwrap_or(&pipelines.mono_sprites),
                &upload.subpixel_sprites,
                Some(&atlas_bind_group(
                    target.atlas,
                    &texture_bind_group,
                    texture_id,
                )),
                range,
            ),
            PrimitiveBatch::PolychromeSprites { texture_id, range } => draw_batch(
                &mut pass,
                &mut bound,
                draw.globals,
                &pipelines.poly_sprites,
                &upload.polychrome_sprites,
                Some(&atlas_bind_group(
                    target.atlas,
                    &texture_bind_group,
                    texture_id,
                )),
                range,
            ),
            // Surfaces are macOS-only for video playback and are not
            // implemented by the WGPU renderer.
            PrimitiveBatch::Surfaces(_surfaces) => {}
        }
    }
    drop(pass);
}

fn atlas_bind_group(
    atlas: &WgpuAtlas,
    texture_bind_group: &impl Fn(&str, &wgpu::TextureView) -> wgpu::BindGroup,
    texture_id: AtlasTextureId,
) -> wgpu::BindGroup {
    let texture_info = atlas.get_texture_info(texture_id);
    texture_bind_group("atlas_texture_bind_group", &texture_info.view)
}

/// Begins a pass that draws into `view`, cleared to `clear` if one is given.
fn begin_main_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &'a wgpu::TextureView,
    label: &str,
    clear: Option<wgpu::Color>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: match clear {
                    Some(color) => wgpu::LoadOp::Clear(color),
                    None => wgpu::LoadOp::Load,
                },
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        ..Default::default()
    })
}

fn draw_batch(
    pass: &mut wgpu::RenderPass<'_>,
    state: &mut PassState,
    globals: &wgpu::BindGroup,
    pipeline: &wgpu::RenderPipeline,
    instances: &wgpu::BindGroup,
    texture: Option<&wgpu::BindGroup>,
    range: Range<usize>,
) {
    if range.is_empty() {
        return;
    }
    state.set_pipeline(pass, pipeline);
    state.set_bind_group(pass, 0, globals);
    state.set_bind_group(pass, 1, instances);
    if let Some(texture) = texture {
        state.set_bind_group(pass, 2, texture);
    }
    pass.draw(0..4, range.start as u32..range.end as u32);
}

/// Rasterizes a group's range of the frame's path vertices into the cleared
/// intermediate texture, in a pass of its own, as upstream's
/// `draw_paths_to_intermediate` does for a batch. Returns false if there is
/// nothing to rasterize or no intermediate texture to rasterize into.
fn rasterize_group(
    target: &FrameTarget,
    encoder: &mut wgpu::CommandEncoder,
    paths: PathTargets,
    upload: &Upload,
    vertices: Option<Range<u32>>,
) -> bool {
    let Some(vertices) = vertices.filter(|vertices| !vertices.is_empty()) else {
        return false;
    };
    let Some(path_intermediate_view) = paths.intermediate else {
        return false;
    };
    let (target_view, resolve_target) = match paths.msaa {
        Some(msaa_view) => (msaa_view, Some(path_intermediate_view)),
        None => (path_intermediate_view, None),
    };
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("path_rasterization_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            resolve_target,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        ..Default::default()
    });
    pass.set_pipeline(&target.pipelines.path_rasterization);
    pass.set_bind_group(0, paths.globals, &[]);
    pass.set_bind_group(1, &upload.path_vertices, &[]);
    pass.draw(vertices, 0..1);
    true
}
