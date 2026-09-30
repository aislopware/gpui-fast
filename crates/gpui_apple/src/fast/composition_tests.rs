//! The pixel oracle for window composition. A scene whose natives cut holes,
//! with each native composited under the drawable the way Core Animation
//! composites a layer under GPUI's (`out = G + (1 − αG)·N`, premultiplied),
//! must look like one GPUI pass drawing each native as an opaque quad at its
//! place in the scene. That checks the holes, their corners and fades, their
//! draw order among the primitives, and the alpha blend, at once.

use std::cell::Cell;

use gpui::{
    Background, BorderStyle, Bounds, ContentMask, Corners, DevicePixels, Edges, Hsla, Path, Quad,
    ScaledPixels, Scene, Shadow, Underline, black,
    composition::{NativeId, NativePlacement},
    hsla, point, px, size,
};
use image::RgbaImage;
use metal::MTLPixelFormat;

use super::{InstanceBufferPool, MetalRenderer};

thread_local! {
    /// Set while a renderer is built with the additive alpha blend GPUI had before
    /// composition, to compare against.
    pub(super) static LEGACY_ALPHA_BLEND: Cell<bool> = const { Cell::new(false) };
    /// Set while a renderer is built to draw into another format than the drawables'.
    pub(super) static DRAWABLE_PIXEL_FORMAT: Cell<Option<MTLPixelFormat>> =
        const { Cell::new(None) };
}

const SIDE: i32 = 160;

/// A xorshift generator, so scenes are the same on every run without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * (self.below(1 << 20) as f32 / (1 << 20) as f32)
    }

    fn whole(&mut self, low: f32, high: f32) -> f32 {
        self.range(low, high).round()
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

fn scaled(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x), ScaledPixels(y)),
        size(ScaledPixels(width), ScaledPixels(height)),
    )
}

fn everywhere() -> ContentMask<ScaledPixels> {
    ContentMask {
        bounds: scaled(0., 0., SIDE as f32, SIDE as f32),
    }
}

/// Colours the shader and the CPU agree on exactly: every channel 0 or 1.
const SENTINELS: [Hsla; 6] = [
    hsla(0., 1., 0.5, 1.),
    hsla(1. / 3., 1., 0.5, 1.),
    hsla(2. / 3., 1., 0.5, 1.),
    hsla(1. / 6., 1., 0.5, 1.),
    hsla(0.5, 1., 0.5, 1.),
    hsla(5. / 6., 1., 0.5, 1.),
];

fn sentinel_rgb(color: Hsla) -> [f32; 3] {
    let rgba = color.to_rgb();
    [rgba.r, rgba.g, rgba.b]
}

#[derive(Clone)]
enum Operation {
    Quad(Quad),
    Shadow(Shadow),
    Underline(Underline),
    Path(Path<ScaledPixels>),
    Layer(Bounds<ScaledPixels>, Vec<Quad>),
    Native(NativePlacement, Hsla),
}

fn random_box(rng: &mut Rng) -> Bounds<ScaledPixels> {
    let x = rng.whole(-10., SIDE as f32 - 20.);
    let y = rng.whole(-10., SIDE as f32 - 20.);
    scaled(x, y, rng.whole(8., 90.), rng.whole(8., 90.))
}

fn random_mask(rng: &mut Rng) -> ContentMask<ScaledPixels> {
    if rng.chance(60) {
        return everywhere();
    }
    ContentMask {
        bounds: random_box(rng).dilate(ScaledPixels(rng.whole(0., 30.))),
    }
}

fn random_radii(rng: &mut Rng, bounds: Bounds<ScaledPixels>) -> Corners<ScaledPixels> {
    if rng.chance(40) {
        return Corners::default();
    }
    let most = bounds.size.width.0.min(bounds.size.height.0) / 2.;
    Corners {
        top_left: ScaledPixels(rng.range(0., most)),
        top_right: ScaledPixels(rng.range(0., most)),
        bottom_right: ScaledPixels(rng.range(0., most)),
        bottom_left: ScaledPixels(rng.range(0., most)),
    }
}

