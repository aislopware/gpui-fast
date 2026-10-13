//! Backdrop blur on Metal: gpui's `fast::backdrop`.
//!
//! A [`ComposedBatch::Blurs`] ends the pass drawing the frame once for all
//! its blurs, copies the region they read (what shows of each and three
//! standard deviations around; one bounding box for a group of nearby
//! blurs of one radius) out of the target into a scratch texture, blurs
//! that with `MPSImageGaussianBlur` (a true gaussian; its edges clamp, so a
//! blur at the window's edge does not darken), and begins the pass again,
//! marking the holes' interiors as a pass restarted after paths does. It
//! then draws the blurred backdrop back in two instanced draws per group:
//! one keeping `destination × (1 − k)` and one adding `blurred × k`, `k`
//! being each region's antialiased rounded-rectangle coverage times its
//! edge fade and opacity. The two leave colour and alpha exactly a mix of
//! the two by `k`, which one draw can't when the drawable is not opaque (a
//! window over vibrancy, or holes).
//!
//! What the blur reads is what GPUI drew: a native under a hole is not in
//! the drawable, and shows through the blurred backdrop unblurred, under
//! whatever is painted over it.
//!
//! The scratch textures are kept by size, rounded up to 64 pixels, four at
//! most and 32 MiB in all; the gaussian kernels by standard deviation, four
//! at most. Both go after 120 frames without a blur. A window's drawables
//! are made readable (`framebufferOnly` off) on the first frame with a
//! blur, before its drawable is taken, and made write-only again after 600
//! frames without one.
//!
//! The approach (snapshot, MPS gaussian, scratch textures sized to the
//! region) is zui's (<https://github.com/zeronsh/zui>, `crates/gpui_macos/
//! src/metal_renderer.rs` at `361ff20`; Apache-2.0, by Wing and the zui
//! contributors).
//!
//! [`ComposedBatch::Blurs`]: gpui::composition::ComposedBatch::Blurs

use std::{mem, ops::Range};

use anyhow::Result;
use gpui::{BackdropBlur, DevicePixels, Scene, Size};
use metal::{
    CommandBufferRef, DeviceRef, LibraryRef, MTLPixelFormat, RenderCommandEncoderRef,
    RenderPipelineState, Texture, TextureRef,
};
use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool},
};

use crate::metal_renderer::{InstanceBinding, MetalRenderer, new_command_encoder_for_texture};

#[link(name = "MetalPerformanceShaders", kind = "framework")]
unsafe extern "C" {
    fn MPSSupportsMTLDevice(device: *mut AnyObject) -> Bool;
}

/// The shaders' buffer slots (`BackdropBlurInputIndex` in `shaders.metal`).
const VERTICES: u64 = 0;
const BLURS: u64 = 1;
const VIEWPORT_SIZE: u64 = 2;
const SOURCE_RECT: u64 = 3;
const SOURCE_TEXTURE: u64 = 0;

/// Scratch textures are sized in steps of this many pixels, so a region
/// that grows a little reuses its pair.
const QUANTUM: u64 = 64;
/// How many scratch pairs and kernels are kept.
const KEPT: usize = 4;
/// The most bytes the kept scratch pairs take, unless one pair needs more.
const SCRATCH_BUDGET: u64 = 32 * 1024 * 1024;
/// Frames without a blur after which scratch textures and kernels go.
const RELEASE_AFTER_FRAMES: u32 = 120;
/// Frames without a blur after which a window's drawables are write-only
/// again.
const WRITE_ONLY_AFTER_FRAMES: u32 = 600;

/// A copy of a region of the target and the same region blurred.
struct Scratch {
    copy: Texture,
    blurred: Texture,
}

impl Scratch {
    fn bytes(&self) -> u64 {
        self.copy.width() * self.copy.height() * 8
    }
}

/// What a renderer keeps for backdrop blurs.
pub(crate) struct Backdrop {
    /// Keeps `destination × (1 − k)`.
    erase: RenderPipelineState,
    /// Adds `blurred × k`.
    add: RenderPipelineState,
    /// Whether Metal Performance Shaders run on the device.
    supported: bool,
    scratch: Vec<Scratch>,
    kernels: Vec<(f32, Retained<AnyObject>)>,
    blur_free_frames: u32,
    /// Whether the window's drawables were made readable for a blur.
    readable: bool,
}

