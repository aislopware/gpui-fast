//! Edge fades on Metal: the scene's table of fades
//! ([`gpui::Scene::edge_fades`]) uploaded with the frame's instances, and
//! bound for the fragment shaders that read it (`edge_fade` in
//! `shaders.metal`).

use anyhow::Result;
use gpui::{EdgeFadeRamps, Scene};
use metal::RenderCommandEncoderRef;

use crate::metal_renderer::{InstanceBinding, InstanceBufferWriter};

/// The fragment buffer the shaders read the table from
/// (`EDGE_FADE_BUFFER` in `shaders.metal`).
pub(crate) const BUFFER: u64 = 7;

/// A table of no entries: its entry 0 counts none, so every fade draws
/// whole.
const EMPTY: [EdgeFadeRamps; 1] = [EdgeFadeRamps {
    edge: [0.; 4],
    rate: [0.; 4],
    start: [0.; 4],
    clip: [0.; 4],
    radii: [0.; 4],
    mask: [0.; 4],
    mask_shape: [0.; 4],
    mask_bottom: [0.; 4],
}];

/// Writes `scene`'s table of fades into the frame's instances.
pub(crate) fn write(scene: &Scene, writer: &mut InstanceBufferWriter) -> Result<InstanceBinding> {
    let table = scene.edge_fades();
    writer.write(if table.is_empty() { &EMPTY } else { table })
}

/// Binds `fades` for the fragment shaders of `encoder`'s draws.
pub(crate) fn bind(encoder: &RenderCommandEncoderRef, fades: &InstanceBinding) {
    encoder.set_fragment_buffer(BUFFER, Some(&fades.buffer), fades.offset as u64);
}

/// Pixel tests: what each kind of primitive draws faded, read back from a
/// headless renderer, against the ramps as `gpui` works them out.
#[cfg(test)]
mod tests {
    use std::{borrow::Cow, sync::Arc};

    use gpui::{
        AtlasKey, Bounds, ContentMask, DevicePixels, EdgeFadeRamps, Edges, Hsla, ImageId,
        MonochromeSprite, Path, PlatformAtlas as _, PolychromeSprite, Quad, RenderImageParams,
        RenderSvgParams, ScaledPixels, Scene, TransformationMatrix, Underline, point, px, rgba,
        size,
    };
    use parking_lot::Mutex;

    use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

    const SIDE: i32 = 64;

