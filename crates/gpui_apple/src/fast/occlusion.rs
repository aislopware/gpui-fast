//! What an opaque native covers whole is neither drawn nor cleared again.
//!
//! A native shows through a hole: a quad drawn after everything the scene
//! orders below the native, keeping `destination × (1 − coverage × opacity)`,
//! so the drawable is cleared where the native shows. Where a hole's coverage
//! and opacity are both 1 — its interior — whatever was drawn there before it
//! is cleared to transparent. Drawing it was wasted, and so is the hole's own
//! pass over those pixels.
//!
//! So a pass starts by marking each hole's interior in a stencil attachment,
//! with the hole's index plus one (the topmost interior wins, holes being in
//! draw order), and clearing the interior's colour to transparent. Every
//! batch then draws only where no hole at or after the next one to draw has
//! its interior: a batch that comes before hole `j` passes where the stencil
//! is at most `j`. The holes themselves draw only their edges, where their
//! coverage falls off. The image is the same, pixel for pixel: an interior
//! pixel ends transparent under everything ordered below its hole, as the
//! hole leaves it, and everything ordered above draws over it as before.
//!
//! The stencil is memoryless on Apple GPUs, so it costs no memory and no
//! bandwidth; a pass restarted after a path group marks the interiors again,
//! without clearing what the pass before drew over them.

use gpui::{Bounds, Corners, Quad, ScaledPixels, Scene, point, size};
use metal::{
    DepthStencilDescriptor, DepthStencilState, DeviceRef, LibraryRef, MTLCompareFunction,
    MTLPixelFormat, MTLStencilOperation, MTLStorageMode, MTLTextureUsage, RenderCommandEncoderRef,
    RenderPipelineDescriptor, RenderPipelineState, StencilDescriptor, Texture, TextureDescriptor,
    TextureRef,
};

use crate::metal_renderer::{
    InstanceBinding, InstanceBufferWriter, MetalRenderer,
    binds::{Binds, Instanced},
};

/// The stencil attachment of every pass that draws a scene's primitives.
pub(crate) const STENCIL_FORMAT: MTLPixelFormat = MTLPixelFormat::Stencil8;

/// How many stencil textures, one per render target size, are kept: a frame
/// draws into the drawable and into layer tiles.
const STENCIL_SIZES: usize = 4;

/// The pipelines, stencil states and stencil textures of a renderer.
pub(crate) struct Occlusion {
    /// Marks an interior in the stencil and clears its colour.
    clear: RenderPipelineState,
    /// Marks an interior in the stencil only.
    mark: RenderPipelineState,
    /// Writes the reference value wherever an interior is drawn.
    marking: DepthStencilState,
    /// Passes where the stencil is at most the reference value.
    culling: DepthStencilState,
    memoryless: bool,
    stencils: Vec<Texture>,
}

impl Occlusion {
    pub(crate) fn new(
        device: &DeviceRef,
        library: &LibraryRef,
        target_format: MTLPixelFormat,
        memoryless: bool,
    ) -> Self {
        let pipeline = |label: &str, clears: bool| {
            let descriptor = RenderPipelineDescriptor::new();
            descriptor.set_label(label);
            let vertex = library
                .get_function("quad_vertex", None)
                .expect("error locating vertex function");
            descriptor.set_vertex_function(Some(&vertex));
            let color = descriptor.color_attachments().object_at(0).unwrap();
            color.set_pixel_format(target_format);
            if clears {
                let fragment = library
                    .get_function("hole_interior_fragment", None)
                    .expect("error locating fragment function");
                descriptor.set_fragment_function(Some(&fragment));
            } else {
                color.set_write_mask(metal::MTLColorWriteMask::empty());
            }
            descriptor.set_stencil_attachment_pixel_format(STENCIL_FORMAT);
            device
                .new_render_pipeline_state(&descriptor)
                .expect("could not create render pipeline state")
        };
        let state = |compare: MTLCompareFunction, pass: MTLStencilOperation| {
            let stencil = StencilDescriptor::new();
            stencil.set_stencil_compare_function(compare);
            stencil.set_depth_stencil_pass_operation(pass);
            let descriptor = DepthStencilDescriptor::new();
            descriptor.set_front_face_stencil(Some(&stencil));
            descriptor.set_back_face_stencil(Some(&stencil));
            device.new_depth_stencil_state(&descriptor)
        };
        Self {
            clear: pipeline("hole_interiors", true),
            mark: pipeline("hole_interior_marks", false),
            marking: state(MTLCompareFunction::Always, MTLStencilOperation::Replace),
            culling: state(MTLCompareFunction::GreaterEqual, MTLStencilOperation::Keep),
            // The simulator's Metal has no memoryless storage.
            memoryless: memoryless && !cfg!(target_abi = "sim"),
            stencils: Vec::new(),
        }
    }

