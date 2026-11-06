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

use anyhow::{Context as _, Result};
use gpui::{Bounds, Corners, Quad, ScaledPixels, Scene, point, size};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLColorWriteMask, MTLCompareFunction, MTLDepthStencilDescriptor, MTLDepthStencilState,
    MTLDevice, MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLRenderCommandEncoder,
    MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState,
    MTLStencilDescriptor, MTLStencilOperation, MTLStorageMode, MTLStoreAction, MTLTexture,
    MTLTextureDescriptor, MTLTextureUsage,
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
    clear: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// Marks an interior in the stencil only.
    mark: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// Writes the reference value wherever an interior is drawn.
    marking: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    /// Passes where the stencil is at most the reference value.
    culling: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    memoryless: bool,
    stencils: Vec<Retained<ProtocolObject<dyn MTLTexture>>>,
}

impl Occlusion {
    pub(crate) fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        library: &ProtocolObject<dyn MTLLibrary>,
        target_format: MTLPixelFormat,
        memoryless: bool,
    ) -> Result<Self> {
        let function = |name: &str| {
            library
                .newFunctionWithName(&NSString::from_str(name))
                .with_context(|| format!("locating function {name}"))
        };
        let pipeline = |label: &str, clears: bool| -> Result<_> {
            let descriptor = MTLRenderPipelineDescriptor::new();
            descriptor.setLabel(Some(&NSString::from_str(label)));
            descriptor.setVertexFunction(Some(&*function("quad_vertex")?));
            // SAFETY: Index zero is within the pipeline's color attachment array.
            let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
            color.setPixelFormat(target_format);
            if clears {
                descriptor.setFragmentFunction(Some(&*function("hole_interior_fragment")?));
            } else {
                color.setWriteMask(MTLColorWriteMask::empty());
            }
            descriptor.setStencilAttachmentPixelFormat(STENCIL_FORMAT);
            device
                .newRenderPipelineStateWithDescriptor_error(&descriptor)
                .with_context(|| format!("creating {label} render pipeline"))
        };
        let state = |compare: MTLCompareFunction, pass: MTLStencilOperation| {
            let stencil = MTLStencilDescriptor::new();
            stencil.setStencilCompareFunction(compare);
            stencil.setDepthStencilPassOperation(pass);
            let descriptor = MTLDepthStencilDescriptor::new();
            descriptor.setFrontFaceStencil(Some(&stencil));
            descriptor.setBackFaceStencil(Some(&stencil));
            device
                .newDepthStencilStateWithDescriptor(&descriptor)
                .context("creating a stencil state")
        };
        Ok(Self {
            clear: pipeline("hole_interiors", true)?,
            mark: pipeline("hole_interior_marks", false)?,
            marking: state(MTLCompareFunction::Always, MTLStencilOperation::Replace)?,
            culling: state(MTLCompareFunction::GreaterEqual, MTLStencilOperation::Keep)?,
            // The simulator's Metal has no memoryless storage.
            memoryless: memoryless && !cfg!(target_abi = "sim"),
            stencils: Vec::new(),
        })
    }

    /// The stencil attachment for a pass drawing into `target`.
    pub(crate) fn stencil_for(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        target: &ProtocolObject<dyn MTLTexture>,
    ) -> Result<Retained<ProtocolObject<dyn MTLTexture>>> {
        let (width, height) = (target.width(), target.height());
        if let Some(index) = self
            .stencils
            .iter()
            .position(|stencil| stencil.width() == width && stencil.height() == height)
        {
            let stencil = self.stencils.remove(index);
            self.stencils.push(stencil.clone());
            return Ok(stencil);
        }
        let descriptor = MTLTextureDescriptor::new();
        // SAFETY: the target's own width and height, which the device already allowed.
        unsafe {
            descriptor.setWidth(width);
            descriptor.setHeight(height);
        }
        descriptor.setPixelFormat(STENCIL_FORMAT);
        descriptor.setUsage(MTLTextureUsage::RenderTarget);
        descriptor.setStorageMode(if self.memoryless {
            MTLStorageMode::Memoryless
        } else {
            MTLStorageMode::Private
        });
        let stencil = device
            .newTextureWithDescriptor(&descriptor)
            .with_context(|| format!("allocating a {width}x{height} stencil"))?;
        if self.stencils.len() == STENCIL_SIZES {
            self.stencils.remove(0);
        }
        self.stencils.push(stencil.clone());
        Ok(stencil)
    }
}

/// Gives a pass `stencil`, cleared to 0 and dropped at its end.
pub(crate) fn attach_stencil(
    descriptor: &MTLRenderPassDescriptor,
    stencil: &ProtocolObject<dyn MTLTexture>,
) {
    let attachment = descriptor.stencilAttachment();
    attachment.setTexture(Some(stencil));
    attachment.setLoadAction(MTLLoadAction::Clear);
    attachment.setClearStencil(0);
    attachment.setStoreAction(MTLStoreAction::DontCare);
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
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        binds: &mut Binds,
        clear: bool,
    ) {
        let occlusion = &renderer.fast_occlusion;
        encoder.setDepthStencilState(Some(&occlusion.marking));
        for index in 0..self.count {
            encoder.setStencilReferenceValue(index as u32 + 1);
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
        encoder.setDepthStencilState(Some(&occlusion.culling));
        encoder.setStencilReferenceValue(0);
    }

    /// Culls the batches drawn before hole `next_hole`, or the holes from it
    /// on, drawing only where no hole from it on has its interior.
    pub(crate) fn before(encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>, next_hole: usize) {
        encoder.setStencilReferenceValue(next_hole as u32);
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