    fn sp(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(width), ScaledPixels(height)),
        }
    }

    fn everywhere() -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: sp(0., 0., SIDE as f32, SIDE as f32),
        }
    }

    fn white() -> Hsla {
        Hsla::from(rgba(0xffffffff))
    }

    fn quad(bounds: Bounds<ScaledPixels>, color: Hsla) -> Quad {
        Quad {
            bounds,
            content_mask: everywhere(),
            background: color.into(),
            ..Default::default()
        }
    }

    /// The fade the tests draw in: every edge of the target fading over 16
    /// device pixels, the left one half deep.
    fn ramps() -> EdgeFadeRamps {
        EdgeFadeRamps::new(
            sp(0., 0., SIDE as f32, SIDE as f32),
            Edges::all(ScaledPixels(16.)),
            Edges {
                left: 0.5,
                ..Edges::all(1.)
            },
        )
    }

    /// Draws white faded by `draw` over black, and requires every pixel to
    /// be as bright as the fade at its centre says, to within the rounding
    /// of 8 bits.
    fn renders_faded(what: &str, draw: impl FnOnce(&mut Scene, u32, &MetalRenderer)) {
        renders_in(what, ramps(), draw);
    }

    /// The rounded clip the tests draw in: corners of 12, 4, 20 and 0
    /// device pixels around a rectangle inside the target.
    fn clip() -> EdgeFadeRamps {
        clipped(EdgeFadeRamps::NONE)
    }

    /// `ramps` clipping as [`clip`] does.
    fn clipped(ramps: EdgeFadeRamps) -> EdgeFadeRamps {
        ramps.rounded(
            sp(6., 9., 50., 44.),
            gpui::Corners {
                top_left: ScaledPixels(12.),
                top_right: ScaledPixels(4.),
                bottom_right: ScaledPixels(20.),
                bottom_left: ScaledPixels(0.),
            },
        )
    }

    /// Draws white in `ramps` over black, and requires every pixel to be as
    /// bright as the ramps at its centre say, to within the rounding of 8
    /// bits.
    fn renders_in(
        what: &str,
        ramps: EdgeFadeRamps,
        draw: impl FnOnce(&mut Scene, u32, &MetalRenderer),
    ) {
        if metal::Device::system_default().is_none() {
            eprintln!("skipped: no Metal device");
            return;
        }
        let mut renderer =
            MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())));
        let mut scene = Scene::default();
        scene.insert_primitive(quad(
            sp(0., 0., SIDE as f32, SIDE as f32),
            Hsla::from(rgba(0x000000ff)),
        ));
        let fade = scene.add_edge_fade(ramps);
        draw(&mut scene, fade, &renderer);
        scene.finish();
        let pixels = renderer
            .render_scene_to_image(&scene, size(DevicePixels(SIDE), DevicePixels(SIDE)))
            .expect("scene rendered")
            .into_raw();
        let mut worst = 0;
        for y in 4..SIDE as usize - 4 {
            for x in 4..SIDE as usize - 4 {
                let alpha = ramps.alpha_at(point(
                    ScaledPixels(x as f32 + 0.5),
                    ScaledPixels(y as f32 + 0.5),
                ));
                let expected = (alpha * 255.).round() as i32;
                let actual = i32::from(pixels[(y * SIDE as usize + x) * 4]);
                worst = worst.max((actual - expected).abs());
                assert!(
                    (actual - expected).abs() <= 2,
                    "{what} at ({x}, {y}): {actual}, the fade says {expected}"
                );
            }
        }
        eprintln!("{what}: within {worst} of 255 levels");
    }

    #[test]
    fn a_faded_quad_draws_its_fade() {
        renders_faded("quad", |scene, fade, _| {
            scene.insert_faded_primitive(quad(sp(0., 0., SIDE as f32, SIDE as f32), white()), fade);
        });
    }

    #[test]
    fn a_faded_rounded_bordered_quad_draws_its_fade() {
        renders_faded("rounded quad", |scene, fade, _| {
            let mut quad = quad(sp(-8., -8., SIDE as f32 + 16., SIDE as f32 + 16.), white());
            quad.corner_radii = gpui::Corners::all(ScaledPixels(4.));
            quad.border_widths = Edges::all(ScaledPixels(2.));
            quad.border_color = white();
            scene.insert_faded_primitive(quad, fade);
        });
    }

    #[test]
    fn a_faded_glyph_draws_its_fade() {
        renders_faded("glyph", |scene, fade, renderer| {
            let tile = renderer
                .sprite_atlas()
                .get_or_insert_with(
                    AtlasKey::Svg(RenderSvgParams {
                        path: "a solid glyph".into(),
                        size: size(DevicePixels(SIDE), DevicePixels(SIDE)),
                    }),
                    &mut || {
                        Ok(Some((
                            size(DevicePixels(SIDE), DevicePixels(SIDE)),
                            Cow::Owned(vec![255; (SIDE * SIDE) as usize]),
                        )))
                    },
                )
                .expect("glyph uploaded")
                .expect("glyph tile");
            scene.insert_faded_primitive(
                MonochromeSprite {
                    order: 0,
                    pad: 0,
                    bounds: sp(0., 0., SIDE as f32, SIDE as f32),
                    content_mask: everywhere(),
                    color: white(),
                    tile,
                    transformation: TransformationMatrix::unit(),
                },
                fade,
            );
        });
    }

    #[test]
    fn a_faded_image_draws_its_fade() {
        renders_faded("image", |scene, fade, renderer| {
            let tile = renderer
                .sprite_atlas()
                .get_or_insert_with(
                    AtlasKey::Image(RenderImageParams {
                        image_id: ImageId(7_777),
                        frame_index: 0,
                    }),
                    &mut || {
                        Ok(Some((
                            size(DevicePixels(SIDE), DevicePixels(SIDE)),
                            Cow::Owned(vec![255; (SIDE * SIDE * 4) as usize]),
                        )))
                    },
                )
                .expect("image uploaded")
                .expect("image tile");
            scene.insert_faded_primitive(
                PolychromeSprite {
                    order: 0,
                    pad: 0,
                    grayscale: false.into(),
                    opacity: 1.,
                    bounds: sp(0., 0., SIDE as f32, SIDE as f32),
                    content_mask: everywhere(),
                    corner_radii: Default::default(),
                    tile,
                },
                fade,
            );
        });
    }

    #[test]
    fn a_faded_underline_draws_its_fade() {
        renders_faded("underline", |scene, fade, _| {
            scene.insert_faded_primitive(
                Underline {
                    order: 0,
                    pad: 0,
                    bounds: sp(0., 0., SIDE as f32, SIDE as f32),
                    content_mask: everywhere(),
                    color: white(),
                    thickness: ScaledPixels(SIDE as f32),
                    wavy: false.into(),
                },
                fade,
            );
        });
    }

    #[test]
    fn a_faded_path_draws_its_fade() {
        renders_faded("path", |scene, fade, _| {
            let side = px(SIDE as f32);
            let mut path = Path::new(point(px(0.), px(0.)));
            path.line_to(point(side, px(0.)));
            path.line_to(point(side, side));
            path.line_to(point(px(0.), side));
            path.line_to(point(px(0.), px(0.)));
            path.color = white().into();
            path.content_mask = ContentMask {
                bounds: Bounds::new(point(px(0.), px(0.)), size(side, side)),
            };
            scene.insert_faded_primitive(path.scale(1.), fade);
        });
    }

    #[test]
    fn a_rounded_clip_cuts_a_quad_along_its_corners() {
        assert!(clip().clips() && !clip().rate.iter().any(|rate| *rate != 0.));
        renders_in("clipped quad", clip(), |scene, fade, _| {
            scene.insert_faded_primitive(quad(sp(0., 0., SIDE as f32, SIDE as f32), white()), fade);
        });
    }

    #[test]
    fn a_rounded_clip_inside_a_fade_cuts_and_fades_each_kind() {
        let both = clipped(ramps());
        assert!(both.clips() && both.rate.iter().any(|rate| *rate != 0.));
        renders_in("clipped faded quad", both, |scene, fade, _| {
            let mut quad = quad(sp(0., 0., SIDE as f32, SIDE as f32), white());
            quad.corner_radii = gpui::Corners::all(ScaledPixels(2.));
            scene.insert_faded_primitive(quad, fade);
        });
        renders_in("clipped faded underline", both, |scene, fade, _| {
            scene.insert_faded_primitive(
                Underline {
                    order: 0,
                    pad: 0,
                    bounds: sp(0., 0., SIDE as f32, SIDE as f32),
                    content_mask: everywhere(),
                    color: white(),
                    thickness: ScaledPixels(SIDE as f32),
                    wavy: false.into(),
                },
                fade,
            );
        });
        renders_in("clipped faded path", both, |scene, fade, _| {
            let side = px(SIDE as f32);
            let mut path = Path::new(point(px(0.), px(0.)));
            path.line_to(point(side, px(0.)));
            path.line_to(point(side, side));
            path.line_to(point(px(0.), side));
            path.line_to(point(px(0.), px(0.)));
            path.color = white().into();
            path.content_mask = ContentMask {
                bounds: Bounds::new(point(px(0.), px(0.)), size(side, side)),
            };
            scene.insert_faded_primitive(path.scale(1.), fade);
        });
    }

    /// The clip's arcs are antialiased: a pixel the arc crosses is drawn in
    /// part, one well inside whole, one past it not at all.
    #[test]
    fn a_rounded_clips_corner_is_antialiased() {
        let clip = clip();
        let at = |x: f32, y: f32| clip.coverage_at(point(ScaledPixels(x), ScaledPixels(y)));
        assert_eq!(at(30., 30.), 1.);
        assert_eq!(at(6.5, 9.5), 0.);
        assert_eq!(at(6.5, 52.5), 1., "the square corner");
        assert!((0..12).any(|i| {
            let coverage = at(6. + i as f32 + 0.5, 9. + i as f32 + 0.5);
            coverage > 0. && coverage < 1.
        }));
    }

    /// The image alpha mask the tests draw in: a rounded rectangle inside
    /// the target feathered out over 12 device pixels past a clearance of 2,
    /// and the lower part faded over 30.
    fn masked(mut ramps: EdgeFadeRamps) -> EdgeFadeRamps {
        ramps.mask = [16., 20., 46., 40.];
        ramps.mask_shape = [6., 12., 2., 30.];
        ramps.mask_bottom = [60., 0., 0., 0.];
        ramps
    }

    /// An image drawn with an alpha mask (`gpui::ImageAlphaMask`) is as
    /// strong at each pixel as the mask says, alone and inside a fade and a
    /// rounded clip; a mask of no feather masks nothing.
    #[test]
    fn an_image_alpha_mask_feathers_the_image() {
        let image = |scene: &mut Scene, fade: u32, renderer: &MetalRenderer| {
            let tile = renderer
                .sprite_atlas()
                .get_or_insert_with(
                    AtlasKey::Image(RenderImageParams {
                        image_id: ImageId(7_778),
                        frame_index: 0,
                    }),
                    &mut || {
                        Ok(Some((
                            size(DevicePixels(SIDE), DevicePixels(SIDE)),
                            Cow::Owned(vec![255; (SIDE * SIDE * 4) as usize]),
                        )))
                    },
                )
                .expect("image uploaded")
                .expect("image tile");
            scene.insert_faded_primitive(
                PolychromeSprite {
                    order: 0,
                    pad: 0,
                    grayscale: false.into(),
                    opacity: 1.,
                    bounds: sp(0., 0., SIDE as f32, SIDE as f32),
                    content_mask: everywhere(),
                    corner_radii: Default::default(),
                    tile,
                },
                fade,
            );
        };
        let mask = masked(EdgeFadeRamps::NONE);
        assert!(mask.masks() && mask.fades() && !mask.clips());
        let at = |x: f32, y: f32| mask.mask_at(point(ScaledPixels(x), ScaledPixels(y)));
        assert_eq!(at(30., 30.), 0., "inside the exclusion");
        assert_eq!(at(31., 21.), 0., "inside the clearance");
        // 8 of the feather's 12 out: smoothstep of 2/3.
        assert!((at(31., 10.) - 20. / 27.).abs() < 1e-5, "{}", at(31., 10.));
        assert_eq!(at(31., 2.), 1., "past the feather");
        assert_eq!(at(2., 60.5), 0., "past the bottom fade");
        renders_in("masked image", mask, image);
        renders_in(
            "masked image in a fade and a clip",
            masked(clipped(ramps())),
            image,
        );
        let mut unmasked = mask;
        unmasked.mask_shape[1] = 0.;
        assert!(!unmasked.fades());
    }

    /// No fade, or an index past the table's end, draws a primitive whole.
    #[test]
    fn an_unfaded_or_unknown_index_draws_whole() {
        if metal::Device::system_default().is_none() {
            return;
        }
        let mut renderer =
            MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())));
        for fade in [0, 999, 1 | 7 << 16] {
            let mut scene = Scene::default();
            if fade != 0 {
                scene.add_edge_fade(EdgeFadeRamps::NONE);
            }
            scene.insert_faded_primitive(quad(sp(0., 0., SIDE as f32, SIDE as f32), white()), fade);
            scene.finish();
            let pixels = renderer
                .render_scene_to_image(&scene, size(DevicePixels(SIDE), DevicePixels(SIDE)))
                .expect("scene rendered")
                .into_raw();
            assert!(pixels.chunks(4).all(|pixel| pixel[0] == 255), "fade {fade}");
        }
    }

    /// What a window-sized image costs the GPU masked as zeron's hero is,
    /// against unmasked: a 3024 x 1964 target, medians of 300 frames.
    ///
    /// `cargo test -p gpui_apple --release --lib image_mask_gpu_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check"]
    fn image_mask_gpu_cost() {
        use std::time::Duration;

        const W: i32 = 3024;
        const H: i32 = 1964;
        if metal::Device::system_default().is_none() {
            return;
        }
        let mut renderer =
            MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())));
        let tile = renderer
            .sprite_atlas()
            .get_or_insert_with(
                AtlasKey::Image(RenderImageParams {
                    image_id: ImageId(7_779),
                    frame_index: 0,
                }),
                &mut || {
                    Ok(Some((
                        size(DevicePixels(1024), DevicePixels(1024)),
                        Cow::Owned(vec![200; 1024 * 1024 * 4]),
                    )))
                },
            )
            .expect("image uploaded")
            .expect("image tile");
        let target = {
            let descriptor = metal::TextureDescriptor::new();
            descriptor.set_width(W as u64);
            descriptor.set_height(H as u64);
            descriptor.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
            descriptor.set_usage(
                metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead,
            );
            descriptor.set_storage_mode(metal::MTLStorageMode::Private);
            renderer.device.new_texture(&descriptor)
        };
        let mut ramps = EdgeFadeRamps::NONE;
        ramps.mask = [400., 1400., 2600., 1800.];
        ramps.mask_shape = [48., 560., 16., 1964.];
        ramps.mask_bottom = [1964., 0., 0., 0.];
        let faded = EdgeFadeRamps::new(
            sp(0., 0., W as f32, H as f32),
            Edges::all(ScaledPixels(64.)),
            Edges::all(1.),
        );
        for (name, ramps) in [
            ("unmasked", None),
            ("edges faded", Some(faded)),
            ("masked", Some(ramps)),
        ] {
            let mut gpu = Vec::new();
            for frame in 0..330 {
                let mut scene = Scene::default();
                let fade = ramps.map_or(0, |ramps| scene.add_edge_fade(ramps));
                let mask = ContentMask {
                    bounds: sp(0., 0., W as f32, H as f32),
                };
                scene.insert_faded_primitive(
                    PolychromeSprite {
                        order: 0,
                        pad: 0,
                        grayscale: false.into(),
                        opacity: 1.,
                        bounds: mask.bounds,
                        content_mask: mask,
                        corner_radii: Default::default(),
                        tile,
                    },
                    fade,
                );
                scene.finish();
                let sample = objc2::rc::autoreleasepool(|_| {
                    let buffer = renderer
                        .render_frame(&scene, &target, size(DevicePixels(W), DevicePixels(H)))
                        .expect("frame encoded");
                    buffer.commit();
                    buffer.wait_until_completed();
                    // SAFETY: `GPUStartTime` and `GPUEndTime` are `MTLCommandBuffer`
                    // properties, read after the buffer completed.
                    let (start, end): (f64, f64) = unsafe {
                        let buffer: &objc2::runtime::AnyObject =
                            &*foreign_types::ForeignType::as_ptr(&buffer).cast();
                        (
                            objc2::msg_send![buffer, GPUStartTime],
                            objc2::msg_send![buffer, GPUEndTime],
                        )
                    };
                    Duration::from_secs_f64((end - start).max(0.))
                });
                if frame >= 30 {
                    gpu.push(sample);
                }
            }
            gpu.sort();
            eprintln!(
                "{name}: gpu median {:?} p95 {:?}",
                gpu[gpu.len() / 2],
                gpu[gpu.len() * 95 / 100]
            );
        }
    }
}