    /// The stencil attachment for a pass drawing into `target`.
    pub(crate) fn stencil_for(&mut self, device: &DeviceRef, target: &TextureRef) -> Texture {
        let (width, height) = (target.width(), target.height());
        if let Some(index) = self
            .stencils
            .iter()
            .position(|stencil| stencil.width() == width && stencil.height() == height)
        {
            let stencil = self.stencils.remove(index);
            self.stencils.push(stencil.clone());
            return stencil;
        }
        let descriptor = TextureDescriptor::new();
        descriptor.set_width(width);
        descriptor.set_height(height);
        descriptor.set_pixel_format(STENCIL_FORMAT);
        descriptor.set_usage(MTLTextureUsage::RenderTarget);
        descriptor.set_storage_mode(if self.memoryless {
            MTLStorageMode::Memoryless
        } else {
            MTLStorageMode::Private
        });
        let stencil = device.new_texture(&descriptor);
        if self.stencils.len() == STENCIL_SIZES {
            self.stencils.remove(0);
        }
        self.stencils.push(stencil.clone());
        stencil
    }
}

/// Gives a pass `stencil`, cleared to 0 and dropped at its end.
pub(crate) fn attach_stencil(descriptor: &metal::RenderPassDescriptorRef, stencil: &TextureRef) {
    let attachment = descriptor.stencil_attachment().unwrap();
    attachment.set_texture(Some(stencil));
    attachment.set_load_action(metal::MTLLoadAction::Clear);
    attachment.set_clear_stencil(0);
    attachment.set_store_action(metal::MTLStoreAction::DontCare);
}

/// The interior of every hole a scene cuts, as quads the quad shader draws:
/// what of the hole its coverage and opacity are 1 in. A hole without one is
/// an empty quad.
pub(crate) struct Interiors {
    quads: InstanceBinding,
    count: usize,
}

impl Interiors {
    /// The interiors of `scene`'s holes, if any hole has one.
    pub(crate) fn of(
        scene: &Scene,
        writer: &mut InstanceBufferWriter,
    ) -> anyhow::Result<Option<Self>> {
        let natives = scene.natives();
        let holes = &natives.holes;
        // The stencil holds a hole's index plus one.
        if holes.is_empty() || holes.len() >= usize::from(u8::MAX) {
            return Ok(None);
        }
        // Both lists are in draw order, a placement beside its hole.
        let interiors: Vec<Quad> = natives
            .placements
            .iter()
            .zip(holes)
            .map(|(placement, hole)| interior(hole, placement.opacity >= 1.))
            .collect();
        if interiors.iter().all(|quad| quad.bounds.is_empty()) {
            return Ok(None);
        }
        Ok(Some(Self {
            quads: writer.write(&interiors)?,
            count: interiors.len(),
        }))
    }

    /// Starts a pass: marks every interior, and clears their colour when
    /// `clear` is set (the pass's first). Leaves the pass culling for the
    /// first hole.
    pub(crate) fn begin_pass(
        &self,
        renderer: &MetalRenderer,
        viewport_size: gpui::Size<gpui::DevicePixels>,
        encoder: &RenderCommandEncoderRef,
        binds: &mut Binds,
        clear: bool,
    ) {
        let occlusion = &renderer.fast_occlusion;
        encoder.set_depth_stencil_state(&occlusion.marking);
        for index in 0..self.count {
            encoder.set_stencil_reference_value(index as u32 + 1);
            renderer.draw_instanced(
                Instanced {
                    pipeline: if clear {
                        &occlusion.clear
                    } else {
                        &occlusion.mark
                    },
                    instances: &self.quads,
                    fragment_reads_instances: false,
                    fades: None,
                    atlas: None,
                    range: index..index + 1,
                },
                viewport_size,
                encoder,
                binds,
            );
        }
        encoder.set_depth_stencil_state(&occlusion.culling);
        encoder.set_stencil_reference_value(0);
    }

    /// Culls the batches drawn before hole `next_hole`, or the holes from it
    /// on, drawing only where no hole from it on has its interior.
    pub(crate) fn before(encoder: &RenderCommandEncoderRef, next_hole: usize) {
        encoder.set_stencil_reference_value(next_hole as u32);
    }
}

/// What of `hole` its coverage and opacity are both 1 in, at whole pixels:
/// its clipped bounds, less its corners' radius, where the quad shader
/// returns the background unchanged. A pixel is drawn when its centre is
/// inside, as for the hole.
fn interior(hole: &Quad, opaque: bool) -> Quad {
    let radius = max_radius(&hole.corner_radii);
    let inset = if radius > 0. { radius.max(1.) } else { 0. };
    let clipped = hole.bounds.intersect(&hole.content_mask.bounds);
    let (left, top) = (clipped.origin.x.0 + inset, clipped.origin.y.0 + inset);
    let (right, bottom) = (
        clipped.origin.x.0 + clipped.size.width.0 - inset,
        clipped.origin.y.0 + clipped.size.height.0 - inset,
    );
    let bounds = if !opaque || right <= left || bottom <= top {
        Bounds::default()
    } else if inset == 0. {
        // Unrounded, the hole covers every pixel it rasterizes whole.
        clipped
    } else {
        Bounds::new(
            point(ScaledPixels(left), ScaledPixels(top)),
            size(ScaledPixels(right - left), ScaledPixels(bottom - top)),
        )
    };
    Quad {
        bounds,
        content_mask: hole.content_mask,
        corner_radii: Corners::default(),
        ..*hole
    }
}

fn max_radius(radii: &Corners<ScaledPixels>) -> f32 {
    radii
        .top_left
        .0
        .max(radii.top_right.0)
        .max(radii.bottom_left.0)
        .max(radii.bottom_right.0)
}
