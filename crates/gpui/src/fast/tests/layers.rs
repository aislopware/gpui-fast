//! Tests of scroll layers.

use crate::{
    AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, ContentMask, DevicePixels, Hsla,
    LayerFrame, LayerKey, MonochromeSprite, Path, Pixels, Point, PolychromeSprite, Quad,
    ScaledPixels, Scene, Shadow, SubpixelSprite, TileCoord, TileId, TransformationMatrix,
    Underline, decode_layer_tile, fast::layers::scene::translate_primitive, layer_tile_id,
    layer_tile_texture_id, point, px, scene::Primitive, size,
};
use std::rc::Rc;

#[test]
fn layer_tile_ids_round_trip_and_never_collide_with_the_atlas() {
    for layer in [0, 1, 77, 0x00FF_FFFF] {
        for tile in [(0, 0), (-1, 3), (2047, -2048), (-2048, 2047)] {
            let key = LayerKey(layer);
            let coord = TileCoord {
                x: tile.0,
                y: tile.1,
            };
            let texture = layer_tile_texture_id(key);
            assert_eq!(texture.kind, AtlasTextureKind::Polychrome);
            assert!(texture.index >= crate::LAYER_TILE_TEXTURE_BASE);
            assert_eq!(
                decode_layer_tile(texture, layer_tile_id(coord)),
                Some((key, coord))
            );
        }
    }
    let atlas = AtlasTextureId {
        index: 3,
        kind: AtlasTextureKind::Polychrome,
    };
    assert_eq!(
        decode_layer_tile(atlas, layer_tile_id(TileCoord { x: 0, y: 0 })),
        None
    );
    let mono = AtlasTextureId {
        index: crate::LAYER_TILE_TEXTURE_BASE,
        kind: AtlasTextureKind::Monochrome,
    };
    assert_eq!(
        decode_layer_tile(mono, layer_tile_id(TileCoord { x: 0, y: 0 })),
        None
    );
}

fn sp(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(w), ScaledPixels(h)),
    }
}

fn wide_mask() -> ContentMask<ScaledPixels> {
    ContentMask {
        bounds: sp(-10_000., -10_000., 20_000., 20_000.),
    }
}

fn quad(bounds: Bounds<ScaledPixels>) -> Quad {
    Quad {
        bounds,
        content_mask: wide_mask(),
        background: Hsla::red().into(),
        ..Default::default()
    }
}

fn layer(content: Scene) -> LayerFrame {
    LayerFrame {
        key: LayerKey(1),
        generation: 1,
        background: crate::rgba(0xffffffff),
        tile_size: 512,
        content: Rc::new(content),
        dirty_tiles: Vec::new(),
    }
}

#[test]
fn a_tile_scene_holds_the_primitives_over_the_tile_in_its_own_space() {
    let mut content = Scene::default();
    content.insert_primitive(quad(sp(10., 10., 20., 20.))); // tile (0,0) only
    content.insert_primitive(quad(sp(500., 100., 40., 10.))); // tiles (0,0) and (1,0)
    content.insert_primitive(quad(sp(600., 700., 10., 10.))); // tile (1,1) only
    content.insert_primitive(quad(sp(-30., -30., 10., 10.))); // tile (-1,-1) only
    content.finish();
    let layer = layer(content);

    assert_eq!(
        layer.tile_bounds(TileCoord { x: -1, y: 2 }),
        sp(-512., 1024., 512., 512.)
    );

    let t10 = layer.tile_scene(TileCoord { x: 1, y: 0 });
    assert_eq!(t10.quads.len(), 1);
    assert_eq!(t10.quads[0].bounds, sp(500. - 512., 100., 40., 10.));
    assert_eq!(
        t10.quads[0].content_mask.bounds.origin,
        point(ScaledPixels(-10_000. - 512.), ScaledPixels(-10_000.))
    );

    let t00 = layer.tile_scene(TileCoord { x: 0, y: 0 });
    assert_eq!(t00.quads.len(), 2);
    // The two do not overlap, so they may share a draw order; the first
    // painted still sorts first.
    assert!(
        t00.quads[0].order <= t00.quads[1].order,
        "drawing order kept"
    );
    assert_eq!(t00.quads[0].bounds, sp(10., 10., 20., 20.));
    assert_eq!(t00.quads[1].bounds, sp(500., 100., 40., 10.));

    let t11 = layer.tile_scene(TileCoord { x: 1, y: 1 });
    assert_eq!(t11.quads.len(), 1);
    assert_eq!(t11.quads[0].bounds, sp(600. - 512., 700. - 512., 10., 10.));

    let tneg = layer.tile_scene(TileCoord { x: -1, y: -1 });
    assert_eq!(tneg.quads.len(), 1);
    assert_eq!(tneg.quads[0].bounds, sp(-30. + 512., -30. + 512., 10., 10.));

    assert!(layer.tile_scene(TileCoord { x: 3, y: 3 }).quads.is_empty());
}

