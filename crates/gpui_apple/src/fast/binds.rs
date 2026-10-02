//! Instanced draws that set only the encoder state the draw before left
//! different.
//!
//! Upstream's draw functions each set, before every batch, the pipeline, the
//! unit vertices, the instances for the vertex and fragment stages, the
//! viewport size and, for sprites, the atlas texture and its size, although
//! the batch before mostly left them bound: the unit vertices and the
//! viewport never change within a pass, every kind's instances are one
//! buffer at different offsets, and sprite batches mostly sample the same
//! atlas texture. Metal's validation layer reports those binds as redundant,
//! and it reports the monochrome sprites' instances bound for a fragment
//! shader that never reads them.
//!
//! [`Binds`] remembers what a pass has bound, and [`MetalRenderer::draw_bound`]
//! draws a batch of any instanced kind, setting only the rest and moving just
//! the offset of a buffer that is already bound. The draws are upstream's,
//! argument for argument.

use std::{ffi::c_void, ops::Range};

use gpui::{AtlasTextureId, DevicePixels, PrimitiveBatch, Size, size};
use metal::{
    BufferRef, RenderCommandEncoderRef, RenderPipelineStateRef, TextureRef,
    foreign_types::ForeignTypeRef as _,
};

use super::{InstanceBinding, InstanceBindings, MetalRenderer, SpriteInputIndex};

/// What a buffer argument slot holds.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Slot {
    /// Nothing this pass set, as far as [`Binds`] knows.
    #[default]
    Unknown,
    Buffer {
        buffer: *const c_void,
        offset: u64,
    },
    /// Eight bytes set inline.
    Bytes(u64),
}

/// What one render command encoder has bound, for the instanced draws.
pub(crate) struct Binds {
    pipeline: *const c_void,
    /// The vertex stage's buffer slots 0 to 3: unit vertices, instances,
    /// viewport size, atlas size.
    vertex: [Slot; 4],
    /// The fragment stage's instances.
    fragment: Slot,
    /// The fragment stage's table of edge fades.
    fades: Slot,
    atlas: *const c_void,
}

impl Default for Binds {
    fn default() -> Self {
        Self {
            pipeline: std::ptr::null(),
            vertex: [Slot::Unknown; 4],
            fragment: Slot::Unknown,
            fades: Slot::Unknown,
            atlas: std::ptr::null(),
        }
    }
}

impl Binds {
    /// Forgets what is bound, after something else drew with the encoder.
    pub(crate) fn forget(&mut self) {
        *self = Self::default();
    }

    fn pipeline(&mut self, encoder: &RenderCommandEncoderRef, pipeline: &RenderPipelineStateRef) {
        let pointer = pipeline.as_ptr() as *const c_void;
        if self.pipeline != pointer {
            encoder.set_render_pipeline_state(pipeline);
            self.pipeline = pointer;
        }
    }

    fn vertex_buffer(
        &mut self,
        encoder: &RenderCommandEncoderRef,
        index: usize,
        buffer: &BufferRef,
        offset: u64,
    ) {
        let pointer = buffer.as_ptr() as *const c_void;
        let wanted = Slot::Buffer {
            buffer: pointer,
            offset,
        };
        match self.vertex[index] {
            slot if slot == wanted => return,
            Slot::Buffer { buffer, .. } if buffer == pointer => {
                encoder.set_vertex_buffer_offset(index as u64, offset)
            }
            _ => encoder.set_vertex_buffer(index as u64, Some(buffer), offset),
        }
        self.vertex[index] = wanted;
    }

    fn vertex_size(
        &mut self,
        encoder: &RenderCommandEncoderRef,
        index: usize,
        value: Size<DevicePixels>,
    ) {
        let bytes = (u64::from(value.width.0 as u32) << 32) | u64::from(value.height.0 as u32);
        if self.vertex[index] == Slot::Bytes(bytes) {
            return;
        }
        encoder.set_vertex_bytes(
            index as u64,
            size_of::<Size<DevicePixels>>() as u64,
            &value as *const Size<DevicePixels> as *const c_void,
        );
        self.vertex[index] = Slot::Bytes(bytes);
    }

