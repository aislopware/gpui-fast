//! Layer masks on Metal: gpui's `fast::layer_mask`, monocode's project
//! background (its `begin_edge_fade` and `composite_background`, and the
//! shader `project_background_fragment`, ported as `layer_mask_fragment`).
//!
//! A [`ComposedBatch::Masks`] ends the pass drawing the frame once for all
//! its cuts, whose regions are apart. A region is the cut's bounds inside
//! its content mask and the target, in whole pixels inward, as monocode's.
//! For a begin it copies what was drawn in the region into the mask's
//! `before` texture, and once the pass is begun again clears the region to
//! transparent. For an end it copies the group, drawn over the cleared
//! region, into `artwork`; with Haze it blurs that twice with
//! `MPSImageGaussianBlur` (clamped edges, monocode's soft and strong
//! standard deviations in CSS pixels times the scale) into `soft` and
//! `strong`; and once the pass is begun again draws the region with
//! monocode's fragment, which writes `layer + before × (1 − layer.a)`, the
//! masked group over what was under it, outside the rounded corners
//! `before` alone. Both draws replace the region's pixels, blending off.
//!
//! Each open mask holds one depth's textures, sized exactly to its region
//! as monocode's are; a mask begun while another is open takes the next
//! depth. They go after 120 frames without a mask. A frame with a mask
//! needs a readable drawable, as one with a backdrop blur does
//! (`fast::backdrop::prepare_layer`); when it has none, the mask is not
//! drawn and the group shows as painted.
//!
//! [`ComposedBatch::Masks`]: gpui::composition::ComposedBatch::Masks

use std::{mem, ops::Range, ptr};

use anyhow::{Context as _, Result};
use gpui::{LayerMaskCut, Scene};
use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlitCommandEncoder as _, MTLCommandBuffer, MTLCommandEncoder as _, MTLDevice, MTLLibrary,
    MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLSize, MTLStorageMode, MTLTexture,
    MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};

use crate::metal_renderer::{MetalRenderer, new_command_encoder_for_texture};

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Encoder = Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>;

/// Frames without a mask after which the textures go.
const RELEASE_AFTER_FRAMES: u32 = 120;

/// The fragment's parameters (`LayerMaskParams` in `shaders.metal`,
/// monocode's `BackgroundCompositeParams`).
#[repr(C)]
struct Params {
    /// The region drawn, in whole pixels.
    origin: [f32; 2],
    size: [f32; 2],
    /// The cut's whole bounds, which the stops and corners are laid on.
    region_origin: [f32; 2],
    region_size: [f32; 2],
    viewport_size: [f32; 2],
    /// Top left, top right, bottom right, bottom left.
    radii: [f32; 4],
    /// monocode's mode; 0 clears.
    mode: f32,
    opacity: f32,
    scale: f32,
    padding: f32,
    /// A group fade's edge widths and depths, top, right, bottom, left, and
    /// its raw ramp along x, in device pixels from the region's top left.
    fade_width: [f32; 4],
    fade_depth: [f32; 4],
    ramp_x: [f32; 2],
    fade_padding: [f32; 2],
}

/// One depth's textures: the region before the group, the group, and the
/// group's two Haze blurs once a mask with Haze ends at this depth.
struct Depth {
    before: Texture,
    artwork: Texture,
    haze: Option<(Texture, Texture)>,
}

/// A mask begun this frame and not yet ended.
#[derive(Clone, Copy)]
struct Open {
    id: u32,
    depth: usize,
    /// `[left, top, right, bottom]` in the target, in whole pixels.
    region: [usize; 4],
}

/// What a renderer keeps for layer masks.
pub(crate) struct Masks {
    /// `layer_mask_vertex` and `layer_mask_fragment`, blending off.
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    depths: Vec<Depth>,
    open: Vec<Open>,
    mask_free_frames: u32,
}