#[test]
fn a_tile_scene_leaves_out_primitives_masked_off_the_tile() {
    let mut content = Scene::default();
    // Spans tiles (0,0) and (1,0), but its mask keeps it inside tile (0,0).
    content.insert_primitive(Quad {
        content_mask: ContentMask {
            bounds: sp(0., 0., 512., 512.),
        },
        ..quad(sp(500., 100., 40., 10.))
    });
    content.finish();
    let layer = layer(content);
    assert_eq!(layer.tile_scene(TileCoord { x: 0, y: 0 }).quads.len(), 1);
    assert!(layer.tile_scene(TileCoord { x: 1, y: 0 }).quads.is_empty());
}

#[test]
fn a_tile_scene_keeps_layers_sharing_one_draw_order() {
    let mut content = Scene::default();
    content.push_layer(sp(0., 0., 1024., 100.));
    content.insert_primitive(quad(sp(10., 10., 10., 10.)));
    content.insert_primitive(quad(sp(600., 10., 10., 10.)));
    content.pop_layer();
    content.insert_primitive(quad(sp(20., 20., 10., 10.)));
    content.finish();
    let layer = layer(content);

    let t00 = layer.tile_scene(TileCoord { x: 0, y: 0 });
    assert_eq!(t00.quads.len(), 2);
    assert!(t00.quads[0].order < t00.quads[1].order);
    let t10 = layer.tile_scene(TileCoord { x: 1, y: 0 });
    assert_eq!(t10.quads.len(), 1);
    assert_eq!(t10.quads[0].bounds, sp(600. - 512., 10., 10., 10.));
}

fn atlas_tile() -> AtlasTile {
    AtlasTile {
        texture_id: AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Monochrome,
        },
        tile_id: TileId(9),
        padding: 0,
        bounds: Bounds {
            origin: point(DevicePixels(0), DevicePixels(0)),
            size: size(DevicePixels(4), DevicePixels(4)),
        },
    }
}