fn random_color(rng: &mut Rng) -> Hsla {
    hsla(
        rng.range(0., 1.),
        rng.range(0.2, 1.),
        rng.range(0.1, 0.9),
        rng.pick(&[1., 1., 0.8, 0.5, 0.25]),
    )
}

fn random_quad(rng: &mut Rng) -> Quad {
    let bounds = random_box(rng);
    let border = rng.chance(30);
    Quad {
        order: 0,
        border_style: if rng.chance(20) {
            BorderStyle::Dashed
        } else {
            BorderStyle::Solid
        },
        bounds,
        content_mask: random_mask(rng),
        background: random_color(rng).into(),
        border_color: if border {
            random_color(rng)
        } else {
            black().opacity(0.)
        },
        corner_radii: random_radii(rng, bounds),
        border_widths: if border {
            Edges::all(ScaledPixels(rng.whole(1., 5.)))
        } else {
            Edges::default()
        },
    }
}

/// A scene of a window: an opaque background and a random mix of translucent,
/// rounded, bordered and clipped quads, shadows, underlines, paths, paint
/// layers and natives. Natives do not overlap each other: where a native covers
/// another one, its rectangular container would show over the lower native in
/// its rounded or faded corners, which one GPUI pass would not.
fn random_scene(seed: u64) -> Vec<Operation> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut operations = vec![Operation::Quad(Quad {
        bounds: scaled(0., 0., SIDE as f32, SIDE as f32),
        content_mask: everywhere(),
        background: hsla(0.6, 0.2, 0.15, 1.).into(),
        ..Quad::default()
    })];
    let mut natives: Vec<Bounds<ScaledPixels>> = Vec::new();
    for _ in 0..rng.below(18) + 6 {
        let operation = match rng.below(100) {
            0..35 => Operation::Quad(random_quad(&mut rng)),
            35..45 => {
                let bounds = random_box(&mut rng);
                let corner_radii = random_radii(&mut rng, bounds);
                Operation::Shadow(Shadow {
                    order: 0,
                    blur_radius: ScaledPixels(rng.range(0., 12.)),
                    bounds: bounds.dilate(ScaledPixels(12.)),
                    corner_radii,
                    content_mask: random_mask(&mut rng),
                    color: hsla(0., 0., 0., rng.range(0.2, 0.7)),
                    element_bounds: bounds,
                    element_corner_radii: corner_radii,
                    inset: 0,
                    pad: 0,
                })
            }
            45..52 => {
                let bounds = random_box(&mut rng);
                Operation::Underline(Underline {
                    order: 0,
                    pad: 0,
                    bounds: scaled(
                        bounds.origin.x.0,
                        bounds.origin.y.0,
                        bounds.size.width.0,
                        3.,
                    ),
                    content_mask: random_mask(&mut rng),
                    color: random_color(&mut rng),
                    thickness: ScaledPixels(rng.whole(1., 3.)),
                    wavy: rng.chance(30).into(),
                })
            }
            52..60 => {
                let at = |rng: &mut Rng| {
                    point(
                        px(rng.whole(0., SIDE as f32)),
                        px(rng.whole(0., SIDE as f32)),
                    )
                };
                let mut path = Path::new(at(&mut rng));
                path.line_to(at(&mut rng));
                path.line_to(at(&mut rng));
                path.color = random_color(&mut rng).into();
                let mut path = path.scale(1.);
                path.content_mask = random_mask(&mut rng);
                Operation::Path(path)
            }
            60..68 => {
                let bounds = random_box(&mut rng).dilate(ScaledPixels(20.));
                let quads = (0..rng.below(3) + 1)
                    .map(|ix| {
                        let mut quad = random_quad(&mut rng);
                        let width = bounds.size.width.0 / 3.;
                        quad.bounds = scaled(
                            bounds.origin.x.0 + width * ix as f32,
                            bounds.origin.y.0,
                            (width - 2.).max(1.),
                            bounds.size.height.0,
                        );
                        quad
                    })
                    .collect();
                Operation::Layer(bounds, quads)
            }
            _ => {
                let bounds = random_box(&mut rng);
                let content_mask = random_mask(&mut rng);
                let visible = bounds
                    .intersect(&content_mask.bounds)
                    .dilate(ScaledPixels(1.));
                if natives.iter().any(|placed| placed.intersects(&visible)) {
                    continue;
                }
                natives.push(visible);
                let color = SENTINELS[natives.len() % SENTINELS.len()];
                let corner_radii = random_radii(&mut rng, bounds);
                Operation::Native(
                    NativePlacement {
                        order: 0,
                        id: NativeId(natives.len() as u64),
                        hitbox: None,
                        focus: None,
                        bounds,
                        content_mask,
                        corner_radii,
                        opacity: rng.pick(&[1., 1., 1., 0.75, 0.5, 0.2]),
                    },
                    color,
                )
            }
        };
        operations.push(operation);
    }
    operations
}