impl Backdrop {
    pub(crate) fn new(device: &DeviceRef, library: &LibraryRef, format: MTLPixelFormat) -> Self {
        use metal::foreign_types::ForeignTypeRef as _;
        let pipeline = |label: &str, fragment: &str, source: metal::MTLBlendFactor| {
            let descriptor = metal::RenderPipelineDescriptor::new();
            descriptor.set_label(label);
            let vertex = library
                .get_function("backdrop_blur_vertex", None)
                .expect("error locating vertex function");
            let fragment = library
                .get_function(fragment, None)
                .expect("error locating fragment function");
            descriptor.set_vertex_function(Some(&vertex));
            descriptor.set_fragment_function(Some(&fragment));
            let color = descriptor.color_attachments().object_at(0).unwrap();
            color.set_pixel_format(format);
            color.set_blending_enabled(true);
            color.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
            color.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
            color.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
            color.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
            color.set_destination_rgb_blend_factor(source);
            color.set_destination_alpha_blend_factor(source);
            descriptor.set_stencil_attachment_pixel_format(super::occlusion::STENCIL_FORMAT);
            device
                .new_render_pipeline_state(&descriptor)
                .expect("could not create render pipeline state")
        };
        // SAFETY: a live MTLDevice.
        let supported = unsafe { MPSSupportsMTLDevice(device.as_ptr().cast()) }.as_bool();
        Self {
            erase: pipeline(
                "backdrop_blur_erase",
                "backdrop_blur_erase_fragment",
                metal::MTLBlendFactor::OneMinusSourceAlpha,
            ),
            add: pipeline(
                "backdrop_blur_add",
                "backdrop_blur_fragment",
                metal::MTLBlendFactor::One,
            ),
            supported,
            scratch: Vec::new(),
            kernels: Vec::new(),
            blur_free_frames: 0,
            readable: false,
        }
    }

    /// Notes whether the frame `scene` draws a blur, releasing what blurs
    /// keep after a run of frames without one.
    pub(crate) fn begin_frame(&mut self, scene: &Scene) {
        if !scene.backdrop_blurs().is_empty() {
            self.blur_free_frames = 0;
            return;
        }
        self.blur_free_frames = self.blur_free_frames.saturating_add(1);
        if self.blur_free_frames == RELEASE_AFTER_FRAMES {
            self.scratch.clear();
            self.kernels.clear();
        }
    }

    /// The scratch pair for a region of `width` × `height` pixels of a
    /// target of `target_width` × `target_height` in `format`.
    fn scratch(
        &mut self,
        device: &DeviceRef,
        (width, height): (u64, u64),
        (target_width, target_height): (u64, u64),
        format: MTLPixelFormat,
    ) -> (Texture, Texture) {
        let width = (width.div_ceil(QUANTUM) * QUANTUM).min(target_width);
        let height = (height.div_ceil(QUANTUM) * QUANTUM).min(target_height);
        // The copy always fills its texture from inside the target, so a
        // pair larger than the target can't be used.
        self.scratch.retain(|pair| {
            pair.copy.width() <= target_width
                && pair.copy.height() <= target_height
                && pair.copy.pixel_format() == format
        });
        if let Some(index) = self
            .scratch
            .iter()
            .position(|pair| pair.copy.width() == width && pair.copy.height() == height)
        {
            let pair = self.scratch.remove(index);
            self.scratch.push(pair);
        } else {
            let needed = width * height * 8;
            let budget = SCRATCH_BUDGET.max(needed);
            while !self.scratch.is_empty()
                && (self.scratch.len() >= KEPT
                    || self.scratch.iter().map(Scratch::bytes).sum::<u64>() + needed > budget)
            {
                self.scratch.remove(0);
            }
            let descriptor = metal::TextureDescriptor::new();
            descriptor.set_texture_type(metal::MTLTextureType::D2);
            descriptor.set_pixel_format(format);
            descriptor.set_width(width);
            descriptor.set_height(height);
            descriptor.set_storage_mode(metal::MTLStorageMode::Private);
            descriptor.set_usage(metal::MTLTextureUsage::ShaderRead);
            let copy = device.new_texture(&descriptor);
            descriptor.set_usage(
                metal::MTLTextureUsage::ShaderRead | metal::MTLTextureUsage::ShaderWrite,
            );
            let blurred = device.new_texture(&descriptor);
            self.scratch.push(Scratch { copy, blurred });
        }
        let pair = self.scratch.last().unwrap();
        (pair.copy.clone(), pair.blurred.clone())
    }

    /// The gaussian kernel of standard deviation `sigma`.
    fn kernel(&mut self, device: &DeviceRef, sigma: f32) -> Option<Retained<AnyObject>> {
        use metal::foreign_types::ForeignTypeRef as _;
        if let Some(index) = self
            .kernels
            .iter()
            .position(|(kept, _)| (*kept - sigma).abs() < 0.01)
        {
            let entry = self.kernels.remove(index);
            let kernel = entry.1.clone();
            self.kernels.push(entry);
            return Some(kernel);
        }
        let class = AnyClass::get(c"MPSImageGaussianBlur")?;
        // SAFETY: `initWithDevice:sigma:` takes a live MTLDevice and a float
        // and returns a retained kernel, or nil; `setEdgeMode:` takes an
        // `MPSImageEdgeMode` (1 is clamp).
        let kernel = unsafe {
            let allocated: *mut AnyObject = msg_send![class, alloc];
            let kernel: *mut AnyObject = msg_send![
                allocated,
                initWithDevice: device.as_ptr().cast::<AnyObject>(),
                sigma: sigma
            ];
            let kernel = Retained::from_raw(kernel)?;
            let _: () = msg_send![&*kernel, setEdgeMode: 1usize];
            kernel
        };
        if self.kernels.len() >= KEPT {
            self.kernels.remove(0);
        }
        self.kernels.push((sigma, kernel.clone()));
        Some(kernel)
    }
}

