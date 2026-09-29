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

use std::ops::Range;

use anyhow::{Context as _, Result};
use gpui::{AtlasTextureId, Bounds, PrimitiveBatch, ScaledPixels, Scene};

use crate::fast::bind_groups::BindGroupCache;
use crate::fast::globals::UploadedGlobals;
use crate::fast::pass_state::PassState;
use crate::wgpu_renderer::{InstanceData, PathRasterizationVertex, PathSprite, WgpuResources};

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
    renderer.fast_frame.bind_groups.begin_frame();

    let mut vertices = std::mem::take(&mut renderer.fast_frame.vertices);
    let mut sprites = std::mem::take(&mut renderer.fast_frame.sprites);
    let mut path_batches = std::mem::take(&mut renderer.fast_frame.path_batches);
    plan_paths(scene, &mut vertices, &mut sprites, &mut path_batches);

    let result = upload(renderer, scene, &vertices, &sprites)
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
        .map(|upload| {
            record(renderer, scene, frame_view, &upload, &path_batches)
        });

    vertices.clear();
    sprites.clear();
    path_batches.clear();
    renderer.fast_frame.vertices = vertices;
    renderer.fast_frame.sprites = sprites;
    renderer.fast_frame.path_batches = path_batches;
    result.map(|()| true)
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
    let mut group_start = 0;
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
    renderer: &mut crate::WgpuRenderer,
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

    let alignment = renderer.instance_data_alignment.max(1);
    let mut offsets = [0u64; 8];
    let mut end = 0u64;
    for (offset, data) in offsets.iter_mut().zip(arrays) {
        *offset = end.next_multiple_of(alignment);
        // wgpu rejects zero-sized bindings, so empty arrays still reserve the
        // 16-byte minimum.
        end = *offset + binding_size(data);
    }
    let end = end.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
    if end > renderer.instance_data_capacity {
        renderer.grow_instance_data(end)?;
    }

    let resources = renderer.resources();
    let InstanceData::Storage(buffer) = &resources.instance_data else {
        anyhow::bail!("the storage buffer transport has no instance buffer");
    };
    if arrays.iter().any(|data| !data.is_empty()) {
        let size = wgpu::BufferSize::new(end).context("empty instance upload")?;
        let mut view = resources
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

    let bind_groups = &renderer.fast_frame.bind_groups;
    let bind = |index: usize, label: &str| {
        bind_groups.storage(
            &resources.device,
            &resources.bind_group_layouts.instances,
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
    renderer: &crate::WgpuRenderer,
    scene: &Scene,
    frame_view: &wgpu::TextureView,
    upload: &Upload,
    path_batches: &[PathBatch],
) {
    let resources = renderer.resources();
    let pipelines = &resources.pipelines;
    let bind_groups = &renderer.fast_frame.bind_groups;
    let texture_bind_group = |label: &str, view: &wgpu::TextureView| {
        bind_groups.texture(
            &resources.device,
            &resources.bind_group_layouts.texture,
            &resources.atlas_sampler,
            label,
            view,
        )
    };
    let intermediate = resources
        .path_intermediate_view
        .as_ref()
        .map(|view| texture_bind_group("path_intermediate_texture_bind_group", view));

    let mut encoder = resources
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("main_encoder"),
        });

    // The first group is rasterized before the main pass, which saves ending
    // the main pass for it.
    let mut rasterized = path_batches
        .first()
        .is_some_and(|first| rasterize_group(resources, &mut encoder, upload, first.group.clone()));

    let mut pass = begin_main_pass(&mut encoder, frame_view, "main_pass", true);
    let mut state = PassState::default();
    let mut path_batches = path_batches.iter().enumerate();

    for batch in scene.batches() {
        match batch {
            PrimitiveBatch::Quads(range) => draw(
                &mut pass,
                &mut state,
                resources,
                &pipelines.quads,
                &upload.quads,
                None,
                range,
            ),
            PrimitiveBatch::Shadows(range) => draw(
                &mut pass,
                &mut state,
                resources,
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
                        rasterize_group(resources, &mut encoder, upload, batch.group.clone());
                    pass = begin_main_pass(&mut encoder, frame_view, "main_pass_continued", false);
                    state.forget();
                }
                if !rasterized || batch.vertices.is_empty() {
                    continue;
                }
                let Some(intermediate) = &intermediate else {
                    continue;
                };
                state.set_pipeline(&mut pass, &pipelines.paths);
                state.set_bind_group(&mut pass, 0, &resources.globals_bind_group);
                state.set_bind_group(&mut pass, 1, &upload.path_sprites);
                state.set_bind_group(&mut pass, 2, intermediate);
                pass.draw(0..4, batch.sprites.clone());
            }
            PrimitiveBatch::Underlines(range) => draw(
                &mut pass,
                &mut state,
                resources,
                &pipelines.underlines,
                &upload.underlines,
                None,
                range,
            ),
            PrimitiveBatch::MonochromeSprites { texture_id, range } => draw(
                &mut pass,
                &mut state,
                resources,
                &pipelines.mono_sprites,
                &upload.monochrome_sprites,
                Some(&atlas_bind_group(renderer, &texture_bind_group, texture_id)),
                range,
            ),
            PrimitiveBatch::SubpixelSprites { texture_id, range } => draw(
                &mut pass,
                &mut state,
                resources,
                pipelines
                    .subpixel_sprites
                    .as_ref()
                    .unwrap_or(&pipelines.mono_sprites),
                &upload.subpixel_sprites,
                Some(&atlas_bind_group(renderer, &texture_bind_group, texture_id)),
                range,
            ),
            PrimitiveBatch::PolychromeSprites { texture_id, range } => draw(
                &mut pass,
                &mut state,
                resources,
                &pipelines.poly_sprites,
                &upload.polychrome_sprites,
                Some(&atlas_bind_group(renderer, &texture_bind_group, texture_id)),
                range,
            ),
            // Surfaces are macOS-only for video playback and are not
            // implemented by the WGPU renderer.
            PrimitiveBatch::Surfaces(_surfaces) => {}
        }
    }
    drop(pass);

    resources.queue.submit(std::iter::once(encoder.finish()));
}