/// Builds the scene, with each native placed (`natives_as_quads` false) or
/// drawn as an opaque quad of its sentinel colour instead.
fn build(operations: &[Operation], natives_as_quads: bool) -> Scene {
    let mut scene = Scene::default();
    for operation in operations {
        match operation.clone() {
            Operation::Quad(quad) => scene.insert_primitive(quad),
            Operation::Shadow(shadow) => scene.insert_primitive(shadow),
            Operation::Underline(underline) => scene.insert_primitive(underline),
            Operation::Path(path) => scene.insert_primitive(path),
            Operation::Layer(bounds, quads) => {
                scene.push_layer(bounds);
                for quad in quads {
                    scene.insert_primitive(quad);
                }
                scene.pop_layer();
            }
            Operation::Native(placement, color) => {
                if natives_as_quads {
                    scene.insert_primitive(Quad {
                        order: 0,
                        border_style: BorderStyle::Solid,
                        bounds: placement.bounds,
                        content_mask: placement.content_mask,
                        background: Background::from(color.opacity(placement.opacity)),
                        border_color: black().opacity(0.),
                        corner_radii: placement.corner_radii,
                        border_widths: Edges::default(),
                    });
                } else {
                    scene.insert_native(placement);
                }
            }
        }
    }
    scene.finish();
    scene
}

/// A rendered image: premultiplied RGBA per pixel, row by row, in 0..=1.
struct Image {
    pixels: Vec<[f32; 4]>,
}

impl Image {
    fn from_rgba8(image: &RgbaImage) -> Self {
        Self {
            pixels: image
                .pixels()
                .map(|pixel| pixel.0.map(|channel| f32::from(channel) / 255.))
                .collect(),
        }
    }
}

/// The native covering the pixel at `index`, the topmost if several do, clipped to
/// the whole device pixels its container covers.
fn native_at(operations: &[Operation], index: usize) -> Option<Hsla> {
    let x = (index % SIDE as usize) as f32 + 0.5;
    let y = (index / SIDE as usize) as f32 + 0.5;
    operations
        .iter()
        .rev()
        .find_map(|operation| match operation {
            Operation::Native(placement, color) => {
                let visible = placement.clipped_bounds();
                let covered = x >= visible.origin.x.0.floor()
                    && x < (visible.origin.x.0 + visible.size.width.0).ceil()
                    && y >= visible.origin.y.0.floor()
                    && y < (visible.origin.y.0 + visible.size.height.0).ceil();
                (covered && placement.opacity > 0.).then_some(*color)
            }
            _ => None,
        })
}

/// Composites each native's sentinel colour under `drawable`, as Core Animation
/// composites a layer under GPUI's: `out = G + (1 − αG)·N`.
fn composite(drawable: &Image, operations: &[Operation]) -> Image {
    let pixels = drawable
        .pixels
        .iter()
        .enumerate()
        .map(|(index, pixel)| {
            let Some(native) = native_at(operations, index) else {
                return *pixel;
            };
            let uncovered = 1. - pixel[3];
            let [r, g, b] = sentinel_rgb(native);
            [
                pixel[0] + uncovered * r,
                pixel[1] + uncovered * g,
                pixel[2] + uncovered * b,
                1.,
            ]
        })
        .collect();
    Image { pixels }
}