impl Masks {
    pub(crate) fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        library: &ProtocolObject<dyn MTLLibrary>,
        format: MTLPixelFormat,
    ) -> Result<Self> {
        let function = |name: &str| {
            library
                .newFunctionWithName(&NSString::from_str(name))
                .with_context(|| format!("locating function {name}"))
        };
        let descriptor = MTLRenderPipelineDescriptor::new();
        descriptor.setLabel(Some(&NSString::from_str("layer_mask")));
        descriptor.setVertexFunction(Some(&*function("layer_mask_vertex")?));
        descriptor.setFragmentFunction(Some(&*function("layer_mask_fragment")?));
        // SAFETY: Index zero is within the pipeline's color attachment array.
        let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        color.setPixelFormat(format);
        color.setBlendingEnabled(false);
        descriptor.setStencilAttachmentPixelFormat(super::occlusion::STENCIL_FORMAT);
        let pipeline = device
            .newRenderPipelineStateWithDescriptor_error(&descriptor)
            .context("creating layer_mask render pipeline")?;
        Ok(Self {
            pipeline,
            depths: Vec::new(),
            open: Vec::new(),
            mask_free_frames: 0,
        })
    }

    /// The bytes its textures take on the GPU, for `fast::gpu_stats`.
    pub(crate) fn resident_bytes(&self) -> usize {
        self.depths
            .iter()
            .flat_map(|depth| {
                let haze = depth.haze.as_ref();
                [Some(&depth.before), Some(&depth.artwork)]
                    .into_iter()
                    .chain([haze.map(|haze| &haze.0), haze.map(|haze| &haze.1)])
                    .flatten()
            })
            .map(|texture| super::gpu_stats::allocated(texture))
            .sum()
    }

    /// Begins a frame drawing `scene`: no mask is open, and the textures
    /// go after a run of frames without one.
    pub(crate) fn begin_frame(&mut self, scene: &Scene) {
        self.open.clear();
        if !scene.layer_masks().is_empty() {
            self.mask_free_frames = 0;
            return;
        }
        self.mask_free_frames = self.mask_free_frames.saturating_add(1);
        if self.mask_free_frames == RELEASE_AFTER_FRAMES {
            self.depths.clear();
        }
    }

    /// Makes `depth`'s `before` and `artwork` the region's size in `format`.
    fn reserve(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        depth: usize,
        (width, height): (usize, usize),
        format: MTLPixelFormat,
    ) -> Result<()> {
        let fits = |texture: &Texture| {
            texture.width() == width
                && texture.height() == height
                && texture.pixelFormat() == format
        };
        if self
            .depths
            .get(depth)
            .is_some_and(|kept| fits(&kept.before))
        {
            return Ok(());
        }
        let read = MTLTextureUsage::ShaderRead;
        let made = Depth {
            before: texture(device, (width, height), format, read)?,
            artwork: texture(device, (width, height), format, read)?,
            haze: None,
        };
        // A mask begun inside another takes the next depth: the smallest no
        // open mask holds, so at most one past those kept.
        if depth == self.depths.len() {
            self.depths.push(made);
        } else {
            self.depths[depth] = made;
        }
        Ok(())
    }

    /// Gives `depth` Haze's two blur textures, the size of its `artwork`.
    fn reserve_haze(&mut self, device: &ProtocolObject<dyn MTLDevice>, depth: usize) -> Result<()> {
        let kept = &mut self.depths[depth];
        let artwork = &kept.artwork;
        let fits = kept.haze.as_ref().is_some_and(|(soft, _)| {
            soft.width() == artwork.width()
                && soft.height() == artwork.height()
                && soft.pixelFormat() == artwork.pixelFormat()
        });
        if !fits {
            let size = (artwork.width(), artwork.height());
            let format = artwork.pixelFormat();
            let usage = MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite;
            kept.haze = Some((
                texture(device, size, format, usage)?,
                texture(device, size, format, usage)?,
            ));
        }
        Ok(())
    }
}