#[test]
fn translation_moves_every_position_a_primitive_carries() {
    let delta = point(ScaledPixels(7.), ScaledPixels(-3.));
    let mask = ContentMask {
        bounds: sp(0., 0., 100., 100.),
    };
    let moved_mask = sp(7., -3., 100., 100.);
    let translate = |primitive: Primitive| translate_primitive(&primitive, delta);

    let shadow = Shadow {
        order: 0,
        blur_radius: ScaledPixels(2.),
        bounds: sp(1., 2., 3., 4.),
        corner_radii: Default::default(),
        content_mask: mask.clone(),
        color: Hsla::red(),
        element_bounds: sp(5., 6., 7., 8.),
        element_corner_radii: Default::default(),
        inset: 0,
        pad: 0,
    };
    let Primitive::Shadow(moved) = translate(shadow.into()) else {
        unreachable!()
    };
    assert_eq!(moved.bounds, sp(8., -1., 3., 4.));
    assert_eq!(moved.element_bounds, sp(12., 3., 7., 8.));
    assert_eq!(moved.content_mask.bounds, moved_mask);
    assert_eq!(moved.blur_radius, ScaledPixels(2.));

    let Primitive::Quad(moved) = translate(
        Quad {
            content_mask: mask.clone(),
            ..quad(sp(1., 2., 3., 4.))
        }
        .into(),
    ) else {
        unreachable!()
    };
    assert_eq!(moved.bounds, sp(8., -1., 3., 4.));
    assert_eq!(moved.content_mask.bounds, moved_mask);

    let mut path = Path::new(point(px(10.), px(20.)));
    path.push_triangle(
        (
            point(px(10.), px(20.)),
            point(px(30.), px(20.)),
            point(px(10.), px(40.)),
        ),
        (point(0., 1.), point(0., 1.), point(0., 1.)),
    );
    let mut path = path.scale(1.);
    path.content_mask = mask.clone();
    for vertex in &mut path.vertices {
        vertex.content_mask = mask.clone();
    }
    let bounds = path.bounds;
    let Primitive::Path(moved) = translate(path.into()) else {
        unreachable!()
    };
    assert_eq!(moved.bounds.origin, bounds.origin + delta);
    assert_eq!(moved.bounds.size, bounds.size);
    assert_eq!(moved.content_mask.bounds, moved_mask);
    let positions: Vec<_> = moved.vertices.iter().map(|v| v.xy_position).collect();
    assert_eq!(
        positions,
        vec![
            point(ScaledPixels(17.), ScaledPixels(17.)),
            point(ScaledPixels(37.), ScaledPixels(17.)),
            point(ScaledPixels(17.), ScaledPixels(37.)),
        ]
    );
    assert!(
        moved
            .vertices
            .iter()
            .all(|v| v.content_mask.bounds == moved_mask)
    );
    assert!(
        moved
            .vertices
            .iter()
            .all(|v| v.st_position == point(0., 1.))
    );

    let underline = Underline {
        order: 0,
        pad: 0,
        bounds: sp(1., 2., 3., 4.),
        content_mask: mask.clone(),
        color: Hsla::red(),
        thickness: ScaledPixels(1.),
        wavy: true.into(),
    };
    let Primitive::Underline(moved) = translate(underline.into()) else {
        unreachable!()
    };
    assert_eq!(moved.bounds, sp(8., -1., 3., 4.));
    assert_eq!(moved.content_mask.bounds, moved_mask);

    // A transformation applies to window positions, `R·p + t`, so moving the
    // sprite by `delta` moves `t` to `t + (I − R)·delta`.
    let rotation = TransformationMatrix {
        rotation_scale: [[0., -1.], [1., 0.]],
        translation: [5., 6.],
    };
    let expected = TransformationMatrix {
        rotation_scale: [[0., -1.], [1., 0.]],
        // (I − R)·(7, −3) = (7, −3) − (3, 7) = (4, −10)
        translation: [5. + 4., 6. - 10.],
    };
    let mono = MonochromeSprite {
        order: 0,
        pad: 0,
        bounds: sp(1., 2., 3., 4.),
        content_mask: mask.clone(),
        color: Hsla::red(),
        tile: atlas_tile(),
        transformation: rotation,
    };
    let Primitive::MonochromeSprite(moved) = translate(mono.into()) else {
        unreachable!()
    };
    assert_eq!(moved.bounds, sp(8., -1., 3., 4.));
    assert_eq!(moved.content_mask.bounds, moved_mask);
    assert_eq!(moved.transformation, expected);
    assert_eq!(moved.tile, atlas_tile());
    let window_position = |m: &TransformationMatrix, p: Point<Pixels>| m.apply(p);
    assert_eq!(
        window_position(&moved.transformation, point(px(8.), px(-1.))),
        window_position(&rotation, point(px(1.), px(2.))) + point(px(7.), px(-3.)),
    );

    let unit = MonochromeSprite {
        transformation: TransformationMatrix::unit(),
        ..mono
    };
    let Primitive::MonochromeSprite(moved) = translate(unit.into()) else {
        unreachable!()
    };
    assert_eq!(moved.transformation, TransformationMatrix::unit());

    let subpixel = SubpixelSprite {
        order: 0,
        pad: 0,
        bounds: sp(1., 2., 3., 4.),
        content_mask: mask.clone(),
        color: Hsla::red(),
        tile: atlas_tile(),
        transformation: rotation,
    };
    let Primitive::SubpixelSprite(moved) = translate(subpixel.into()) else {
        unreachable!()
    };
    assert_eq!(moved.bounds, sp(8., -1., 3., 4.));
    assert_eq!(moved.content_mask.bounds, moved_mask);
    assert_eq!(moved.transformation, expected);

    let poly = PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false.into(),
        opacity: 1.,
        bounds: sp(1., 2., 3., 4.),
        content_mask: mask.clone(),
        corner_radii: Default::default(),
        tile: atlas_tile(),
    };
    let Primitive::PolychromeSprite(moved) = translate(poly.into()) else {
        unreachable!()
    };
    assert_eq!(moved.bounds, sp(8., -1., 3., 4.));
    assert_eq!(moved.content_mask.bounds, moved_mask);
    assert_eq!(moved.tile, atlas_tile());
}