/// Makes the drawables of `layer` readable before the drawable for `scene`
/// is taken if `scene` draws a blur, and write-only again after a run of
/// frames without one.
pub(crate) fn prepare_layer(
    renderer: &mut MetalRenderer,
    layer: &metal::MetalLayerRef,
    scene: &Scene,
) {
    let backdrop = &mut renderer.fast_backdrop;
    let blurs = !scene.backdrop_blurs().is_empty() && backdrop.supported;
    if blurs && !backdrop.readable {
        layer.set_framebuffer_only(false);
        backdrop.readable = true;
    } else if !blurs && backdrop.readable && backdrop.blur_free_frames >= WRITE_ONLY_AFTER_FRAMES {
        #[cfg(not(all(feature = "test-support", debug_assertions)))]
        layer.set_framebuffer_only(true);
        backdrop.readable = false;
    }
}

/// Writes the frame's blurs into its instances.
pub(crate) fn write(
    scene: &Scene,
    writer: &mut crate::metal_renderer::InstanceBufferWriter,
) -> Result<InstanceBinding> {
    writer.write(scene.backdrop_blurs())
}

/// What a pass needs to begin again after a blur.
pub(crate) struct Pass<'a> {
    pub(crate) command_buffer: &'a CommandBufferRef,
    pub(crate) texture: &'a TextureRef,
    pub(crate) stencil: &'a TextureRef,
    pub(crate) viewport_size: Size<DevicePixels>,
}

/// Draws the blurs `range` of `scene`, of one draw order, over what
/// `encoder`'s pass drew so far: ends the pass once, blurs a copy of the
/// region each group of them reads, and returns the pass begun again with
/// the blurred backdrop drawn back. Returns `encoder` as it is when the
/// blurs show nothing or Metal Performance Shaders can't run.
///
/// A group is a run of blurs of one radius whose regions' bounding box is
/// at most twice as large as the regions together: one copy and one
/// gaussian serve it. Blurs of a draw order never overlap (`fast::backdrop`
/// joins only those whose regions are apart), so each draws back over what
/// the others left.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw<'a>(
    renderer: &mut MetalRenderer,
    scene: &Scene,
    range: Range<usize>,
    blurs: &InstanceBinding,
    fades: &InstanceBinding,
    pass: &Pass<'a>,
    encoder: &'a RenderCommandEncoderRef,
) -> &'a RenderCommandEncoderRef {
    if !renderer.fast_backdrop.supported {
        return encoder;
    }
    let all = scene.backdrop_blurs();
    let (target_width, target_height) = (pass.texture.width(), pass.texture.height());
    // The region a blur reads, in whole pixels inside the target.
    let region = |blur: &BackdropBlur| {
        let reach = blur.reach();
        let x0 = (reach.origin.x.0.floor().max(0.) as u64).min(target_width);
        let y0 = (reach.origin.y.0.floor().max(0.) as u64).min(target_height);
        let x1 = ((reach.origin.x.0 + reach.size.width.0).ceil().max(0.) as u64).min(target_width);
        let y1 =
            ((reach.origin.y.0 + reach.size.height.0).ceil().max(0.) as u64).min(target_height);
        (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
    };
    let area = |[x0, y0, x1, y1]: [u64; 4]| (x1 - x0) * (y1 - y0);
    // Each group: its blurs, the bounding box of their regions, and their
    // regions' areas together.
    let mut groups: Vec<(Range<usize>, [u64; 4], u64)> = Vec::new();
    for index in range {
        let Some(own) = region(&all[index]) else {
            continue;
        };
        if let Some((group, union, together)) = groups.last_mut()
            && group.end == index
            && all[group.start].blur_radius == all[index].blur_radius
        {
            let joined = [
                union[0].min(own[0]),
                union[1].min(own[1]),
                union[2].max(own[2]),
                union[3].max(own[3]),
            ];
            if area(joined) <= 2 * (*together + area(own)) {
                *union = joined;
                *together += area(own);
                group.end = index + 1;
                continue;
            }
        }
        groups.push((index..index + 1, own, area(own)));
    }
    let mut encoder = encoder;
    for (group, union, _) in groups {
        let sigma = all[group.start].blur_radius.0.max(0.5);
        encoder = draw_group(renderer, group, union, sigma, blurs, fades, pass, encoder);
    }
    encoder
}