fn texture(
    device: &ProtocolObject<dyn MTLDevice>,
    (width, height): (usize, usize),
    format: MTLPixelFormat,
    usage: MTLTextureUsage,
) -> Result<Texture> {
    let descriptor = MTLTextureDescriptor::new();
    descriptor.setTextureType(MTLTextureType::Type2D);
    descriptor.setPixelFormat(format);
    // SAFETY: at most the target's own width and height, which the device allowed.
    unsafe {
        descriptor.setWidth(width);
        descriptor.setHeight(height);
    }
    descriptor.setStorageMode(MTLStorageMode::Private);
    descriptor.setUsage(usage);
    device
        .newTextureWithDescriptor(&descriptor)
        .context("allocating a layer mask texture")
}

/// The region `cut` draws in a target of `width` × `height`: its bounds
/// inside its content mask and the target, in whole pixels inward, as
/// `[left, top, right, bottom]`; `None` when that is empty.
fn region(cut: &LayerMaskCut, width: usize, height: usize) -> Option<[usize; 4]> {
    let bounds = cut.clipped_bounds();
    let left = bounds.origin.x.0.max(0.).ceil();
    let top = bounds.origin.y.0.max(0.).ceil();
    let right = (bounds.origin.x.0 + bounds.size.width.0)
        .min(width as f32)
        .floor();
    let bottom = (bounds.origin.y.0 + bounds.size.height.0)
        .min(height as f32)
        .floor();
    (left < right && top < bottom)
        .then(|| [left as usize, top as usize, right as usize, bottom as usize])
}

/// A cut to draw in this batch, at its mask's depth and region.
struct Work {
    cut: LayerMaskCut,
    depth: usize,
    region: [usize; 4],
    /// For an end with Haze: its blurs' kernels, if Metal Performance
    /// Shaders run.
    kernels: Option<(Retained<AnyObject>, Retained<AnyObject>)>,
}