fn atlas_bind_group(
    renderer: &crate::WgpuRenderer,
    texture_bind_group: &impl Fn(&str, &wgpu::TextureView) -> wgpu::BindGroup,
    texture_id: AtlasTextureId,
) -> wgpu::BindGroup {
    let texture_info = renderer.atlas.get_texture_info(texture_id);
    texture_bind_group("atlas_texture_bind_group", &texture_info.view)
}

fn begin_main_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    frame_view: &'a wgpu::TextureView,
    label: &str,
    clear: bool,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: frame_view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        ..Default::default()
    })
}

fn draw(
    pass: &mut wgpu::RenderPass<'_>,
    state: &mut PassState,
    resources: &WgpuResources,
    pipeline: &wgpu::RenderPipeline,
    instances: &wgpu::BindGroup,
    texture: Option<&wgpu::BindGroup>,
    range: Range<usize>,
) {
    if range.is_empty() {
        return;
    }
    state.set_pipeline(pass, pipeline);
    state.set_bind_group(pass, 0, &resources.globals_bind_group);
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
    resources: &WgpuResources,
    encoder: &mut wgpu::CommandEncoder,
    upload: &Upload,
    vertices: Option<Range<u32>>,
) -> bool {
    let Some(vertices) = vertices.filter(|vertices| !vertices.is_empty()) else {
        return false;
    };
    let Some(path_intermediate_view) = resources.path_intermediate_view.as_ref() else {
        return false;
    };
    let (target_view, resolve_target) = match &resources.path_msaa_view {
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
    pass.set_pipeline(&resources.pipelines.path_rasterization);
    pass.set_bind_group(0, &resources.path_globals_bind_group, &[]);
    pass.set_bind_group(1, &upload.path_vertices, &[]);
    pass.draw(vertices, 0..1);
    true
}