    fn fragment_buffer(
        &mut self,
        encoder: &RenderCommandEncoderRef,
        buffer: &BufferRef,
        offset: u64,
    ) {
        let index = SpriteInputIndex::Sprites as u64;
        let pointer = buffer.as_ptr() as *const c_void;
        let wanted = Slot::Buffer {
            buffer: pointer,
            offset,
        };
        match self.fragment {
            slot if slot == wanted => return,
            Slot::Buffer { buffer, .. } if buffer == pointer => {
                encoder.set_fragment_buffer_offset(index, offset)
            }
            _ => encoder.set_fragment_buffer(index, Some(buffer), offset),
        }
        self.fragment = wanted;
    }

    fn fades(&mut self, encoder: &RenderCommandEncoderRef, fades: &InstanceBinding) {
        let wanted = Slot::Buffer {
            buffer: fades.buffer.as_ptr() as *const c_void,
            offset: fades.offset as u64,
        };
        if self.fades != wanted {
            crate::fast::edge_fade::bind(encoder, fades);
            self.fades = wanted;
        }
    }

    fn atlas(&mut self, encoder: &RenderCommandEncoderRef, texture: &TextureRef) {
        let pointer = texture.as_ptr() as *const c_void;
        if self.atlas != pointer {
            encoder.set_fragment_texture(SpriteInputIndex::AtlasTexture as u64, Some(texture));
            self.atlas = pointer;
        }
    }
}

/// One batch of an instanced kind.
pub(crate) struct Instanced<'a> {
    pub(crate) pipeline: &'a RenderPipelineStateRef,
    pub(crate) instances: &'a InstanceBinding,
    /// Whether the kind's fragment shader reads its instance.
    pub(crate) fragment_reads_instances: bool,
    /// The frame's table of edge fades, for a kind whose fragment shader
    /// reads it.
    pub(crate) fades: Option<&'a InstanceBinding>,
    pub(crate) atlas: Option<&'a TextureRef>,
    pub(crate) range: Range<usize>,
}

impl MetalRenderer {
    /// Draws `batch` if it is of an instanced kind, returning whether it
    /// was, binding through `binds`. The draw is upstream's.
    pub(crate) fn draw_bound(
        &self,
        batch: &PrimitiveBatch,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        encoder: &RenderCommandEncoderRef,
        binds: &mut Binds,
    ) -> bool {
        let draw = match batch {
            PrimitiveBatch::Shadows(range) => Instanced {
                pipeline: &self.shadows_pipeline_state,
                instances: &instance_bindings.shadows,
                fragment_reads_instances: true,
                fades: Some(&instance_bindings.fades),
                atlas: None,
                range: range.clone(),
            },
            PrimitiveBatch::Quads(range) => Instanced {
                pipeline: &self.quads_pipeline_state,
                instances: &instance_bindings.quads,
                fragment_reads_instances: true,
                fades: Some(&instance_bindings.fades),
                atlas: None,
                range: range.clone(),
            },
            PrimitiveBatch::Underlines(range) => Instanced {
                pipeline: &self.underlines_pipeline_state,
                instances: &instance_bindings.underlines,
                fragment_reads_instances: true,
                fades: Some(&instance_bindings.fades),
                atlas: None,
                range: range.clone(),
            },
            PrimitiveBatch::MonochromeSprites { texture_id, range } => {
                let Some(atlas) = self.atlas_texture(*texture_id) else {
                    return true;
                };
                return self.draw_instanced(
                    Instanced {
                        pipeline: &self.monochrome_sprites_pipeline_state,
                        instances: &instance_bindings.monochrome_sprites,
                        fragment_reads_instances: false,
                        fades: Some(&instance_bindings.fades),
                        atlas: Some(&atlas),
                        range: range.clone(),
                    },
                    viewport_size,
                    encoder,
                    binds,
                );
            }
            PrimitiveBatch::PolychromeSprites { texture_id, range } => {
                if crate::fast::layers::composite::draw_tiles(
                    self,
                    *texture_id,
                    range,
                    instance_bindings,
                    viewport_size,
                    encoder,
                    binds,
                ) {
                    return true;
                }
                let Some(atlas) = self.atlas_texture(*texture_id) else {
                    return true;
                };
                return self.draw_instanced(
                    Instanced {
                        pipeline: &self.polychrome_sprites_pipeline_state,
                        instances: &instance_bindings.polychrome_sprites,
                        fragment_reads_instances: true,
                        fades: Some(&instance_bindings.fades),
                        atlas: Some(&atlas),
                        range: range.clone(),
                    },
                    viewport_size,
                    encoder,
                    binds,
                );
            }
            _ => return false,
        };
        self.draw_instanced(draw, viewport_size, encoder, binds)
    }