fn renderer() -> MetalRenderer {
    MetalRenderer::new_headless(std::sync::Arc::new(parking_lot::Mutex::new(
        InstanceBufferPool::default(),
    )))
}

fn render(renderer: &mut MetalRenderer, scene: &Scene) -> RgbaImage {
    renderer
        .render_scene_to_image(scene, size(DevicePixels(SIDE), DevicePixels(SIDE)))
        .expect("a headless render")
}

/// Renders into a half-float target, which keeps what each draw blends to without
/// rounding it to 8 bits, so two ways of drawing the same picture can be compared
/// by their algebra rather than by where they rounded.
fn render_precisely(renderer: &mut MetalRenderer, scene: &Scene) -> Image {
    let side = size(DevicePixels(SIDE), DevicePixels(SIDE));
    objc2::rc::autoreleasepool(|_| {
        renderer.update_path_intermediate_textures(side);
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_width(SIDE as u64);
        descriptor.set_height(SIDE as u64);
        descriptor.set_pixel_format(MTLPixelFormat::RGBA16Float);
        descriptor
            .set_usage(metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead);
        descriptor.set_storage_mode(metal::MTLStorageMode::Shared);
        let texture = renderer.device.new_texture(&descriptor);
        let command_buffer = renderer
            .render_frame(scene, &texture, side)
            .expect("a headless render");
        command_buffer.commit();
        command_buffer.wait_until_completed();
        let mut halves = vec![0u16; SIDE as usize * SIDE as usize * 4];
        texture.get_bytes(
            halves.as_mut_ptr().cast(),
            SIDE as u64 * 8,
            metal::MTLRegion {
                origin: metal::MTLOrigin { x: 0, y: 0, z: 0 },
                size: metal::MTLSize {
                    width: SIDE as u64,
                    height: SIDE as u64,
                    depth: 1,
                },
            },
            0,
        );
        Image {
            pixels: halves
                .chunks_exact(4)
                .map(|pixel| [0, 1, 2, 3].map(|channel| half_to_f32(pixel[channel])))
                .collect(),
        }
    })
}

fn half_to_f32(half: u16) -> f32 {
    let sign = if half & 0x8000 != 0 { -1. } else { 1. };
    let exponent = i32::from((half >> 10) & 0x1f);
    let mantissa = f32::from(half & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        0x1f => f32::INFINITY,
        _ => (1. + mantissa / 1024.) * 2f32.powi(exponent - 15),
    }
}

fn precise_renderer() -> MetalRenderer {
    DRAWABLE_PIXEL_FORMAT.set(Some(MTLPixelFormat::RGBA16Float));
    let renderer = renderer();
    DRAWABLE_PIXEL_FORMAT.set(None);
    renderer
}

/// How far the composite of each seed's scene, drawn by `draw`, is from one pass: the
/// largest difference in any colour channel in 8-bit steps, and how many channels of all
/// the scenes' pixels differ by more than one step. With `eight_bit`, the composite is
/// rounded to 8 bits as the display's is.
fn compare_scenes(eight_bit: bool, mut draw: impl FnMut(&Scene) -> Image) -> (f32, usize, usize) {
    let mut scenes_with_natives = 0;
    let mut worst = 0f32;
    let mut beyond_one_step = 0;
    let mut channels = 0;
    for seed in 0..500 {
        let operations = random_scene(seed);
        let with_holes = build(&operations, false);
        if !with_holes.natives().placements.is_empty() {
            scenes_with_natives += 1;
        }
        let mut composited = composite(&draw(&with_holes), &operations);
        if eight_bit {
            for pixel in &mut composited.pixels {
                *pixel = pixel.map(|channel| (channel * 255.).round() / 255.);
            }
        }
        let one_pass = draw(&build(&operations, true));
        for (a, b) in composited.pixels.iter().zip(&one_pass.pixels) {
            for channel in 0..3 {
                let difference = (a[channel] - b[channel]).abs() * 255.;
                let difference = if eight_bit {
                    difference.round()
                } else {
                    difference
                };
                worst = worst.max(difference);
                beyond_one_step += (difference > 1.) as usize;
                channels += 1;
            }
        }
    }
    assert!(
        scenes_with_natives > 400,
        "{scenes_with_natives} scenes had natives"
    );
    (worst, beyond_one_step, channels)
}

