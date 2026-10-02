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
        let fade = scene.add_edge_fade(ramps());
        draw(&mut scene, fade, &renderer);
        scene.finish();
        let pixels = renderer
            .render_scene_to_image(&scene, size(DevicePixels(SIDE), DevicePixels(SIDE)))
            .expect("scene rendered")
            .into_raw();
        let mut worst = 0;
        for y in 4..SIDE as usize - 4 {
            for x in 4..SIDE as usize - 4 {
                let alpha = ramps().alpha_at(point(
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
}