/// Draws the blurs `group`, of standard deviation `sigma`, reading `union`
/// of the target.
#[allow(clippy::too_many_arguments)]
fn draw_group<'a>(
    renderer: &mut MetalRenderer,
    group: Range<usize>,
    [x0, y0, x1, y1]: [u64; 4],
    sigma: f32,
    blurs: &InstanceBinding,
    fades: &InstanceBinding,
    pass: &Pass<'a>,
    encoder: &'a RenderCommandEncoderRef,
) -> &'a RenderCommandEncoderRef {
    use metal::foreign_types::ForeignTypeRef as _;
    let (target_width, target_height) = (pass.texture.width(), pass.texture.height());
    let device = renderer.device.clone();
    let Some(kernel) = renderer.fast_backdrop.kernel(&device, sigma) else {
        return encoder;
    };
    let (copy, blurred) = renderer.fast_backdrop.scratch(
        &device,
        (x1 - x0, y1 - y0),
        (target_width, target_height),
        pass.texture.pixel_format(),
    );
    // The copy fills its whole texture from inside the target, so that the
    // kernel's clamped edge is the target's edge where the region meets it.
    let copy_x = x0.min(target_width - copy.width());
    let copy_y = y0.min(target_height - copy.height());
    encoder.end_encoding();
    let blit = pass.command_buffer.new_blit_command_encoder();
    blit.copy_from_texture(
        pass.texture,
        0,
        0,
        metal::MTLOrigin {
            x: copy_x,
            y: copy_y,
            z: 0,
        },
        metal::MTLSize {
            width: copy.width(),
            height: copy.height(),
            depth: 1,
        },
        &copy,
        0,
        0,
        metal::MTLOrigin { x: 0, y: 0, z: 0 },
    );
    blit.end_encoding();
    // SAFETY: a live kernel, command buffer and two textures of one size and
    // format, the destination writable.
    unsafe {
        let _: () = msg_send![
            &*kernel,
            encodeToCommandBuffer: pass.command_buffer.as_ptr().cast::<AnyObject>(),
            sourceTexture: copy.as_ptr().cast::<AnyObject>(),
            destinationTexture: blurred.as_ptr().cast::<AnyObject>()
        ];
    }
    let encoder = new_command_encoder_for_texture(
        pass.command_buffer,
        pass.texture,
        pass.stencil,
        pass.viewport_size,
        None,
    );
    let source_rect = [
        copy_x as f32,
        copy_y as f32,
        copy.width() as f32,
        copy.height() as f32,
    ];
    for pipeline in [&renderer.fast_backdrop.erase, &renderer.fast_backdrop.add] {
        encoder.set_render_pipeline_state(pipeline);
        encoder.set_vertex_buffer(VERTICES, Some(&renderer.unit_vertices), 0);
        encoder.set_vertex_buffer(BLURS, Some(&blurs.buffer), blurs.offset as u64);
        encoder.set_fragment_buffer(BLURS, Some(&blurs.buffer), blurs.offset as u64);
        encoder.set_vertex_bytes(
            VIEWPORT_SIZE,
            mem::size_of_val(&pass.viewport_size) as u64,
            &pass.viewport_size as *const Size<DevicePixels> as *const _,
        );
        encoder.set_fragment_bytes(
            SOURCE_RECT,
            mem::size_of_val(&source_rect) as u64,
            source_rect.as_ptr() as *const _,
        );
        encoder.set_fragment_texture(SOURCE_TEXTURE, Some(&blurred));
        super::edge_fade::bind(encoder, fades);
        encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            group.len() as u64,
            group.start as u64,
        );
    }
    encoder
}