/// The composition's algebra, at a precision where rounding does not hide a mistake.
#[test]
fn holes_with_natives_composited_under_them_match_one_pass_drawing_the_natives() {
    let mut renderer = precise_renderer();
    let (worst, beyond_one_step, channels) =
        compare_scenes(false, |scene| render_precisely(&mut renderer, scene));
    eprintln!("half float: worst {worst:.3} steps, {beyond_one_step} of {channels} beyond one");
    assert!(worst <= 1., "{worst} 8-bit steps apart");
}

/// The same in the drawables' own 8-bit format, where the two ways of drawing round
/// their intermediate results at different points: the drawable holding a hole is
/// rounded before Core Animation composites it, one pass rounds each blend.
#[test]
fn in_the_drawable_format_holes_differ_from_one_pass_by_rounding_alone() {
    let mut renderer = renderer();
    let (worst, beyond_one_step, channels) = compare_scenes(true, |scene| {
        Image::from_rgba8(&render(&mut renderer, scene))
    });
    eprintln!("8-bit: worst {worst:.0} steps, {beyond_one_step} of {channels} beyond one");
    assert!(worst <= 3., "{worst} 8-bit steps apart");
    assert!(
        beyond_one_step * 10_000 < channels,
        "{beyond_one_step} of {channels} channels more than one step apart"
    );
}

#[test]
fn an_opaque_scene_renders_the_same_as_with_the_additive_alpha_blend() {
    let mut renderer = renderer();
    LEGACY_ALPHA_BLEND.set(true);
    let mut legacy = self::renderer();
    LEGACY_ALPHA_BLEND.set(false);
    for seed in 0..100 {
        let operations: Vec<Operation> = random_scene(seed)
            .into_iter()
            .filter(|operation| !matches!(operation, Operation::Native(..)))
            .collect();
        let scene = build(&operations, true);
        let now = render(&mut renderer, &scene);
        let before = render(&mut legacy, &scene);
        assert!(
            now.as_raw() == before.as_raw(),
            "seed {seed}: the blend change moved pixels of an opaque scene"
        );
        assert!(now.pixels().all(|pixel| pixel[3] == 255));
    }
}