#[test]
fn a_tile_scene_keeps_overlapping_primitives_in_drawing_order() {
    let mut content = Scene::default();
    content.insert_primitive(quad(sp(10., 10., 20., 20.)));
    content.insert_primitive(quad(sp(20., 20., 20., 20.)));
    content.finish();
    let layer = layer(content);
    let t00 = layer.tile_scene(TileCoord { x: 0, y: 0 });
    assert_eq!(t00.quads.len(), 2);
    assert!(t00.quads[0].order < t00.quads[1].order);
    assert_eq!(t00.quads[1].bounds, sp(20., 20., 20., 20.));
}

/// Upstream's glyph quantization, as `Window::paint_glyph` had it.
fn old_quantize(x: f32, y: f32) -> (f32, f32, u8) {
    use crate::{SUBPIXEL_VARIANTS_X as VX, SUBPIXEL_VARIANTS_Y as VY};
    let qx = crate::util::round_half_toward_zero(x * VX as f32) / VX as f32;
    let qy = crate::util::round_half_toward_zero(y * VY as f32) / VY as f32;
    (qx.trunc(), qy.trunc(), (qx.fract() * VX as f32) as u8)
}

#[test]
fn glyph_quantization_is_unchanged_on_screen() {
    for i in 0..20_000 {
        let x = i as f32 * 0.0137;
        let y = i as f32 * 0.0291;
        let (origin, variant) =
            crate::fast::glyphs::quantize_origin(point(ScaledPixels(x), ScaledPixels(y)));
        let (ox, oy, v) = old_quantize(x, y);
        assert_eq!(
            (origin.x.0, origin.y.0, variant.x, variant.y),
            (ox, oy, v, 0),
            "at ({x}, {y})"
        );
        let emoji =
            crate::fast::glyphs::quantize_emoji_origin(point(ScaledPixels(x), ScaledPixels(y)));
        assert_eq!(
            (emoji.x.0, emoji.y.0),
            (
                crate::util::round_half_toward_zero(x),
                crate::util::round_half_toward_zero(y)
            ),
            "emoji at ({x}, {y})"
        );
    }
}

#[test]
fn glyph_quantization_moves_with_whole_pixel_shifts() {
    for i in 0..5_000 {
        let x = -300. + i as f32 * 0.0731;
        let y = -300. + i as f32 * 0.0519;
        let (a, va) = crate::fast::glyphs::quantize_origin(point(ScaledPixels(x), ScaledPixels(y)));
        let (b, vb) = crate::fast::glyphs::quantize_origin(point(
            ScaledPixels(x + 1024.),
            ScaledPixels(y + 1024.),
        ));
        assert_eq!(va, vb, "variant at ({x}, {y})");
        assert_eq!(
            (b.x.0 - a.x.0, b.y.0 - a.y.0),
            (1024., 1024.),
            "origin at ({x}, {y})"
        );
        let ea =
            crate::fast::glyphs::quantize_emoji_origin(point(ScaledPixels(x), ScaledPixels(y)));
        let eb = crate::fast::glyphs::quantize_emoji_origin(point(
            ScaledPixels(x + 1024.),
            ScaledPixels(y + 1024.),
        ));
        assert_eq!(
            (eb.x.0 - ea.x.0, eb.y.0 - ea.y.0),
            (1024., 1024.),
            "emoji at ({x}, {y})"
        );
    }
}