/// Pixel tests: a headless renderer draws scenes with blurs, read back.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{
        BackdropBlur, Bounds, ContentMask, Corners, DevicePixels, EdgeFadeRamps, Edges, Hsla, Quad,
        ScaledPixels, Scene, point, rgba, size,
    };
    use image::RgbaImage;
    use parking_lot::Mutex;

    use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

    const W: i32 = 240;
    const H: i32 = 160;

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

    fn blur(bounds: Bounds<ScaledPixels>, sigma: f32) -> BackdropBlur {
        BackdropBlur {
            order: 0,
            fade: 0,
            bounds,
            content_mask: everywhere(),
            corner_radii: Corners::default(),
            blur_radius: ScaledPixels(sigma),
            opacity: 1.,
        }
    }

    fn renderer() -> Option<MetalRenderer> {
        if metal::Device::system_default().is_none() {
            eprintln!("skipped: no Metal device");
            return None;
        }
        Some(MetalRenderer::new_headless(Arc::new(Mutex::new(
            InstanceBufferPool::default(),
        ))))
    }

    fn render(renderer: &mut MetalRenderer, scene: &mut Scene) -> RgbaImage {
        scene.finish();
        renderer
            .render_scene_to_image(scene, size(DevicePixels(W), DevicePixels(H)))
            .expect("scene rendered")
    }

    /// Black on the left of `x = 120`, white on its right.
    fn halves(scene: &mut Scene) {
        scene.insert_primitive(quad(sp(0., 0., 120., H as f32), 0x000000ff));
        scene.insert_primitive(quad(sp(120., 0., 120., H as f32), 0xffffffff));
    }

    /// The standard normal distribution function.
    fn phi(x: f32) -> f32 {
        // Abramowitz and Stegun 7.1.26, to 1.5e-7.
        let t = 1. / (1. + 0.3275911 * x.abs() / std::f32::consts::SQRT_2);
        let y = 1.
            - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t
                + 0.254829592)
                * t
                * (-(x * x) / 2.).exp();
        0.5 * (1. + y.copysign(x))
    }

    /// A step under a blur of σ 8 becomes the gaussian's distribution
    /// across it: half way at the step, 84% a σ past it; what is outside
    /// the region is untouched.
    #[test]
    fn a_blur_spreads_a_step_as_a_gaussian_of_its_sigma() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let mut scene = Scene::default();
        halves(&mut scene);
        scene.insert_backdrop_blur(blur(sp(60., 40., 120., 80.), 8.));
        let image = render(&mut renderer, &mut scene);
        let mut worst = 0i32;
        for x in 64..176u32 {
            let expected = (phi((x as f32 + 0.5 - 120.) / 8.) * 255.).round() as i32;
            let actual = i32::from(image.get_pixel(x, 80)[0]);
            worst = worst.max((actual - expected).abs());
        }
        eprintln!("step under σ 8: within {worst} of 255 levels");
        assert!(worst <= 3, "{worst}");
        // Outside the region, and in it above and below, exactly as drawn.
        assert_eq!(image.get_pixel(119, 20)[0], 0);
        assert_eq!(image.get_pixel(120, 20)[0], 255);
        assert_eq!(image.get_pixel(119, 140)[0], 0);
        assert_eq!(image.get_pixel(59, 80)[0], 0);
        assert_eq!(image.get_pixel(180, 80)[0], 255);
    }

    /// What is painted after the blur is drawn over it, sharp; what was
    /// painted before it under the region is drawn blurred.
    #[test]
    fn what_is_painted_after_a_blur_is_drawn_over_it() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let mut scene = Scene::default();
        halves(&mut scene);
        scene.insert_backdrop_blur(blur(sp(60., 40., 120., 80.), 8.));
        // A red bar across the step, painted over the blur.
        scene.insert_primitive(quad(sp(100., 70., 40., 20.), 0xff0000ff));
        let image = render(&mut renderer, &mut scene);
        let pixel = image.get_pixel(119, 80);
        assert_eq!((pixel[0], pixel[1], pixel[2]), (255, 0, 0));
        // Beside the bar, the step is blurred.
        let blurred = image.get_pixel(119, 60)[0];
        assert!((100..155).contains(&blurred), "{blurred}");
    }

    /// The region's rounded corners are cut with an antialiased edge, and
    /// the content mask clips it.
    #[test]
    fn a_blur_is_clipped_to_its_rounded_corners_and_its_content_mask() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let mut scene = Scene::default();
        halves(&mut scene);
        let mut rounded = blur(sp(100., 40., 40., 80.), 8.);
        rounded.corner_radii = Corners::all(ScaledPixels(16.));
        rounded.content_mask = ContentMask {
            bounds: sp(0., 0., W as f32, 100.),
        };
        scene.insert_backdrop_blur(rounded);
        let image = render(&mut renderer, &mut scene);
        // Past the top left arc: sharp black.
        assert_eq!(image.get_pixel(101, 41)[0], 0);
        // Inside: blurred.
        let inside = image.get_pixel(118, 70)[0];
        assert!((60..200).contains(&inside), "{inside}");
        // Below the content mask: sharp.
        assert_eq!(image.get_pixel(118, 105)[0], 0);
        // Along the arc, a pixel drawn in part.
        let arc = (0..16).map(|i| image.get_pixel(100 + i, 40 + i)[0]);
        let mut partial = false;
        for (i, value) in arc.enumerate() {
            let sharp = 0;
            let fully = image.get_pixel(118, 40 + i as u32 + 20)[0];
            partial |= value != sharp && value != fully;
        }
        assert!(partial, "the corner is antialiased");
    }

    /// A blur flush with the target's edge reads the edge clamped: a flat
    /// colour stays that colour up to the edge.
    #[test]
    fn a_blur_at_the_target_edge_does_not_darken_it() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let mut scene = Scene::default();
        scene.insert_primitive(quad(sp(0., 0., W as f32, H as f32), 0x8090a0ff));
        scene.insert_backdrop_blur(blur(sp(0., 0., 80., 80.), 12.));
        let image = render(&mut renderer, &mut scene);
        for (x, y) in [(0, 0), (0, 40), (40, 0), (79, 79)] {
            let pixel = image.get_pixel(x, y);
            for (channel, want) in [0x80u8, 0x90, 0xa0].into_iter().enumerate() {
                assert!(pixel[channel].abs_diff(want) <= 1, "({x}, {y}): {pixel:?}");
            }
        }
    }

    /// Opacity and an edge fade mix the blurred backdrop with what is under
    /// it by their product, and two blurs in a frame each read what is
    /// under them, the second the first's result.
    #[test]
    fn opacity_and_fades_mix_the_blur_and_blurs_stack() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let draw = |renderer: &mut MetalRenderer, opacity: f32, fade: bool| {
            let mut scene = Scene::default();
            halves(&mut scene);
            let mut first = blur(sp(100., 40., 40., 80.), 6.);
            first.opacity = opacity;
            if fade {
                let ramps = EdgeFadeRamps::new(
                    sp(100., 40., 40., 80.),
                    Edges {
                        top: ScaledPixels(40.),
                        ..Edges::default()
                    },
                    Edges::all(1.),
                );
                first.fade = scene.add_edge_fade(ramps);
            }
            scene.insert_backdrop_blur(first);
            render(renderer, &mut scene)
        };
        let whole = draw(&mut renderer, 1., false);
        let half = draw(&mut renderer, 0.5, false);
        let sharp = 0u8;
        let (x, y) = (118, 80);
        let mixed = (f32::from(whole.get_pixel(x, y)[0]) + f32::from(sharp)) / 2.;
        assert!(
            (f32::from(half.get_pixel(x, y)[0]) - mixed).abs() <= 1.,
            "{} against {mixed}",
            half.get_pixel(x, y)[0]
        );
        // Faded toward the top: sharp at the top edge, blurred lower down.
        let faded = draw(&mut renderer, 1., true);
        assert!(faded.get_pixel(x, 41)[0] <= 2);
        assert_eq!(faded.get_pixel(x, 100)[0], whole.get_pixel(x, 100)[0]);

        // A second blur over the first.
        let mut scene = Scene::default();
        halves(&mut scene);
        scene.insert_backdrop_blur(blur(sp(60., 40., 120., 80.), 4.));
        scene.insert_primitive(quad(sp(118., 60., 4., 40.), 0x00ff00ff));
        scene.insert_backdrop_blur(blur(sp(60., 40., 120., 80.), 4.));
        let image = render(&mut renderer, &mut scene);
        let green = image.get_pixel(120, 80);
        assert!(
            green[1] > 40 && green[1] < 230,
            "the bar is blurred: {green:?}"
        );
    }

    /// A blur among natives' holes and paths draws everything outside its
    /// region as the same scene without it does: the pass it restarts marks
    /// the holes' interiors again and goes on culling for the next hole.
    #[test]
    fn a_blur_among_holes_and_paths_leaves_the_rest_as_drawn_without_it() {
        use gpui::composition::{NativeId, NativePlacement};
        let Some(mut renderer) = renderer() else {
            return;
        };
        let build = |with_blur: bool| {
            let mut scene = Scene::default();
            halves(&mut scene);
            let native = |id: u64, bounds: Bounds<ScaledPixels>| NativePlacement {
                order: 0,
                id: NativeId(id),
                hitbox: None,
                focus: None,
                bounds,
                content_mask: everywhere(),
                corner_radii: Corners::all(ScaledPixels(4.)),
                opacity: 1.,
            };
            scene.insert_native(native(1, sp(10., 10., 60., 50.)));
            if with_blur {
                scene.insert_backdrop_blur(blur(sp(40., 30., 120., 60.), 6.));
            }
            scene.insert_native(native(2, sp(170., 100., 50., 40.)));
            let mut path = gpui::Path::new(point(gpui::px(150.), gpui::px(110.)));
            path.line_to(point(gpui::px(230.), gpui::px(110.)));
            path.line_to(point(gpui::px(190.), gpui::px(150.)));
            path.color = Hsla::from(rgba(0x3366ccff)).into();
            path.content_mask = ContentMask {
                bounds: Bounds::new(
                    point(gpui::px(0.), gpui::px(0.)),
                    size(gpui::px(W as f32), gpui::px(H as f32)),
                ),
            };
            scene.insert_primitive(path.scale(1.));
            scene.insert_primitive(quad(sp(20., 120., 60., 20.), 0x22aa44ff));
            scene
        };
        let plain = render(&mut renderer, &mut build(false));
        let blurred = render(&mut renderer, &mut build(true));
        let region = sp(40., 30., 120., 60.);
        let mut differ = 0;
        for (x, y, pixel) in blurred.enumerate_pixels() {
            let inside = region.contains(&point(
                ScaledPixels(x as f32 + 0.5),
                ScaledPixels(y as f32 + 0.5),
            ));
            if !inside && pixel != plain.get_pixel(x, y) {
                differ += 1;
            }
        }
        assert_eq!(differ, 0, "pixels outside the blur differ");
        // The first native's hole stays cleared where the blur leaves it.
        assert_eq!(blurred.get_pixel(15, 15)[3], 0);
        // Inside the blur, over the step, blurred.
        let step = blurred.get_pixel(119, 85)[0];
        assert!((60..200).contains(&step), "{step}");
    }

    /// Two blurs apart, of one radius, joined in one draw order, are copied
    /// and blurred as one group, and each draws as it does alone.
    #[test]
    fn blurs_drawn_from_one_copy_draw_as_each_alone() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let (a, b) = (sp(90., 10., 60., 40.), sp(90., 100., 60., 40.));
        let build = |first: bool, second: bool| {
            let mut scene = Scene::default();
            halves(&mut scene);
            if first {
                scene.insert_backdrop_blur(blur(a, 4.));
                scene.insert_primitive(quad(a, 0xff000040));
            }
            if second {
                scene.insert_backdrop_blur(blur(b, 4.));
                scene.insert_primitive(quad(b, 0x0000ff40));
            }
            scene
        };
        let mut both = build(true, true);
        both.finish();
        let blurs = both.backdrop_blurs();
        assert_eq!(blurs[0].order, blurs[1].order, "the second joins the first");
        let groups: Vec<_> = both
            .composed_batches()
            .filter(|batch| matches!(batch, gpui::composition::ComposedBatch::Blurs(_)))
            .collect();
        assert_eq!(groups.len(), 1);
        let both = render(&mut renderer, &mut both);
        let alone = [
            render(&mut renderer, &mut build(true, false)),
            render(&mut renderer, &mut build(false, true)),
        ];
        let mut worst = 0;
        for (region, alone) in [a, b].into_iter().zip(&alone) {
            for (x, y, pixel) in both.enumerate_pixels() {
                let centre = point(ScaledPixels(x as f32 + 0.5), ScaledPixels(y as f32 + 0.5));
                if region.contains(&centre) {
                    for channel in 0..4 {
                        let difference = (i32::from(pixel[channel])
                            - i32::from(alone.get_pixel(x, y)[channel]))
                        .abs();
                        worst = worst.max(difference);
                    }
                }
            }
        }
        eprintln!("grouped against alone: within {worst} of 255 levels");
        assert!(worst <= 1, "{worst}");
        // Blurred, not copied: across the step inside each region.
        let step = both.get_pixel(119, 30)[1];
        assert!((40..215).contains(&step), "{step}");
    }

    /// What a frosted popover over a list scrolling under it costs, against
    /// the same popover opaque with a large shadow (zeron's two materials)
    /// and against no popover: per frame, the CPU time `render_frame` takes
    /// to encode and the GPU time of the frame's command buffer, at a 14"
    /// MacBook Pro's full window (3024 x 1964), the list moving 12 device
    /// pixels a frame, so every blur is computed afresh. Zeron frosts with σ
    /// 16 points, 32 device pixels.
    ///
    /// `cargo test -p gpui_apple --release --lib frosted_popover_frame_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check"]
    fn frosted_popover_frame_cost() {
        use std::time::{Duration, Instant};

        const WIDTH: i32 = 3024;
        const HEIGHT: i32 = 1964;
        const FRAMES: usize = 300;
        let Some(mut renderer) = renderer() else {
            return;
        };
        let window = size(DevicePixels(WIDTH), DevicePixels(HEIGHT));
        let mask = ContentMask {
            bounds: sp(0., 0., WIDTH as f32, HEIGHT as f32),
        };
        let solid = |bounds: Bounds<ScaledPixels>, color: u32, radius: f32| Quad {
            bounds,
            content_mask: mask,
            background: Hsla::from(rgba(color)).into(),
            corner_radii: Corners::all(ScaledPixels(radius)),
            ..Default::default()
        };
        // Rows of 64 device pixels, each a background, 40 glyph-sized marks
        // and a pill.
        let list = |scene: &mut Scene, offset: f32| {
            scene.insert_primitive(solid(mask.bounds, 0x0d0d0dff, 0.));
            let first = (offset / 64.).floor() as i32;
            for row in first..first + HEIGHT / 64 + 2 {
                let y = row as f32 * 64. - offset;
                let color = if row % 2 == 0 { 0x181818ff } else { 0x1e1e22ff };
                scene.insert_primitive(solid(sp(0., y, WIDTH as f32, 64.), color, 0.));
                for mark in 0..40 {
                    let x = 48. + mark as f32 * 26. + (row % 5) as f32 * 3.;
                    let shade = 0x80 + (mark * 7 + row) % 0x60;
                    let color = (shade as u32) << 24 | (shade as u32) << 16 | 0xccff;
                    scene.insert_primitive(solid(sp(x, y + 22., 18., 20.), color, 3.));
                }
                scene.insert_primitive(solid(sp(2700., y + 16., 96., 32.), 0x3fa66bff, 16.));
            }
        };
        let popover = sp(912., 360., 1200., 880.);
        let composer = sp(312., 1560., 2400., 320.);
        let tooltip = sp(2240., 220., 560., 96.);
        let rows = |scene: &mut Scene, panel: Bounds<ScaledPixels>| {
            let count = ((panel.size.height.0 - 48.) / 54.) as i32;
            for row in 0..count {
                let y = panel.origin.y.0 + 24. + row as f32 * 54.;
                let color = if row == 0 { 0xffffff1a } else { 0xffffff0a };
                let width = panel.size.width.0 - 48.;
                scene.insert_primitive(solid(sp(panel.origin.x.0 + 24., y, width, 48.), color, 8.));
            }
        };
        let opaque = |scene: &mut Scene, panel: Bounds<ScaledPixels>| {
            scene.insert_primitive(gpui::Shadow {
                order: 0,
                blur_radius: ScaledPixels(48.),
                bounds: panel.dilate(ScaledPixels(144.)),
                corner_radii: Corners::all(ScaledPixels(24.)),
                content_mask: mask,
                color: gpui::hsla(0., 0., 0., 0.45),
                element_bounds: panel,
                element_corner_radii: Corners::all(ScaledPixels(24.)),
                inset: 0,
                pad: 0,
            });
            scene.insert_primitive(solid(panel, 0x27272aff, 24.));
            rows(scene, panel);
        };
        let frosted = |scene: &mut Scene, panel: Bounds<ScaledPixels>| {
            scene.insert_backdrop_blur(BackdropBlur {
                order: 0,
                fade: 0,
                bounds: panel,
                content_mask: mask,
                corner_radii: Corners::all(ScaledPixels(24.)),
                blur_radius: ScaledPixels(32.),
                opacity: 1.,
            });
            scene.insert_primitive(solid(panel, 0x1818182a, 24.));
            rows(scene, panel);
        };

        let target = {
            let descriptor = metal::TextureDescriptor::new();
            descriptor.set_width(WIDTH as u64);
            descriptor.set_height(HEIGHT as u64);
            descriptor.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
            descriptor.set_usage(
                metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead,
            );
            descriptor.set_storage_mode(metal::MTLStorageMode::Private);
            renderer.device.new_texture(&descriptor)
        };
        let mut draw = |scene: &Scene| -> (Duration, Duration) {
            objc2::rc::autoreleasepool(|_| {
                let start = Instant::now();
                let command_buffer = renderer
                    .render_frame(scene, &target, window)
                    .expect("frame encoded");
                let cpu = start.elapsed();
                command_buffer.commit();
                command_buffer.wait_until_completed();
                // SAFETY: `GPUStartTime` and `GPUEndTime` are `MTLCommandBuffer`
                // properties, read after the buffer completed.
                let (gpu_start, gpu_end): (f64, f64) = unsafe {
                    let command_buffer: &objc2::runtime::AnyObject =
                        &*foreign_types::ForeignType::as_ptr(&command_buffer).cast();
                    (
                        objc2::msg_send![command_buffer, GPUStartTime],
                        objc2::msg_send![command_buffer, GPUEndTime],
                    )
                };
                (cpu, Duration::from_secs_f64((gpu_end - gpu_start).max(0.)))
            })
        };
        // Zeron's 22 point file badges in tool rows, each frosted, down a
        // transcript's column.
        let badges = |scene: &mut Scene, frost: bool| {
            for badge in 0..20 {
                let panel = sp(
                    600. + (badge % 2) as f32 * 300.,
                    120. + badge as f32 * 88.,
                    240.,
                    44.,
                );
                if frost {
                    frosted(scene, panel);
                } else {
                    scene.insert_primitive(solid(panel, 0x27272aff, 10.));
                }
            }
        };
        let variants: [(&str, &dyn Fn(&mut Scene)); 7] = [
            ("list alone", &|_| {}),
            ("opaque popover", &|scene| opaque(scene, popover)),
            ("frosted popover", &|scene| frosted(scene, popover)),
            ("frosted popover, composer and tooltip", &|scene| {
                frosted(scene, popover);
                frosted(scene, composer);
                frosted(scene, tooltip);
            }),
            ("opaque popover, composer and tooltip", &|scene| {
                opaque(scene, popover);
                opaque(scene, composer);
                opaque(scene, tooltip);
            }),
            ("20 frosted badges", &|scene| badges(scene, true)),
            ("20 opaque badges", &|scene| badges(scene, false)),
        ];
        for (name, over) in variants {
            let mut samples = Vec::with_capacity(FRAMES);
            for frame in 0..FRAMES + 30 {
                let mut scene = Scene::default();
                list(&mut scene, (frame as f32 * 12.) % 6400.);
                over(&mut scene);
                scene.finish();
                let sample = draw(&scene);
                if frame >= 30 {
                    samples.push(sample);
                }
            }
            let at = |mut values: Vec<Duration>, percent: usize| {
                values.sort();
                values[values.len() * percent / 100]
            };
            let cpu: Vec<_> = samples.iter().map(|s| s.0).collect();
            let gpu: Vec<_> = samples.iter().map(|s| s.1).collect();
            eprintln!(
                "{name}: encode median {:?} p95 {:?}; gpu median {:?} p95 {:?}",
                at(cpu.clone(), 50),
                at(cpu, 95),
                at(gpu.clone(), 50),
                at(gpu, 95),
            );
        }
    }
}