/// What a palette over a native costs the GPU, composed as this fork composes (one drawable:
/// the native's hole, then the palette over it) against a second full-window GPUI surface
/// for overlays, as longbridge/gpui-fast#30 (zed#62379) composes: the base surface drawn
/// without the native, and the palette drawn alone on a transparent overlay drawable the
/// size of the window, every frame, open or not. Per frame: CPU time and instructions
/// encoding, and GPU time of the command buffers, at a 14" MacBook Pro's full window.
///
/// `cargo test -p gpui_apple --release --lib composition_overlay_gpu_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, not a check"]
fn composition_overlay_gpu_cost() {
    use objc::{msg_send, sel, sel_impl};
    use std::time::{Duration, Instant};

    const WIDTH: i32 = 3024;
    const HEIGHT: i32 = 1964;
    const FRAMES: usize = 300;
    let window = size(DevicePixels(WIDTH), DevicePixels(HEIGHT));
    let mask = ContentMask {
        bounds: scaled(0., 0., WIDTH as f32, HEIGHT as f32),
    };
    let solid = |bounds: Bounds<ScaledPixels>, color: u32, radius: f32| Quad {
        order: 0,
        border_style: BorderStyle::Solid,
        bounds,
        content_mask: mask,
        background: Background::from(Hsla::from(gpui::rgba(color))),
        border_color: black().opacity(0.),
        corner_radii: Corners::all(ScaledPixels(radius)),
        border_widths: Edges::default(),
    };
    // The workspace: a panel, the navigator's rows and a strip of tiles with headers.
    let base = |scene: &mut Scene| {
        scene.insert_primitive(solid(
            scaled(0., 0., WIDTH as f32, HEIGHT as f32),
            0x18181bff,
            0.,
        ));
        for row in 0..48 {
            let y = 80. + row as f32 * 38.;
            scene.insert_primitive(solid(scaled(16., y, 520., 32.), 0x232328ff, 6.));
        }
        for tile in 0..3 {
            let x = 560. + tile as f32 * 820.;
            scene.insert_primitive(solid(scaled(x, 60., 800., 1880.), 0x1e1e22ff, 12.));
            scene.insert_primitive(solid(scaled(x, 60., 800., 44.), 0x2b2b30ff, 12.));
        }
    };
    let palette = |scene: &mut Scene| {
        let panel = scaled(912., 360., 1200., 880.);
        scene.insert_primitive(Shadow {
            order: 0,
            blur_radius: ScaledPixels(48.),
            bounds: panel.dilate(ScaledPixels(144.)),
            corner_radii: Corners::all(ScaledPixels(20.)),
            content_mask: mask,
            color: hsla(0., 0., 0., 0.45),
            element_bounds: panel,
            element_corner_radii: Corners::all(ScaledPixels(20.)),
            inset: 0,
            pad: 0,
        });
        scene.insert_primitive(solid(panel, 0x26262bff, 20.));
        for row in 0..14 {
            let y = 460. + row as f32 * 54.;
            let color = if row == 0 { 0x3fa66b40 } else { 0x2e2e33ff };
            scene.insert_primitive(solid(scaled(936., y, 1152., 48.), color, 8.));
            scene.insert_primitive(Underline {
                order: 0,
                pad: 0,
                bounds: scaled(952., y + 47., 1120., 1.),
                content_mask: mask,
                color: hsla(0., 0., 1., 0.06),
                thickness: ScaledPixels(1.),
                wavy: false.into(),
            });
        }
    };
    let native = |bounds: Bounds<ScaledPixels>| NativePlacement {
        order: 0,
        id: NativeId(1),
        hitbox: None,
        focus: None,
        bounds,
        content_mask: mask,
        corner_radii: Corners::all(ScaledPixels(12.)),
        opacity: 1.,
    };
    let scene_of = |parts: &[&dyn Fn(&mut Scene)]| {
        let mut scene = Scene::default();
        for part in parts {
            part(&mut scene);
        }
        scene.finish();
        scene
    };

    let mut renderer = renderer();
    renderer.update_path_intermediate_textures(window);
    let target = || {
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_width(WIDTH as u64);
        descriptor.set_height(HEIGHT as u64);
        descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        descriptor
            .set_usage(metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead);
        descriptor.set_storage_mode(metal::MTLStorageMode::Private);
        renderer.device.new_texture(&descriptor)
    };
    let (base_target, overlay_target) = (target(), target());
    fn instructions() -> u64 {
        unsafe extern "C" {
            // libsystem_kernel: the thread's performance counters; kind 1 is
            // instructions and cycles.
            fn thread_selfcounts(kind: i32, buffer: *mut u64, size: usize) -> i32;
        }
        let mut counts = [0u64; 2];
        // SAFETY: the buffer holds the two counters kind 1 writes.
        unsafe { thread_selfcounts(1, counts.as_mut_ptr(), size_of_val(&counts)) };
        counts[0]
    }
    // Encodes each (scene, target) pass into its own command buffer, as each surface's
    // renderer does, and returns the CPU time and instructions encoding and the GPU time
    // of all of them.
    let mut frame = |passes: &[(&Scene, &metal::Texture)]| -> (Duration, u64, Duration) {
        objc2::rc::autoreleasepool(|_| {
            let start = Instant::now();
            let before = instructions();
            let buffers: Vec<_> = passes
                .iter()
                .map(|(scene, target)| {
                    let buffer = renderer
                        .render_frame(scene, target, window)
                        .expect("frame encoded");
                    buffer.commit();
                    buffer
                })
                .collect();
            let (cpu, encoded) = (start.elapsed(), instructions() - before);
            let mut gpu = Duration::ZERO;
            for buffer in &buffers {
                buffer.wait_until_completed();
                // SAFETY: `GPUStartTime` and `GPUEndTime` are `MTLCommandBuffer`
                // properties, read after the buffer completed.
                let (start, end): (f64, f64) = unsafe {
                    (
                        msg_send![buffer.as_ref(), GPUStartTime],
                        msg_send![buffer.as_ref(), GPUEndTime],
                    )
                };
                gpu += Duration::from_secs_f64((end - start).max(0.));
            }
            (cpu, encoded, gpu)
        })
    };
    let mut measure = |name: &str, passes: &[(&Scene, &metal::Texture)]| {
        let mut samples: Vec<(Duration, u64, Duration)> =
            (0..FRAMES + 30).map(|_| frame(passes)).skip(30).collect();
        let median = |samples: &mut Vec<(Duration, u64, Duration)>,
                      key: fn(&(Duration, u64, Duration)) -> u128| {
            samples.sort_by_key(key);
            samples[samples.len() / 2]
        };
        let cpu = median(&mut samples, |s| s.0.as_nanos()).0;
        let encoded = median(&mut samples, |s| u128::from(s.1)).1;
        let gpu = median(&mut samples, |s| s.2.as_nanos()).2;
        eprintln!("{name}: encode {cpu:?} ({encoded} instructions), gpu {gpu:?}");
    };

    for (what, bounds) in [
        ("browser tile", scaled(1380., 104., 800., 1836.)),
        ("remote screen", scaled(560., 104., 2440., 1836.)),
    ] {
        let hole = |scene: &mut Scene| scene.insert_native(native(bounds));
        let ours_open = scene_of(&[&base, &hole, &palette]);
        let ours_closed = scene_of(&[&base, &hole]);
        let base_only = scene_of(&[&base]);
        let palette_only = scene_of(&[&palette]);
        let empty = scene_of(&[]);
        eprintln!("-- palette over a {what}");
        measure("ours, palette open", &[(&ours_open, &base_target)]);
        measure(
            "overlay surface, palette open",
            &[(&base_only, &base_target), (&palette_only, &overlay_target)],
        );
        measure("ours, palette closed", &[(&ours_closed, &base_target)]);
        measure(
            "overlay surface, palette closed",
            &[(&base_only, &base_target), (&empty, &overlay_target)],
        );
    }
    let drawable = u64::from(WIDTH.unsigned_abs()) * u64::from(HEIGHT.unsigned_abs()) * 4;
    eprintln!(
        "an overlay surface's CAMetalLayer: up to 3 drawables of {:.1} MB, {:.1} MB",
        drawable as f64 / 1e6,
        3. * drawable as f64 / 1e6
    );
}

/// Culling what opaque natives cover whole (`fast::occlusion`) leaves every pixel as
/// upstream's loop, which draws everything and cuts the holes, leaves it.
#[test]
fn culling_under_opaque_natives_leaves_every_pixel_as_drawing_everything() {
    use crate::metal_renderer::binds::UPSTREAM_LOOP;

    let mut renderer = renderer();
    let mut compared = 0;
    for seed in 0..500 {
        let operations: Vec<Operation> = random_scene(seed)
            .into_iter()
            .filter(|operation| !matches!(operation, Operation::Path(..)))
            .collect();
        let scene = build(&operations, false);
        if scene.natives().placements.is_empty() {
            continue;
        }
        compared += 1;
        UPSTREAM_LOOP.with(|flag| flag.set(true));
        let everything = render(&mut renderer, &scene);
        UPSTREAM_LOOP.with(|flag| flag.set(false));
        let culled = render(&mut renderer, &scene);
        assert!(
            culled.as_raw() == everything.as_raw(),
            "seed {seed}: culling under natives moved pixels"
        );
    }
    assert!(compared > 100, "only {compared} scenes placed natives");
}