    /// Draws the holes `range` of the scene's natives, as
    /// [`MetalRenderer::draw_holes`] does, binding through `binds`.
    pub(crate) fn draw_holes_bound(
        &self,
        range: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        encoder: &RenderCommandEncoderRef,
        binds: &mut Binds,
    ) {
        self.draw_instanced(
            Instanced {
                pipeline: &self.holes_pipeline_state,
                instances: &instance_bindings.holes,
                fragment_reads_instances: true,
                fades: Some(&instance_bindings.fades),
                atlas: None,
                range,
            },
            viewport_size,
            encoder,
            binds,
        );
    }

    fn atlas_texture(&self, texture_id: AtlasTextureId) -> Option<metal::Texture> {
        self.sprite_atlas.metal_texture(texture_id)
    }

    pub(crate) fn draw_instanced(
        &self,
        draw: Instanced<'_>,
        viewport_size: Size<DevicePixels>,
        encoder: &RenderCommandEncoderRef,
        binds: &mut Binds,
    ) -> bool {
        if draw.range.is_empty() {
            return true;
        }
        // Every instanced kind's shaders take the unit vertices, the
        // instances, the viewport size and the atlas size at the sprites'
        // indices.
        binds.pipeline(encoder, draw.pipeline);
        binds.vertex_buffer(
            encoder,
            SpriteInputIndex::Vertices as usize,
            &self.unit_vertices,
            0,
        );
        binds.vertex_buffer(
            encoder,
            SpriteInputIndex::Sprites as usize,
            &draw.instances.buffer,
            draw.instances.offset as u64,
        );
        binds.vertex_size(
            encoder,
            SpriteInputIndex::ViewportSize as usize,
            viewport_size,
        );
        if draw.fragment_reads_instances {
            binds.fragment_buffer(
                encoder,
                &draw.instances.buffer,
                draw.instances.offset as u64,
            );
        }
        if let Some(fades) = draw.fades {
            binds.fades(encoder, fades);
        }
        if let Some(atlas) = draw.atlas {
            let atlas_size = size(
                DevicePixels(atlas.width() as i32),
                DevicePixels(atlas.height() as i32),
            );
            binds.vertex_size(
                encoder,
                SpriteInputIndex::AtlasTextureSize as usize,
                atlas_size,
            );
            binds.atlas(encoder, atlas);
        }
        encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            draw.range.len() as u64,
            draw.range.start as u64,
        );
        true
    }
}

/// Whether a frame without paths is encoded by upstream's loop, with
/// upstream's draws, for tests to compare against.
#[cfg(not(test))]
#[inline(always)]
pub(crate) fn upstream_loop() -> bool {
    false
}