/// Draws the cuts `range` of `scene`, of one draw order, over what
/// `encoder`'s pass drew so far: ends the pass once, copies each region
/// (and blurs each Haze group), and leaves in `encoder` the pass begun
/// again with each begin's region cleared and each end's group drawn back
/// through its mask. Returns whether it did: it leaves `encoder` as it is
/// when there is nothing to draw or the target can't be read. On an error
/// the pass is ended.
pub(crate) fn draw(
    renderer: &mut MetalRenderer,
    scene: &Scene,
    range: Range<usize>,
    pass: &super::backdrop::Pass<'_>,
    encoder: &mut Encoder,
) -> Result<bool> {
    // A display link hands out its drawable before the frame is drawn, so the first frame with
    // a mask after the layer went write-only has one it cannot read; the next is readable.
    if pass.texture.isFramebufferOnly() {
        return Ok(false);
    }
    let (target_width, target_height) = (pass.texture.width(), pass.texture.height());
    let format = pass.texture.pixelFormat();
    let device = renderer.device.clone();
    let mut work = Vec::new();
    // The masks this batch ends: their depths stay taken until it is drawn.
    let mut ended = Vec::new();
    let planned = (|| -> Result<()> {
        for cut in &scene.layer_masks()[range] {
            let masks = &mut renderer.fast_backdrop.masks;
            if cut.begin {
                let Some(region) = region(cut, target_width, target_height) else {
                    continue;
                };
                let depth = (0..)
                    .find(|depth| !masks.open.iter().any(|open| open.depth == *depth))
                    .unwrap_or_default();
                let size = (region[2] - region[0], region[3] - region[1]);
                masks.reserve(&device, depth, size, format)?;
                masks.open.push(Open {
                    id: cut.id,
                    depth,
                    region,
                });
                work.push(Work {
                    cut: *cut,
                    depth,
                    region,
                    kernels: None,
                });
                continue;
            }
            let Some(open) = masks.open.iter().find(|open| open.id == cut.id).copied() else {
                continue;
            };
            ended.push(cut.id);
            let mut kernels = None;
            if let Some((soft, strong)) = cut.mask.haze_sigmas() {
                masks.reserve_haze(&device, open.depth)?;
                let backdrop = &mut renderer.fast_backdrop;
                if backdrop.supported {
                    // monocode's scale, from its marker's one-pixel border, is at least 1.
                    let scale = cut.scale.max(1.);
                    let soft = backdrop.kernel(&device, soft * scale);
                    let strong = backdrop.kernel(&device, strong * scale);
                    kernels = soft.zip(strong);
                }
            }
            work.push(Work {
                cut: *cut,
                depth: open.depth,
                region: open.region,
                kernels,
            });
        }
        Ok(())
    })();
    if let Err(error) = planned {
        encoder.endEncoding();
        return Err(error);
    }
    if work.is_empty() {
        return Ok(false);
    }

    encoder.endEncoding();
    let masks = &renderer.fast_backdrop.masks;
    let blit = pass
        .command_buffer
        .blitCommandEncoder()
        .context("creating a layer mask blit encoder")?;
    for item in &work {
        let depth = &masks.depths[item.depth];
        let copy = if item.cut.begin {
            &depth.before
        } else {
            &depth.artwork
        };
        let [left, top, right, bottom] = item.region;
        // SAFETY: the region lies inside the target and is the copy's size; both share a
        // pixel format and one sample, and the blit is ended before the next encoder begins.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                pass.texture,
                0,
                0,
                MTLOrigin {
                    x: left,
                    y: top,
                    z: 0,
                },
                MTLSize {
                    width: right - left,
                    height: bottom - top,
                    depth: 1,
                },
                copy,
                0,
                0,
                MTLOrigin { x: 0, y: 0, z: 0 },
            );
        }
    }
    blit.endEncoding();
    for item in &work {
        let (Some((soft_kernel, strong_kernel)), Some((soft, strong))) =
            (&item.kernels, &masks.depths[item.depth].haze)
        else {
            continue;
        };
        let artwork = &masks.depths[item.depth].artwork;
        for (kernel, destination) in [(soft_kernel, soft), (strong_kernel, strong)] {
            // SAFETY: a live kernel, command buffer and two textures of one size and format,
            // the destination writable.
            unsafe {
                let _: () = msg_send![
                    &**kernel,
                    encodeToCommandBuffer: pass.command_buffer,
                    sourceTexture: &**artwork,
                    destinationTexture: &**destination
                ];
            }
        }
    }
    *encoder = new_command_encoder_for_texture(
        pass.command_buffer,
        pass.texture,
        pass.stencil,
        pass.viewport_size,
        None,
    )?;
    encoder.setRenderPipelineState(&masks.pipeline);
    let viewport = [
        i32::from(pass.viewport_size.width) as f32,
        i32::from(pass.viewport_size.height) as f32,
    ];
    for item in &work {
        let cut = &item.cut;
        // A group fade's begin only copies what is under the group.
        if cut.begin && !cut.mask.clears() {
            continue;
        }
        let [left, top, right, bottom] = item.region;
        let radii = cut.corner_radii;
        let fade = cut.mask.fade.unwrap_or_default();
        let edges = |edges: gpui::Edges<f32>| [edges.top, edges.right, edges.bottom, edges.left];
        let params = Params {
            origin: [left as f32, top as f32],
            size: [(right - left) as f32, (bottom - top) as f32],
            region_origin: [cut.bounds.origin.x.0, cut.bounds.origin.y.0],
            region_size: [cut.bounds.size.width.0, cut.bounds.size.height.0],
            viewport_size: viewport,
            radii: [
                radii.top_left.0,
                radii.top_right.0,
                radii.bottom_right.0,
                radii.bottom_left.0,
            ],
            mode: if cut.begin {
                0.
            } else {
                cut.mask.mode() as f32
            },
            opacity: cut.mask.opacity,
            scale: cut.scale.max(1.),
            padding: 0.,
            fade_width: edges(fade.width),
            fade_depth: edges(fade.depth),
            ramp_x: fade.ramp_x.map_or([0., 0.], |(start, end)| [start, end]),
            fade_padding: [0., 0.],
        };
        let depth = &masks.depths[item.depth];
        // Without Haze, or without its blurs, the fragment reads the group for them.
        let (soft, strong) = match (&item.kernels, &depth.haze) {
            (Some(_), Some((soft, strong))) => (soft, strong),
            _ => (&depth.artwork, &depth.artwork),
        };
        // SAFETY: the parameters match `LayerMaskParams` in `shaders.metal` and Metal copies
        // them; the textures are live and Metal retains them.
        unsafe {
            encoder.setVertexBytes_length_atIndex(
                ptr::NonNull::from(&params).cast(),
                mem::size_of_val(&params),
                0,
            );
            encoder.setFragmentBytes_length_atIndex(
                ptr::NonNull::from(&params).cast(),
                mem::size_of_val(&params),
                0,
            );
            encoder.setFragmentTexture_atIndex(Some(&depth.before), 0);
            encoder.setFragmentTexture_atIndex(Some(&depth.artwork), 1);
            encoder.setFragmentTexture_atIndex(Some(soft), 2);
            encoder.setFragmentTexture_atIndex(Some(strong), 3);
            encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 6);
        }
    }
    let masks = &mut renderer.fast_backdrop.masks;
    masks.open.retain(|open| !ended.contains(&open.id));
    Ok(true)
}