#[cfg(test)]
thread_local! {
    pub(crate) static UPSTREAM_LOOP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn upstream_loop() -> bool {
    UPSTREAM_LOOP.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, time::Instant};

    use gpui::{
        AtlasKey, Bounds, ContentMask, Corners, DevicePixels, MonochromeSprite, PlatformAtlas as _,
        Quad, RenderSvgParams, ScaledPixels, Scene, Shadow, TransformationMatrix, Underline, hsla,
        point, size,
    };
    use metal::MTLPixelFormat;

    use super::{super::InstanceBufferPool, MetalRenderer, UPSTREAM_LOOP};

    const SIDE: i32 = 512;

    fn renderer() -> MetalRenderer {
        MetalRenderer::new_headless(std::sync::Arc::new(parking_lot::Mutex::new(
            InstanceBufferPool::default(),
        )))
    }

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
        Bounds::new(
            point(ScaledPixels(x), ScaledPixels(y)),
            size(ScaledPixels(width), ScaledPixels(height)),
        )
    }

    /// `cards` overlapping cards, each a shadow, a background, a line of
    /// glyphs and an underline, so each card draws in four batches of their
    /// own; then a grid of glyphs over row backgrounds, which draws in two.
    fn scene(renderer: &MetalRenderer, cards: usize) -> Scene {
        let tile = renderer
            .sprite_atlas
            .get_or_insert_with(
                AtlasKey::Svg(RenderSvgParams {
                    path: "a glyph".into(),
                    size: size(DevicePixels(8), DevicePixels(12)),
                }),
                &mut || {
                    let alpha: Vec<u8> = (0..96).map(|texel| (texel * 5 % 256) as u8).collect();
                    Ok(Some((
                        size(DevicePixels(8), DevicePixels(12)),
                        Cow::Owned(alpha),
                    )))
                },
            )
            .unwrap()
            .unwrap();
        let everywhere = ContentMask {
            bounds: bounds(0., 0., SIDE as f32, SIDE as f32),
        };
        let glyph = |x: f32, y: f32, hue: f32| MonochromeSprite {
            order: 0,
            pad: 0,
            bounds: bounds(x, y, 8., 12.),
            content_mask: everywhere,
            color: hsla(hue, 0.6, 0.7, 1.),
            tile,
            transformation: TransformationMatrix::unit(),
        };
        let mut scene = Scene::default();
        for row in 0..40 {
            let y = row as f32 * 12.;
            scene.insert_primitive(Quad {
                bounds: bounds(0., y, SIDE as f32, 12.),
                content_mask: everywhere,
                background: hsla(0.6, 0.2, 0.1 + row as f32 * 0.005, 1.).into(),
                ..Quad::default()
            });
            for column in 0..60 {
                scene.insert_primitive(glyph(column as f32 * 8., y, row as f32 / 40.));
            }
        }
        for card in 0..cards {
            let x = 20. + (card % 20) as f32 * 18.;
            let y = 20. + card as f32 * 7.;
            let card_bounds = bounds(x, y, 160., 40.);
            scene.insert_primitive(Shadow {
                bounds: card_bounds,
                content_mask: everywhere,
                blur_radius: ScaledPixels(6.),
                corner_radii: Corners::all(ScaledPixels(4.)),
                color: hsla(0., 0., 0., 0.5),
                element_bounds: card_bounds,
                element_corner_radii: Corners::all(ScaledPixels(4.)),
                order: 0,
                inset: 0,
                pad: 0,
            });
            scene.insert_primitive(Quad {
                bounds: card_bounds,
                content_mask: everywhere,
                background: hsla(card as f32 / cards as f32, 0.5, 0.4, 0.9).into(),
                ..Quad::default()
            });
            for column in 0..16 {
                scene.insert_primitive(glyph(x + 8. + column as f32 * 8., y + 8., 0.1));
            }
            scene.insert_primitive(Underline {
                bounds: bounds(x + 8., y + 22., 128., 2.),
                content_mask: everywhere,
                color: hsla(0.1, 0.9, 0.6, 1.),
                thickness: ScaledPixels(1.),
                wavy: false.into(),
                order: 0,
                pad: 0,
            });
        }
        scene.finish();
        scene
    }

    fn render(renderer: &mut MetalRenderer, scene: &Scene, upstream: bool) -> image::RgbaImage {
        UPSTREAM_LOOP.with(|flag| flag.set(upstream));
        let image = renderer
            .render_scene_to_image(scene, size(DevicePixels(SIDE), DevicePixels(SIDE)))
            .expect("a headless render");
        UPSTREAM_LOOP.with(|flag| flag.set(false));
        image
    }

    #[test]
    fn binding_only_what_changed_draws_what_upstream_draws() {
        let mut renderer = renderer();
        let scene = scene(&renderer, 30);
        assert!(
            scene.batches().count() > 60,
            "the cards draw in batches of their own"
        );
        let upstream = render(&mut renderer, &scene, true);
        let bound = render(&mut renderer, &scene, false);
        assert!(
            upstream
                .pixels()
                .any(|pixel| pixel.0 != upstream.get_pixel(0, 0).0),
            "the scene draws something"
        );
        assert!(upstream == bound, "the frames differ");
    }

    /// Instructions the calling thread has retired, as `gpui_perf` counts
    /// them.
    fn instructions() -> u64 {
        // Exported by libsystem_kernel, though not in the SDK's headers: the
        // kernel's per-thread count of the CPU's performance counters.
        unsafe extern "C" {
            fn thread_selfcounts(kind: i32, buffer: *mut u64, size: usize) -> i32;
        }
        // Kind 1 is instructions and cycles.
        let mut counts = [0u64; 2];
        // SAFETY: the buffer holds the two counters kind 1 writes.
        let status = unsafe { thread_selfcounts(1, counts.as_mut_ptr(), size_of_val(&counts)) };
        assert_eq!(status, 0, "the CPU counts instructions");
        counts[0]
    }

    /// Prints the instructions and CPU time encoding a frame takes, upstream's way and
    /// binding only what changed, over frames of few and of many batches.
    /// `cargo test -p gpui_apple --release --lib -- --ignored encoding_cost --nocapture`
    #[test]
    #[ignore = "a measurement"]
    fn encoding_cost() {
        let mut renderer = renderer();
        let viewport = size(DevicePixels(SIDE), DevicePixels(SIDE));
        renderer.update_path_intermediate_textures(viewport);
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_width(SIDE as u64);
        descriptor.set_height(SIDE as u64);
        descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        descriptor.set_usage(metal::MTLTextureUsage::RenderTarget);
        descriptor.set_storage_mode(metal::MTLStorageMode::Private);
        let target = renderer.device.new_texture(&descriptor);
        for cards in [0, 8, 60] {
            let scene = scene(&renderer, cards);
            let batches = scene.batches().count();
            let mut times = [Vec::new(), Vec::new()];
            let mut counts = [Vec::new(), Vec::new()];
            for frame in 0..3000 {
                let upstream = frame % 2 == 0;
                UPSTREAM_LOOP.with(|flag| flag.set(upstream));
                objc2::rc::autoreleasepool(|_| {
                    let start = Instant::now();
                    let retired = instructions();
                    let command_buffer = renderer.render_frame(&scene, &target, viewport).unwrap();
                    let retired = instructions() - retired;
                    let elapsed = start.elapsed();
                    command_buffer.commit();
                    command_buffer.wait_until_completed();
                    if frame >= 200 {
                        times[usize::from(!upstream)].push(elapsed.as_secs_f64() * 1e6);
                        counts[usize::from(!upstream)].push(retired as f64 / 1e3);
                    }
                });
            }
            UPSTREAM_LOOP.with(|flag| flag.set(false));
            let median = |mut values: Vec<f64>| {
                values.sort_by(f64::total_cmp);
                values[values.len() / 2]
            };
            let [upstream, bound] = times.map(median);
            let [upstream_k, bound_k] = counts.map(median);
            println!(
                "{batches:4} batches: upstream {upstream_k:8.1}k instructions {upstream:7.2} µs, \
                 bound {bound_k:8.1}k {bound:7.2} µs: {:+.1}k ({:+.2}%), {:+.2} µs",
                bound_k - upstream_k,
                (bound_k - upstream_k) / upstream_k * 100.,
                bound - upstream,
            );
        }
    }
}