/// Pixel tests: a headless renderer draws scenes with masks, read back and
/// checked against monocode's fragment, written out here in Rust with its
/// stop tables and constants.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{
        Bounds, ContentMask, Corners, DevicePixels, Hsla, LayerMask, LayerMaskCut, Quad,
        ScaledPixels, Scene, point, rgba, size,
    };
    use image::RgbaImage;
    use parking_lot::Mutex;

    use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

    const W: i32 = 200;
    const H: i32 = 240;

    fn sp(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(width), ScaledPixels(height)),
        }
    }

    fn everywhere() -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: sp(0., 0., W as f32, H as f32),
        }
    }

    fn quad(bounds: Bounds<ScaledPixels>, color: u32) -> Quad {
        Quad {
            bounds,
            content_mask: everywhere(),
            background: Hsla::from(rgba(color)).into(),
            ..Default::default()
        }
    }

    fn cut(begin: bool, bounds: Bounds<ScaledPixels>, mask: LayerMask, scale: f32) -> LayerMaskCut {
        LayerMaskCut {
            order: 0,
            id: 0,
            begin,
            bounds,
            content_mask: everywhere(),
            corner_radii: Corners {
                top_left: ScaledPixels(22.),
                top_right: ScaledPixels(0.),
                bottom_right: ScaledPixels(14.),
                bottom_left: ScaledPixels(6.),
            },
            mask,
            scale,
            quads_before: 0,
        }
    }

    fn renderer() -> Option<MetalRenderer> {
        if objc2_metal::MTLCreateSystemDefaultDevice().is_none() {
            eprintln!("skipped: no Metal device");
            return None;
        }
        Some(
            MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())))
                .unwrap(),
        )
    }

    fn render(renderer: &mut MetalRenderer, scene: &mut Scene) -> RgbaImage {
        scene.finish();
        renderer
            .render_scene_to_image(scene, size(DevicePixels(W), DevicePixels(H)))
            .expect("scene rendered")
    }

    fn mix(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t
    }

    fn saturate(x: f32) -> f32 {
        x.clamp(0., 1.)
    }

    /// monocode's `project_background_mask`.
    fn stops(y: f32, empty: bool, haze: bool) -> f32 {
        if !empty {
            if haze {
                return 1. - saturate((y - 0.18) / 0.58);
            }
            if y <= 0.18 {
                return 1.;
            }
            if y <= 0.32 {
                return mix(1., 0.61, (y - 0.18) / 0.14);
            }
            if y <= 0.47 {
                return mix(0.61, 0.32, (y - 0.32) / 0.15);
            }
            if y <= 0.62 {
                return mix(0.32, 0.11, (y - 0.47) / 0.15);
            }
            return mix(0.11, 0., saturate((y - 0.62) / 0.10));
        }
        if y <= 0.19 {
            return 1.;
        }
        if haze {
            if y <= 0.30 {
                return mix(1., 0.94, (y - 0.19) / 0.11);
            }
            if y <= 0.42 {
                return mix(0.94, 0.72, (y - 0.30) / 0.12);
            }
            if y <= 0.54 {
                return mix(0.72, 0.42, (y - 0.42) / 0.12);
            }
            if y <= 0.66 {
                return mix(0.42, 0.16, (y - 0.54) / 0.12);
            }
            if y <= 0.78 {
                return mix(0.16, 0.04, (y - 0.66) / 0.12);
            }
            return mix(0.04, 0., saturate((y - 0.78) / 0.12));
        }
        if y <= 0.30 {
            return mix(1., 0.79, (y - 0.19) / 0.11);
        }
        if y <= 0.42 {
            return mix(0.79, 0.50, (y - 0.30) / 0.12);
        }
        if y <= 0.54 {
            return mix(0.50, 0.20, (y - 0.42) / 0.12);
        }
        if y <= 0.64 {
            return mix(0.20, 0.05, (y - 0.54) / 0.10);
        }
        mix(0.05, 0., saturate((y - 0.64) / 0.08))
    }

    /// monocode's `project_background_fragment` at the pixel centred on
    /// `position`, for a group of one flat colour `art` (which its blurs
    /// leave as it is) over `before`, premultiplied, in the mask `bounds`.
    #[allow(clippy::too_many_arguments)]
    fn monocode(
        position: (f32, f32),
        bounds: Bounds<ScaledPixels>,
        radii: [f32; 4],
        mode: u32,
        opacity: f32,
        scale: f32,
        before: [f32; 4],
        art: [f32; 4],
    ) -> [f32; 4] {
        let local = (
            position.0 - bounds.origin.x.0,
            position.1 - bounds.origin.y.0,
        );
        let region = (bounds.size.width.0, bounds.size.height.0);
        let y = local.1 / region.1;
        let haze = mode >= 3;
        let empty = mode != 2 && mode != 4;
        let mut layer = art;
        let over = |top: [f32; 4], under: [f32; 4]| -> [f32; 4] {
            std::array::from_fn(|i| top[i] + under[i] * (1. - top[3]))
        };
        if haze {
            let soft_mask = saturate((y - 0.18) / if empty { 0.42 } else { 0.52 });
            let strong_mask =
                saturate((y - if empty { 0.28 } else { 0.22 }) / if empty { 0.48 } else { 0.54 });
            layer = over(art.map(|c| c * soft_mask), layer);
            layer = over(art.map(|c| c * strong_mask), layer);
            let mut darken = if !empty {
                saturate((y - 0.15) / 0.61) * 0.70
            } else if y <= 0.38 {
                saturate((y - 0.16) / 0.22) * 0.26
            } else if y <= 0.55 {
                mix(0.26, 0.55, (y - 0.38) / 0.17)
            } else {
                mix(0.55, 0.78, saturate((y - 0.55) / 0.20))
            };
            if (local.1 / scale) % 3. < 1. {
                darken = 1. - (1. - darken) * 0.64;
            }
            let shade = 23. / 255. * darken;
            layer = over([shade, shade, shade, darken], layer);
        }
        let half = (region.0 * 0.5, region.1 * 0.5);
        let corner = match (local.1 < half.1, local.0 < half.0) {
            (true, true) => 0,
            (true, false) => 1,
            (false, false) => 2,
            (false, true) => 3,
        };
        let radius = radii[corner];
        let c = (
            (local.0 - half.0).abs() - half.0 + radius,
            (local.1 - half.1).abs() - half.1 + radius,
        );
        let distance =
            (c.0.max(0.).powi(2) + c.1.max(0.).powi(2)).sqrt() + c.0.max(c.1).min(0.) - radius;
        let k = stops(y, empty, haze) * opacity * saturate(0.5 - distance);
        over(layer.map(|c| c * k), before)
    }

    /// The colour `color` draws as, premultiplied, read back.
    fn drawn(renderer: &mut MetalRenderer, color: u32) -> [f32; 4] {
        let mut scene = Scene::default();
        scene.insert_primitive(quad(sp(0., 0., W as f32, H as f32), color));
        let image = render(renderer, &mut scene);
        image.get_pixel(0, 0).0.map(|c| f32::from(c) / 255.)
    }

    /// Draws a page, a masked group of one colour over the card `bounds`,
    /// and checks every pixel of the card against monocode's fragment over
    /// the colours read back, and every pixel outside it against the page;
    /// returns the worst difference inside, in 255ths.
    fn check(mask: LayerMask, scale: f32) -> i32 {
        let Some(mut renderer) = renderer() else {
            return 0;
        };
        let (page, group) = (0x336699ffu32, 0xe08020ffu32);
        let before = drawn(&mut renderer, page);
        let art = drawn(&mut renderer, group);
        let bounds = sp(20., 16., 160., 208.);
        let mut scene = Scene::default();
        scene.insert_primitive(quad(sp(0., 0., W as f32, H as f32), page));
        scene.insert_layer_mask(cut(true, bounds, mask, scale));
        scene.insert_primitive(quad(bounds, group));
        scene.insert_layer_mask(cut(false, bounds, mask, scale));
        let image = render(&mut renderer, &mut scene);
        let radii = [22., 0., 14., 6.];
        let mut worst = 0;
        for (x, y, pixel) in image.enumerate_pixels() {
            let centre = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = bounds.contains(&point(ScaledPixels(centre.0), ScaledPixels(centre.1)));
            let expected = if inside {
                monocode(
                    centre,
                    bounds,
                    radii,
                    mask.mode(),
                    mask.opacity,
                    scale,
                    before,
                    art,
                )
            } else {
                before
            };
            for channel in 0..4 {
                let want = (expected[channel] * 255.).round() as i32;
                let got = i32::from(pixel[channel]);
                let difference = (got - want).abs();
                if inside {
                    worst = worst.max(difference);
                } else {
                    assert_eq!(difference, 0, "({x}, {y}) outside the card: {pixel:?}");
                }
            }
        }
        worst
    }

    /// Without Haze, a session's and an empty project's stops, at an
    /// opacity, with four radii.
    #[test]
    fn a_mask_draws_monocodes_stops_and_corners() {
        for (empty, opacity) in [(false, 1.), (true, 0.6)] {
            let worst = check(
                LayerMask::project_background(empty, false, false, opacity),
                1.,
            );
            eprintln!("stops, empty {empty}: within {worst} of 255 levels");
            assert!(worst <= 2, "empty {empty}: {worst}");
        }
    }

    /// With Haze, its blurs (which leave a flat group as it is), its
    /// darkening and its line one CSS pixel in three, at scales 1 and 2.
    #[test]
    fn haze_draws_monocodes_darkening_and_line() {
        for (empty, preview, scale) in [(false, false, 2.), (true, false, 1.), (true, true, 2.)] {
            let mask = LayerMask::project_background(empty, true, preview, 0.9);
            let worst = check(mask, scale);
            eprintln!("haze, empty {empty}, preview {preview}, scale {scale}: within {worst}");
            assert!(worst <= 2, "empty {empty}, preview {preview}: {worst}");
        }
    }

    /// Haze blurs the group: a sharp edge in it is a step near the top of
    /// the card, before the blurs fade in, and lower down white bleeds
    /// across it into the black.
    #[test]
    fn haze_blurs_the_group_toward_the_bottom() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let bounds = sp(20., 16., 160., 208.);
        let mut scene = Scene::default();
        scene.insert_primitive(quad(sp(0., 0., W as f32, H as f32), 0x000000ff));
        let mask = LayerMask::project_background(false, true, false, 1.);
        scene.insert_layer_mask(cut(true, bounds, mask, 1.));
        scene.insert_primitive(quad(sp(20., 16., 80., 208.), 0xffffffff));
        scene.insert_primitive(quad(sp(100., 16., 80., 208.), 0x000000ff));
        scene.insert_layer_mask(cut(false, bounds, mask, 1.));
        let image = render(&mut renderer, &mut scene);
        assert!(image.get_pixel(99, 30)[0] > 200 && image.get_pixel(100, 30)[0] < 10);
        // A row 60% down the card, off the darker line.
        let y = 16 + 124;
        let (near, far) = (image.get_pixel(102, y)[0], image.get_pixel(150, y)[0]);
        assert!(
            near > far + 4,
            "the edge is blurred: {near} beside it, {far} away"
        );
    }

    /// A mask nested in another, and two masks apart in one batch, each
    /// draw back their own group over their own backdrop.
    #[test]
    fn nested_masks_and_masks_apart_draw_their_own_groups() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let plain = LayerMask::project_background(false, false, false, 1.);
        let (left, right) = (sp(10., 10., 80., 60.), sp(110., 10., 80., 60.));
        let outer = sp(10., 100., 180., 120.);
        let inner = sp(40., 110., 100., 40.);
        let mut scene = Scene::default();
        scene.insert_primitive(quad(sp(0., 0., W as f32, H as f32), 0x000000ff));
        for (bounds, color) in [(left, 0xff0000ffu32), (right, 0x00ff00ff)] {
            scene.insert_layer_mask(cut(true, bounds, plain, 1.));
            scene.insert_primitive(quad(bounds, color));
            scene.insert_layer_mask(cut(false, bounds, plain, 1.));
        }
        scene.insert_layer_mask(cut(true, outer, plain, 1.));
        scene.insert_primitive(quad(outer, 0x0000ffff));
        scene.insert_layer_mask(cut(true, inner, plain, 1.));
        scene.insert_primitive(quad(inner, 0xffffffff));
        scene.insert_layer_mask(cut(false, inner, plain, 1.));
        scene.insert_layer_mask(cut(false, outer, plain, 1.));
        let image = render(&mut renderer, &mut scene);
        let near = |x: u32, y: u32, want: [u8; 4]| {
            let pixel = image.get_pixel(x, y).0;
            let close = (0..4).all(|i| pixel[i].abs_diff(want[i]) <= 1);
            assert!(close, "({x}, {y}): {pixel:?}, not {want:?}");
        };
        // The top of each card is fully shown (stops at 1 above 18%).
        near(50, 15, [255, 0, 0, 255]);
        near(150, 15, [0, 255, 0, 255]);
        // The inner group, over the outer's blue, shows white at its top;
        // the outer card shows blue beside it.
        near(90, 113, [255, 255, 255, 255]);
        near(20, 113, [0, 0, 255, 255]);
        // Low in the inner card its stops are 0: what was under it, the
        // outer group's blue, shows, as beside it, not the page's black.
        let low = image.get_pixel(90, 146).0;
        near(20, 146, low);
        assert!(low[2] > 60 && low[0] == 0, "{low:?}");
    }
}
