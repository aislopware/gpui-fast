//! Tests of scroll layers.

use crate::AppContext as _;
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
        content_mask: mask,
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
            content_mask: mask,
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
    path.content_mask = mask;
    for vertex in &mut path.vertices {
        vertex.content_mask = mask;
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
        content_mask: mask,
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
        content_mask: mask,
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
        content_mask: mask,
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
        content_mask: mask,
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

struct EmptyView;

impl crate::Render for EmptyView {
    fn render(
        &mut self,
        _window: &mut crate::Window,
        _cx: &mut crate::Context<Self>,
    ) -> impl crate::IntoElement {
        crate::Empty
    }
}

#[crate::test]
fn scroll_layers_are_on_where_compiled_and_can_be_turned_off(cx: &mut crate::TestAppContext) {
    let window = cx.add_window(|_, _| EmptyView);
    cx.update_window(window.into(), |_, window, cx| {
        assert_eq!(
            window.fast_layers.enabled,
            crate::fast::layers::COMPILED
                && std::env::var("GPUI_SCROLL_LAYERS").map_or(true, |value| value != "0")
        );
        assert_eq!(
            crate::fast::layers::active(window, cx),
            window.fast_layers.enabled
        );
        window.set_scroll_layers(false);
        assert!(!window.fast_layers.enabled);
        assert!(window.fast_layers.layers.is_empty());
        assert!(
            window.refreshing,
            "the switch redraws the window from scratch"
        );
        window.draw(cx).clear(cx);
        assert!(!crate::fast::layers::active(window, cx));

        window.set_scroll_layers(true);
        assert!(
            !crate::fast::layers::active(window, cx),
            "not while the window is refreshing"
        );
        window.draw(cx).clear(cx);
        assert_eq!(
            crate::fast::layers::active(window, cx),
            crate::fast::layers::COMPILED
        );

        window.set_view_retention(false);
        window.draw(cx).clear(cx);
        assert!(!crate::fast::layers::active(window, cx));
    })
    .unwrap();
}

#[test]
fn layout_stats_count_scroll_layer_work() {
    let stats = crate::LayoutStats::default();
    assert_eq!(
        (
            stats.layer_frames_composited,
            stats.layer_frames_repainted,
            stats.tiles_dirtied,
            stats.layer_rebuilds_for_input,
            stats.layers_demoted,
        ),
        (0, 0, 0, 0, 0)
    );
}

/// The paint stream (M3): painting a scroll container's content into a
/// layer, tile diffing, snapping, background baking and compositing.
mod paint {
    use crate::AppContext as _;
    use crate::fast::layers::tiles::{dirty_tiles, tile_hashes};
    use crate::{Bounds, ContentMask, Hsla, Quad, ScaledPixels, Scene, TileCoord, point, size};

    fn sp(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(w), ScaledPixels(h)),
        }
    }

    fn quad(bounds: Bounds<ScaledPixels>, color: Hsla) -> Quad {
        Quad {
            bounds,
            content_mask: ContentMask {
                bounds: sp(-10_000., -10_000., 20_000., 20_000.),
            },
            background: color.into(),
            ..Default::default()
        }
    }

    fn scene(quads: &[Quad]) -> Scene {
        let mut scene = Scene::default();
        for quad in quads {
            scene.insert_primitive(*quad);
        }
        scene.finish();
        scene
    }

    fn region() -> Bounds<ScaledPixels> {
        sp(0., 0., 1024., 1024.)
    }

    fn dirty(old: &Scene, new: &Scene) -> Vec<TileCoord> {
        dirty_tiles(
            &tile_hashes(old, 512, region()),
            &tile_hashes(new, 512, region()),
        )
    }

    fn tiles(coords: &[(i32, i32)]) -> Vec<TileCoord> {
        coords.iter().map(|&(x, y)| TileCoord { x, y }).collect()
    }

    #[test]
    fn every_tile_of_the_region_gets_a_hash_even_an_empty_one() {
        let hashes = tile_hashes(
            &scene(&[quad(sp(10., 10., 5., 5.), Hsla::red())]),
            512,
            region(),
        );
        let mut keys: Vec<_> = hashes.keys().copied().collect();
        keys.sort();
        assert_eq!(keys, tiles(&[(0, 0), (0, 1), (1, 0), (1, 1)]));
        assert_eq!(
            hashes[&TileCoord { x: 1, y: 0 }],
            hashes[&TileCoord { x: 1, y: 1 }]
        );
        assert_ne!(
            hashes[&TileCoord { x: 0, y: 0 }],
            hashes[&TileCoord { x: 1, y: 1 }]
        );
    }

    #[test]
    fn identical_scenes_dirty_no_tile() {
        let quads = [
            quad(sp(10., 10., 20., 20.), Hsla::red()),
            quad(sp(500., 600., 40., 10.), Hsla::blue()),
        ];
        assert_eq!(dirty(&scene(&quads), &scene(&quads)), Vec::new());
    }

    #[test]
    fn a_colour_change_dirties_the_tiles_the_quad_covers() {
        let before = [
            quad(sp(10., 10., 20., 20.), Hsla::red()),
            quad(sp(500., 600., 40., 10.), Hsla::blue()),
        ];
        let mut after = before;
        after[1].background = Hsla::green().into();
        assert_eq!(
            dirty(&scene(&before), &scene(&after)),
            tiles(&[(0, 1), (1, 1)])
        );
    }

    #[test]
    fn a_move_within_one_tile_dirties_that_tile() {
        let before = [
            quad(sp(10., 10., 20., 20.), Hsla::red()),
            quad(sp(600., 600., 10., 10.), Hsla::blue()),
        ];
        let mut after = before;
        after[1].bounds = sp(640., 610., 10., 10.);
        assert_eq!(dirty(&scene(&before), &scene(&after)), tiles(&[(1, 1)]));
    }

    #[test]
    fn an_added_primitive_spanning_two_tiles_dirties_both() {
        let before = [quad(sp(10., 10., 20., 20.), Hsla::red())];
        let after = [
            quad(sp(10., 10., 20., 20.), Hsla::red()),
            quad(sp(100., 500., 10., 40.), Hsla::blue()),
        ];
        assert_eq!(
            dirty(&scene(&before), &scene(&after)),
            tiles(&[(0, 0), (0, 1)])
        );
    }

    #[test]
    fn tiles_new_to_the_region_are_dirty() {
        let quads = [quad(sp(10., 10., 20., 20.), Hsla::red())];
        let old = tile_hashes(&scene(&quads), 512, sp(0., 0., 512., 512.));
        let new = tile_hashes(&scene(&quads), 512, sp(0., 0., 512., 1024.));
        assert_eq!(dirty_tiles(&old, &new), tiles(&[(0, 1)]));
    }

    #[test]
    fn a_primitive_moved_by_whole_tiles_hashes_like_its_old_tile() {
        // Its mask moves with it, so it draws the same pixels in its tile.
        let at = |bounds: Bounds<ScaledPixels>| Quad {
            content_mask: ContentMask { bounds },
            ..quad(bounds, Hsla::red())
        };
        let a = tile_hashes(&scene(&[at(sp(10., 10., 20., 20.))]), 512, region());
        let b = tile_hashes(&scene(&[at(sp(522., 522., 20., 20.))]), 512, region());
        assert_eq!(a[&TileCoord { x: 0, y: 0 }], b[&TileCoord { x: 1, y: 1 }]);
    }

    #[test]
    fn a_mask_change_outside_a_tile_leaves_the_tile_clean() {
        // The painted region, every primitive's mask while a layer paints,
        // moves with the scroll offset: only the tiles it crosses change.
        let masked = |mask: Bounds<ScaledPixels>| {
            [
                Quad {
                    content_mask: ContentMask { bounds: mask },
                    ..quad(sp(10., 10., 20., 900.), Hsla::red())
                },
                Quad {
                    content_mask: ContentMask { bounds: mask },
                    ..quad(sp(600., 10., 20., 900.), Hsla::blue())
                },
            ]
        };
        let before = masked(sp(0., -100., 1024., 1000.));
        let after = masked(sp(0., -90., 1024., 1000.));
        assert_eq!(
            dirty(&scene(&before), &scene(&after)),
            tiles(&[(0, 1), (1, 1)])
        );
    }

    fn row_color(row: usize) -> Hsla {
        crate::hsla(row as f32 / 64., 0.5, 0.5, 1.)
    }

    /// A panel, white at first, holding a 100 px scroll container of `rows` rows of
    /// 20 px, each its own colour.
    struct Rows {
        rows: usize,
        panel: Hsla,
        scroll: crate::ScrollHandle,
        height: crate::Pixels,
    }

    impl crate::Render for Rows {
        fn render(
            &mut self,
            _window: &mut crate::Window,
            _cx: &mut crate::Context<Self>,
        ) -> impl crate::IntoElement {
            use crate::{
                InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _,
                Styled as _,
            };
            crate::div().size_full().bg(self.panel).child(
                crate::div()
                    .id("s")
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .h(self.height)
                    .children(
                        (0..self.rows).map(|i| crate::div().h(crate::px(20.)).bg(row_color(i))),
                    ),
            )
        }
    }

    fn rows_window(cx: &mut crate::TestAppContext, rows: usize) -> crate::WindowHandle<Rows> {
        rows_window_of_height(cx, rows, crate::px(100.))
    }

    fn rows_window_of_height(
        cx: &mut crate::TestAppContext,
        rows: usize,
        height: crate::Pixels,
    ) -> crate::WindowHandle<Rows> {
        let window = cx.add_window(|_, _| Rows {
            rows,
            panel: crate::white(),
            scroll: crate::ScrollHandle::new(),
            height,
        });
        // The first frame redraws everything, which never uses layers.
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        window
    }

    /// Draws `window` with its view rebuilt and every scroll container made
    /// to decide `decision`.
    fn draw_deciding(
        cx: &mut crate::TestAppContext,
        window: crate::WindowHandle<Rows>,
        decision: crate::fast::layers::policy::Decision,
    ) {
        window
            .update(cx, |_, window, cx| {
                window.fast_layers.forced_decision = Some(decision);
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    fn with_record<R>(
        cx: &mut crate::TestAppContext,
        window: crate::WindowHandle<Rows>,
        f: impl FnOnce(&crate::fast::layers::record::LayerRecord, &crate::Window) -> R,
    ) -> R {
        cx.update_window(window.into(), |_, window, _| {
            assert_eq!(window.fast_layers.layers.len(), 1, "one layer");
            let layer = window.fast_layers.layers.values().next().unwrap();
            f(
                layer.record.as_ref().expect("the layer was painted"),
                window,
            )
        })
        .unwrap()
    }

    fn row_quads(scene: &Scene, rows: usize) -> Vec<(usize, Bounds<ScaledPixels>)> {
        scene
            .quads
            .iter()
            .filter_map(|quad| {
                (0..rows)
                    .find(|&row| quad.background == row_color(row).into())
                    .map(|row| (row, quad.bounds))
            })
            .collect()
    }

    #[crate::test]
    fn a_repainted_layer_holds_the_content_in_content_space(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let window = rows_window(cx, 40);
        draw_deciding(cx, window, crate::fast::layers::policy::Decision::Repaint);

        let main_rows = cx
            .update_window(window.into(), |_, window, _| {
                row_quads(&window.rendered_frame.scene, 40)
            })
            .unwrap();
        assert_eq!(main_rows, Vec::new(), "the rows are painted into the layer");

        let scale = cx
            .update_window(window.into(), |_, window, _| window.scale_factor())
            .unwrap();
        with_record(cx, window, |record, _| {
            // The viewport, 100 px, and one viewport of overscan below it;
            // nothing above, at the top.
            assert_eq!(record.viewport.size.height, crate::px(100.));
            assert_eq!(record.painted_region.origin.y, crate::px(0.));
            assert_eq!(record.painted_region.size.height, crate::px(200.));
            let rows = row_quads(&record.content, 40);
            assert_eq!(
                rows.iter().map(|(row, _)| *row).collect::<Vec<_>>(),
                (0..10).collect::<Vec<_>>()
            );
            for (row, bounds) in rows {
                assert_eq!(bounds.origin.y, ScaledPixels(row as f32 * 20. * scale));
                assert_eq!(bounds.size.height, ScaledPixels(20. * scale));
            }
            assert_eq!(record.generation, 1);
            let mut tiles: Vec<_> = record.tile_hashes.keys().copied().collect();
            tiles.sort();
            // 200 px at the test window's scale of 2 is 400 device px: one
            // tile high.
            assert!(tiles.iter().all(|tile| tile.y == 0), "{tiles:?}");
            assert_eq!(
                record.dirty_tiles, tiles,
                "a first paint dirties every tile"
            );
        });
    }

    #[crate::test]
    fn scrolled_content_is_stored_where_it_was_before_the_scroll(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let window = rows_window(cx, 40);
        window
            .update(cx, |view, _, _| {
                view.scroll
                    .set_offset(crate::point(crate::px(0.), crate::px(-300.)))
            })
            .unwrap();
        draw_deciding(cx, window, crate::fast::layers::policy::Decision::Repaint);
        let scale = cx
            .update_window(window.into(), |_, window, _| window.scale_factor())
            .unwrap();
        with_record(cx, window, |record, _| {
            // Overscan of one viewport above and below the one at 300..400.
            assert_eq!(record.painted_region.origin.y, crate::px(-100.));
            assert_eq!(record.painted_region.size.height, crate::px(300.));
            let rows = row_quads(&record.content, 40);
            assert_eq!(
                rows.iter().map(|(row, _)| *row).collect::<Vec<_>>(),
                (10..25).collect::<Vec<_>>()
            );
            for (row, bounds) in rows {
                assert_eq!(bounds.origin.y, ScaledPixels(row as f32 * 20. * scale));
            }
        });
    }

    #[crate::test]
    fn a_repaint_at_a_shifted_offset_dirties_only_the_edge_tiles(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        use crate::fast::layers::policy::Decision;
        // Enough viewport for the painted region to span many tiles.
        let window = rows_window_of_height(cx, 400, crate::px(1000.));
        let scroll_to = |cx: &mut crate::TestAppContext, y: f32| {
            window
                .update(cx, |view, _, _| {
                    view.scroll
                        .set_offset(crate::point(crate::px(0.), crate::px(y)))
                })
                .unwrap();
            draw_deciding(cx, window, Decision::Repaint);
        };
        scroll_to(cx, -3000.);
        scroll_to(cx, -3010.);
        with_record(cx, window, |record, _| {
            let rows: Vec<i32> = record.tile_hashes.keys().map(|tile| tile.y).collect();
            let (top, bottom) = (*rows.iter().min().unwrap(), *rows.iter().max().unwrap());
            assert!(bottom - top >= 4, "the region spans many tile rows");
            assert!(!record.dirty_tiles.is_empty(), "the region's edges moved");
            for tile in &record.dirty_tiles {
                assert!(
                    tile.y == top || tile.y == bottom,
                    "tile {tile:?} inside the region ({top}..={bottom}) is dirty"
                );
            }
        });
        // The same content at the same offset dirties nothing.
        scroll_to(cx, -3010.);
        with_record(cx, window, |record, _| {
            assert_eq!(record.dirty_tiles, Vec::new());
        });
    }

    #[crate::test]
    fn overscan_is_clamped_to_content(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        // 60 px of rows in the 100 px viewport: nothing to scroll to.
        let window = rows_window(cx, 3);
        draw_deciding(cx, window, crate::fast::layers::policy::Decision::Repaint);
        with_record(cx, window, |record, window| {
            assert_eq!(
                record.painted_region, record.viewport,
                "no overscan past the content"
            );
            assert_eq!(row_quads(&record.content, 3).len(), 3);
            let region = record.painted_region.scale(window.scale_factor());
            for tile in record.tile_hashes.keys() {
                let top = tile.y as f32 * 512.;
                assert!(
                    top < region.bottom_right().y.0,
                    "tile {tile:?} below the content"
                );
            }
        });
    }

    /// A scroll container whose content records the element offset it is
    /// prepainted at.
    struct Offsets {
        scroll: crate::ScrollHandle,
        seen: std::rc::Rc<std::cell::Cell<crate::Point<crate::Pixels>>>,
    }

    impl crate::Render for Offsets {
        fn render(
            &mut self,
            _window: &mut crate::Window,
            _cx: &mut crate::Context<Self>,
        ) -> impl crate::IntoElement {
            use crate::{
                InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _,
                Styled as _,
            };
            let seen = self.seen.clone();
            crate::div().size_full().child(
                crate::div()
                    .id("s")
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .h(crate::px(100.))
                    .child(
                        crate::canvas(
                            move |_, window, _| seen.set(window.element_offset()),
                            |_, _, _, _| {},
                        )
                        .h(crate::px(1000.))
                        .w_full(),
                    ),
            )
        }
    }

    #[crate::test]
    fn scroll_offsets_land_on_device_pixels(cx: &mut crate::TestAppContext) {
        let seen = std::rc::Rc::new(std::cell::Cell::new(crate::Point::default()));
        let window = cx.add_window({
            let seen = seen.clone();
            |_, _| Offsets {
                scroll: crate::ScrollHandle::new(),
                seen,
            }
        });
        cx.test_window(window.into())
            .simulate_scale_factor_change(1.25);
        window
            .update(cx, |view, _, cx| {
                view.scroll
                    .set_offset(crate::point(crate::px(0.), crate::px(-10.37)));
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        let offset = seen.get();
        if crate::fast::layers::COMPILED {
            // -10.37 px is -12.9625 device px; the content goes to -13.
            assert_eq!(offset.y * 1.25, crate::px(-13.));
        } else {
            assert_eq!(offset.y, crate::px(-10.37));
        }

        let snap = crate::fast::layers::paint::snap_offset;
        let fractional = crate::point(crate::px(0.37), crate::px(-10.37));
        assert_eq!(snap(fractional, 1.25, false), fractional);
        let snapped = snap(fractional, 1.25, true);
        assert_eq!(
            (snapped.x * 1.25, snapped.y * 1.25),
            (crate::px(0.), crate::px(-13.))
        );
        let whole = crate::point(crate::px(-8.), crate::px(-12.8));
        assert_eq!(snap(whole, 1.25, true), whole, "whole device pixels stay");
    }

    fn bake(quads: &[Quad], window_opaque: bool) -> Option<crate::Rgba> {
        crate::fast::layers::background::bake(
            &scene(quads),
            sp(100., 100., 200., 300.),
            window_opaque,
        )
    }

    #[test]
    fn an_opaque_panel_under_the_viewport_is_baked() {
        let panel = quad(sp(0., 0., 1000., 1000.), Hsla::blue());
        assert_eq!(bake(&[panel], true), Some(Hsla::blue().into()));
        // Something painted later away from the viewport changes nothing.
        let elsewhere = quad(sp(500., 500., 10., 10.), Hsla::red());
        assert_eq!(bake(&[panel, elsewhere], true), Some(Hsla::blue().into()));
        // The topmost panel is the one baked.
        let above = quad(sp(50., 50., 400., 400.), Hsla::green());
        assert_eq!(bake(&[panel, above], true), Some(Hsla::green().into()));
    }

    #[test]
    fn only_a_solid_opaque_panel_covering_the_viewport_is_baked() {
        let panel = quad(sp(0., 0., 1000., 1000.), Hsla::blue());
        assert_eq!(bake(&[panel], false), None, "transparent window");

        let gradient = Quad {
            background: crate::linear_gradient(
                0.,
                crate::linear_color_stop(Hsla::red(), 0.),
                crate::linear_color_stop(Hsla::blue(), 1.),
            ),
            ..panel
        };
        assert_eq!(bake(&[gradient], true), None, "gradient");

        let translucent = quad(sp(0., 0., 1000., 1000.), Hsla::blue().opacity(0.5));
        assert_eq!(bake(&[translucent], true), None, "translucent");

        let partly_inside = quad(sp(250., 250., 100., 100.), Hsla::red());
        assert_eq!(bake(&[panel, partly_inside], true), None, "partly covered");

        let short = quad(sp(0., 0., 1000., 350.), Hsla::blue());
        assert_eq!(bake(&[short], true), None, "does not cover the viewport");

        let clipped = Quad {
            content_mask: ContentMask {
                bounds: sp(0., 0., 1000., 200.),
            },
            ..panel
        };
        assert_eq!(
            bake(&[clipped], true),
            None,
            "clipped short of the viewport"
        );

        let rounded_inside = Quad {
            bounds: sp(90., 90., 500., 500.),
            corner_radii: crate::Corners::all(ScaledPixels(20.)),
            ..panel
        };
        assert_eq!(bake(&[rounded_inside], true), None, "corner inside");
        let rounded_outside = Quad {
            bounds: sp(0., 0., 1000., 1000.),
            corner_radii: crate::Corners::all(ScaledPixels(20.)),
            ..panel
        };
        assert_eq!(
            bake(&[rounded_outside], true),
            Some(Hsla::blue().into()),
            "corners away from the viewport"
        );

        let bordered = Quad {
            bounds: sp(95., 0., 1000., 1000.),
            border_widths: crate::Edges::all(ScaledPixels(10.)),
            border_color: Hsla::red(),
            ..panel
        };
        assert_eq!(bake(&[bordered], true), None, "border inside");
        let transparent_border = Quad {
            border_color: Hsla::transparent_black(),
            ..bordered
        };
        assert_eq!(
            bake(&[transparent_border], true),
            Some(Hsla::blue().into()),
            "an invisible border"
        );
    }

    /// Draws `window` over a panel of colour `panel`, every scroll container
    /// made to decide `decision`.
    fn draw_over(
        cx: &mut crate::TestAppContext,
        window: crate::WindowHandle<Rows>,
        panel: Hsla,
        decision: crate::fast::layers::policy::Decision,
    ) {
        window
            .update(cx, |view, window, cx| {
                view.panel = panel;
                window.fast_layers.forced_decision = Some(decision);
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    #[crate::test]
    fn a_layer_bakes_the_panel_under_it(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let window = rows_window(cx, 40);
        draw_deciding(cx, window, crate::fast::layers::policy::Decision::Repaint);
        with_record(cx, window, |record, _| {
            assert_eq!(record.background, crate::white().into());
        });
    }

    #[crate::test]
    fn background_change_repaints_layer(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        use crate::fast::layers::policy::Decision;
        let window = rows_window(cx, 40);
        draw_deciding(cx, window, Decision::Repaint);
        let (generation, tiles) = with_record(cx, window, |record, _| {
            let mut tiles: Vec<_> = record.tile_hashes.keys().copied().collect();
            tiles.sort();
            (record.generation, tiles)
        });

        // Painted again with nothing changed but the panel: every tile is
        // cleared with another colour, so every tile is dirty.
        draw_over(cx, window, Hsla::blue(), Decision::Repaint);
        let generation = with_record(cx, window, |record, _| {
            assert_eq!(record.background, Hsla::blue().into());
            assert_eq!(record.generation, generation + 1);
            assert_eq!(record.dirty_tiles, tiles);
            record.generation
        });

        // Composited over another panel: the content stands, the tiles are
        // cleared with the new colour.
        draw_over(cx, window, Hsla::green(), Decision::Composite);
        with_record(cx, window, |record, _| {
            assert_eq!(record.background, Hsla::green().into());
            assert_eq!(record.generation, generation + 1);
            assert_eq!(record.dirty_tiles, tiles);
        });

        // Nothing changed: the same tiles, nothing dirty.
        draw_deciding(cx, window, Decision::Repaint);
        with_record(cx, window, |record, _| {
            assert_eq!(record.background, Hsla::green().into());
            assert_eq!(record.dirty_tiles, Vec::new());
        });
    }

    #[crate::test]
    fn no_layer_is_painted_over_a_background_it_cannot_bake(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let window = rows_window(cx, 40);
        draw_over(
            cx,
            window,
            Hsla::blue().opacity(0.5),
            crate::fast::layers::policy::Decision::Repaint,
        );
        cx.update_window(window.into(), |_, window, _| {
            assert_eq!(
                row_quads(&window.rendered_frame.scene, 40).len(),
                5,
                "the rows are painted as without layers"
            );
            assert!(
                window
                    .fast_layers
                    .layers
                    .values()
                    .all(|layer| layer.record.is_none())
            );
        })
        .unwrap();
    }

    /// The layer tiles `scene` composites, with the layer each belongs to.
    fn tile_quads(scene: &Scene) -> Vec<(crate::LayerKey, TileCoord, crate::PolychromeSprite)> {
        scene
            .polychrome_sprites
            .iter()
            .filter_map(|sprite| {
                crate::decode_layer_tile(sprite.tile.texture_id, sprite.tile.tile_id)
                    .map(|(key, coord)| (key, coord, *sprite))
            })
            .collect()
    }

    #[crate::test]
    fn tile_quads_cover_the_viewport_at_the_current_offset(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        use crate::fast::layers::policy::Decision;
        let window = rows_window(cx, 40);
        draw_deciding(cx, window, Decision::Repaint);
        window
            .update(cx, |view, _, _| {
                view.scroll
                    .set_offset(crate::point(crate::px(0.), crate::px(-130.)))
            })
            .unwrap();
        draw_deciding(cx, window, Decision::Composite);

        cx.update_window(window.into(), |_, window, _| {
            let scale = window.scale_factor();
            let layer = window.fast_layers.layers.values().next().expect("a layer");
            let record = layer.record.as_ref().expect("painted");
            let scene = &window.rendered_frame.scene;
            assert_eq!(row_quads(scene, 40), Vec::new(), "the rows are not drawn");

            // The viewport, snapped as every content mask is.
            let viewport = record.viewport;
            let floor = |v: crate::Pixels| crate::util::floor_to_device_pixel(v.0, scale);
            let ceil = |v: crate::Pixels| crate::util::ceil_to_device_pixel(v.0, scale);
            let mask = Bounds::from_corners(
                point(
                    ScaledPixels(floor(viewport.left())),
                    ScaledPixels(floor(viewport.top())),
                ),
                point(
                    ScaledPixels(ceil(viewport.right())),
                    ScaledPixels(ceil(viewport.bottom())),
                ),
            );
            let translation = point(ScaledPixels(0.), ScaledPixels(-130. * scale));

            assert_eq!(scene.layers.frames.len(), 1);
            let frame = &scene.layers.frames[0];
            assert_eq!(frame.key, layer.key);
            assert!(std::rc::Rc::ptr_eq(&frame.content, &record.content));
            assert_eq!(frame.generation, record.generation);
            assert_eq!(frame.background, record.background);
            assert_eq!(frame.tile_size, 512);
            assert_eq!(frame.dirty_tiles, record.dirty_tiles);

            let quads = tile_quads(scene);
            let mut expected = Vec::new();
            let content_viewport = Bounds {
                origin: mask.origin - translation,
                size: mask.size,
            };
            for y in -4..4 {
                for x in -4..8 {
                    let tile = TileCoord { x, y };
                    if frame.tile_bounds(tile).intersects(&content_viewport) {
                        expected.push(tile);
                    }
                }
            }
            let mut coords: Vec<_> = quads.iter().map(|(_, coord, _)| *coord).collect();
            coords.sort();
            assert!(!expected.is_empty());
            assert_eq!(coords, expected);
            let order = quads[0].2.order;
            for (key, coord, sprite) in &quads {
                assert_eq!(*key, layer.key);
                let tile = frame.tile_bounds(*coord);
                assert_eq!(
                    sprite.bounds,
                    Bounds {
                        origin: tile.origin + translation,
                        size: tile.size,
                    }
                );
                assert_eq!(sprite.content_mask.bounds, mask);
                assert_eq!(sprite.order, order, "the tiles share one draw order");
                assert_eq!(sprite.opacity, 1.);
                assert_eq!(sprite.tile.bounds.size.width.0, 512);
                assert_eq!(sprite.tile.bounds.size.height.0, 512);
                assert_eq!(sprite.tile.bounds.origin, crate::Point::default());
            }
        })
        .unwrap();
    }

    #[crate::test]
    fn a_repainted_layer_is_composited_where_it_was_painted(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let window = rows_window(cx, 40);
        cx.update_window(window.into(), |_, window, _| window.reset_layout_stats())
            .unwrap();
        draw_deciding(cx, window, crate::fast::layers::policy::Decision::Repaint);
        cx.update_window(window.into(), |_, window, _| {
            let scene = &window.rendered_frame.scene;
            let quads = tile_quads(scene);
            assert!(!quads.is_empty());
            for (_, coord, sprite) in &quads {
                let tile = scene.layers.frames[0].tile_bounds(*coord);
                assert_eq!(sprite.bounds, tile, "no translation at offset 0");
            }
            let stats = window.layout_stats();
            assert_eq!(stats.layer_frames_repainted, 1);
            assert_eq!(stats.layer_frames_composited, 1);
            assert_eq!(
                stats.tiles_dirtied,
                scene.layers.frames[0].dirty_tiles.len() as u64
            );
        })
        .unwrap();
    }

    #[crate::test]
    fn a_reused_view_keeps_compositing_its_layer(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let window = rows_window(cx, 40);
        draw_deciding(cx, window, crate::fast::layers::policy::Decision::Repaint);
        let before = cx
            .update_window(window.into(), |_, window, _| {
                tile_quads(&window.rendered_frame.scene).len()
            })
            .unwrap();
        // Nothing changed: the view is reused, its scene replayed.
        cx.update_window(window.into(), |_, window, cx| {
            window.reset_layout_stats();
            window.draw(cx).clear(cx)
        })
        .unwrap();
        cx.update_window(window.into(), |_, window, _| {
            let scene = &window.rendered_frame.scene;
            assert_eq!(tile_quads(scene).len(), before);
            assert_eq!(scene.layers.frames.len(), 1, "the layer frame comes along");
            let stats = window.layout_stats();
            assert_eq!(stats.layer_frames_repainted, 0, "the view was reused");
            assert!(stats.views_reused > 0, "the view was reused");
        })
        .unwrap();
    }

    #[crate::test]
    fn a_composited_layer_over_a_background_it_cannot_bake_is_drawn_into_the_frame(
        cx: &mut crate::TestAppContext,
    ) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        use crate::fast::layers::policy::Decision;
        let window = rows_window(cx, 40);
        let direct = cx
            .update_window(window.into(), |_, window, _| {
                row_quads(&window.rendered_frame.scene, 40)
            })
            .unwrap();
        draw_deciding(cx, window, Decision::Repaint);
        draw_over(cx, window, Hsla::blue().opacity(0.5), Decision::Composite);
        cx.update_window(window.into(), |_, window, _| {
            let scene = &window.rendered_frame.scene;
            assert!(tile_quads(scene).is_empty());
            assert!(scene.layers.frames.is_empty());
            assert_eq!(row_quads(scene, 40), direct, "the rows as without a layer");
            let layer = window.fast_layers.layers.values().next().expect("a layer");
            assert!(layer.record.is_none(), "painted afresh next time");
        })
        .unwrap();
    }

    /// A scroll container whose content paints a path under a row.
    struct PathRows;

    impl crate::Render for PathRows {
        fn render(
            &mut self,
            _window: &mut crate::Window,
            _cx: &mut crate::Context<Self>,
        ) -> impl crate::IntoElement {
            use crate::{
                InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _,
                Styled as _,
            };
            crate::div().size_full().bg(crate::white()).child(
                crate::div()
                    .id("s")
                    .overflow_y_scroll()
                    .h(crate::px(100.))
                    .child(
                        crate::canvas(
                            |_, _, _| {},
                            |bounds, _, window, _| {
                                let origin = bounds.origin;
                                let mut path = crate::Path::new(origin);
                                path.line_to(origin + crate::point(crate::px(30.), crate::px(0.)));
                                path.line_to(origin + crate::point(crate::px(0.), crate::px(30.)));
                                window.paint_path(path, crate::black());
                            },
                        )
                        .h(crate::px(40.))
                        .w_full(),
                    )
                    .children((0..20).map(|i| crate::div().h(crate::px(20.)).bg(row_color(i)))),
            )
        }
    }

    #[crate::test]
    fn content_with_paths_is_drawn_into_the_frame_not_composited(cx: &mut crate::TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let window = cx.add_window(|_, _| PathRows);
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        let (direct_paths, direct_rows) = cx
            .update_window(window.into(), |_, window, _| {
                let scene = &window.rendered_frame.scene;
                (scene.paths.len(), row_quads(scene, 20))
            })
            .unwrap();
        assert_eq!(direct_paths, 1);

        window
            .update(cx, |_, window, cx| {
                window.fast_layers.forced_decision =
                    Some(crate::fast::layers::policy::Decision::Repaint);
                window.reset_layout_stats();
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.update_window(window.into(), |_, window, _| {
            let stats = window.layout_stats();
            assert_eq!(stats.layer_frames_repainted, 0, "no layer frame was made");
            assert_eq!(stats.layer_frames_composited, 0);
            let layer = window.fast_layers.layers.values().next().expect("a layer");
            assert!(layer.record.as_ref().expect("painted").has_paths);
            let scene = &window.rendered_frame.scene;
            assert!(
                scene.polychrome_sprites.iter().all(|sprite| {
                    crate::decode_layer_tile(sprite.tile.texture_id, sprite.tile.tile_id).is_none()
                }),
                "no tile quads"
            );
            assert!(scene.layers.frames.is_empty(), "no layer frames");
            assert_eq!(scene.paths.len(), 1, "the path is in the frame");
            assert_eq!(
                row_quads(scene, 20),
                direct_rows,
                "the rows are drawn as without a layer"
            );
        })
        .unwrap();
    }
}

/// Primitives that draw outside their bounds: a shadow's blur and a
/// transformed sprite reach the tiles they draw over, not just the tiles
/// their bounds cover.
mod footprints {
    use super::{atlas_tile, layer, sp, wide_mask};
    use crate::fast::layers::tiles::{dirty_tiles, tile_hashes};
    use crate::{
        Hsla, MonochromeSprite, Radians, Scene, Shadow, TileCoord, TransformationMatrix, point,
    };

    fn shadow(color: Hsla) -> Shadow {
        // Its blur reaches x = 500 + 3 * 10 = 530, inside tile (1, 0).
        Shadow {
            order: 0,
            blur_radius: crate::ScaledPixels(10.),
            bounds: sp(400., 100., 100., 100.),
            corner_radii: Default::default(),
            content_mask: wide_mask(),
            color,
            element_bounds: sp(400., 100., 100., 100.),
            element_corner_radii: Default::default(),
            inset: 0,
            pad: 0,
        }
    }

    fn rotated_sprite(color: Hsla) -> MonochromeSprite {
        // A 70 px square ending 2 px short of tile (1, 0), turned an eighth
        // around its centre (475, 135): its corners reach x = 524.5.
        MonochromeSprite {
            order: 0,
            pad: 0,
            bounds: sp(440., 100., 70., 70.),
            content_mask: wide_mask(),
            color,
            tile: atlas_tile(),
            transformation: TransformationMatrix::unit()
                .translate(point(crate::ScaledPixels(475.), crate::ScaledPixels(135.)))
                .rotate(Radians(std::f32::consts::FRAC_PI_4))
                .translate(point(
                    crate::ScaledPixels(-475.),
                    crate::ScaledPixels(-135.),
                )),
        }
    }

    fn scene_of(primitive: impl Into<crate::scene::Primitive>) -> Scene {
        let mut scene = Scene::default();
        scene.insert_primitive(primitive);
        scene.finish();
        scene
    }

    fn dirty(old: &Scene, new: &Scene) -> Vec<TileCoord> {
        let region = sp(0., 0., 1024., 512.);
        dirty_tiles(
            &tile_hashes(old, 512, region),
            &tile_hashes(new, 512, region),
        )
    }

    const BOTH: [TileCoord; 2] = [TileCoord { x: 0, y: 0 }, TileCoord { x: 1, y: 0 }];

    #[test]
    fn a_shadow_change_dirties_every_tile_its_blur_reaches() {
        assert_eq!(
            dirty(
                &scene_of(shadow(Hsla::red())),
                &scene_of(shadow(Hsla::blue()))
            ),
            BOTH
        );
    }

    #[test]
    fn a_tile_scene_holds_a_shadow_whose_blur_reaches_the_tile() {
        let frame = layer(scene_of(shadow(Hsla::red())));
        assert_eq!(frame.tile_scene(TileCoord { x: 1, y: 0 }).shadows.len(), 1);
        assert!(
            frame
                .tile_scene(TileCoord { x: 1, y: 1 })
                .shadows
                .is_empty()
        );
    }

    #[test]
    fn a_shadow_masked_off_a_tile_leaves_it_alone() {
        let masked = |color| Shadow {
            content_mask: crate::ContentMask {
                bounds: sp(0., 0., 512., 512.),
            },
            ..shadow(color)
        };
        assert_eq!(
            dirty(
                &scene_of(masked(Hsla::red())),
                &scene_of(masked(Hsla::blue()))
            ),
            [TileCoord { x: 0, y: 0 }]
        );
    }

    #[test]
    fn a_rotated_sprite_change_dirties_every_tile_it_turns_into() {
        assert_eq!(
            dirty(
                &scene_of(rotated_sprite(Hsla::red())),
                &scene_of(rotated_sprite(Hsla::blue()))
            ),
            BOTH
        );
        let frame = layer(scene_of(rotated_sprite(Hsla::red())));
        assert_eq!(
            frame
                .tile_scene(TileCoord { x: 1, y: 0 })
                .monochrome_sprites
                .len(),
            1
        );
    }
}

/// Tests of telling scrolls apart from other changes and of deciding what a
/// scroll container's layer does each frame (M4).
mod invalidation {
    use crate::fast::layers::invalidate::{ScrollSource, render_read_offset, scrolled};
    use crate::{
        AnyWindowHandle, App, AppContext as _, Context, Entity, GlobalElementId,
        InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollDelta,
        ScrollHandle, ScrollWheelEvent, StatefulInteractiveElement as _, Styled as _,
        TestAppContext, TouchPhase, Window, div, point, px, rgb,
    };
    use std::{cell::Cell, rc::Rc};

    /// A page scrolled by a wheel: a 100 px tall scroll container of forty
    /// 20 px rows at the top left of the window.
    struct Page {
        handle: ScrollHandle,
        read_offset_in_render: bool,
        reader: Option<Entity<Reader>>,
    }

    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            if self.read_offset_in_render {
                let _ = self.handle.offset();
            }
            div()
                .size_full()
                .child(
                    div()
                        .id("scroller")
                        .overflow_y_scroll()
                        .track_scroll(&self.handle)
                        .w(px(200.))
                        .h(px(100.))
                        .children(
                            (0..40).map(|row| div().h(px(20.)).bg(rgb(0x100000 + row * 0x10))),
                        ),
                )
                .children(self.reader.clone())
        }
    }

    /// A view outside the scroll container that shows where it is scrolled.
    struct Reader {
        handle: ScrollHandle,
        renders: Rc<Cell<usize>>,
    }

    impl Render for Reader {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let offset = self.handle.offset();
            div().w(px(10.)).h(px(10.) - offset.y / 100.)
        }
    }

    fn page(handle: ScrollHandle, read_offset_in_render: bool) -> Page {
        Page {
            handle,
            read_offset_in_render,
            reader: None,
        }
    }

    pub(super) fn with_window<R>(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> R {
        cx.update_window(window, |_, window, cx| f(window, cx))
            .unwrap()
    }

    pub(super) fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
        with_window(cx, window, |window, cx| window.draw(cx).clear(cx));
    }

    /// Scrolls the page by `dy` with the wheel, returning the scroll
    /// containers noted as scrolled before the frame that follows is drawn.
    pub(super) fn wheel(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        dy: f32,
    ) -> Vec<GlobalElementId> {
        with_window(cx, window, |window, cx| {
            window.dispatch_event(
                crate::PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: point(px(20.), px(20.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                    modifiers: Default::default(),
                    touch_phase: TouchPhase::Moved,
                }),
                cx,
            );
            window
                .fast_layers
                .scrolls
                .scrolled
                .iter()
                .cloned()
                .collect()
        })
    }

    /// The page's scroll container, as last painted.
    fn scroller(cx: &mut TestAppContext, window: AnyWindowHandle) -> GlobalElementId {
        with_window(cx, window, |window, _| {
            window
                .fast_layers
                .scrolls
                .containers()
                .find(|id| id.last() == Some(&"scroller".into()))
                .cloned()
                .expect("the scroll container was painted")
        })
    }

    fn is_scrolled(cx: &mut TestAppContext, window: AnyWindowHandle, id: &GlobalElementId) -> bool {
        with_window(cx, window, |window, _| scrolled(window, id))
    }

    #[crate::test]
    fn a_wheel_scroll_is_noted_for_its_container(cx: &mut TestAppContext) {
        let window: AnyWindowHandle = cx
            .add_window(|_, _| page(ScrollHandle::new(), false))
            .into();
        draw(cx, window);
        let scroller = scroller(cx, window);
        assert!(!is_scrolled(cx, window, &scroller));

        assert_eq!(wheel(cx, window, -30.), vec![scroller.clone()]);
        draw(cx, window);
        assert!(
            !is_scrolled(cx, window, &scroller),
            "a frame takes in the scrolls before it"
        );
    }

    #[crate::test]
    fn programmatic_scrolls_are_noted(cx: &mut TestAppContext) {
        let handle = ScrollHandle::new();
        let window: AnyWindowHandle = cx
            .add_window({
                let handle = handle.clone();
                move |_, _| page(handle, false)
            })
            .into();
        draw(cx, window);
        let scroller = scroller(cx, window);
        assert!(!is_scrolled(cx, window, &scroller));
        handle.set_offset(point(px(0.), px(-40.)));
        assert!(is_scrolled(cx, window, &scroller));
        draw(cx, window);
        assert!(!is_scrolled(cx, window, &scroller));
        handle.scroll_to_item(30);
        assert!(is_scrolled(cx, window, &scroller));
        draw(cx, window);
        assert!(!is_scrolled(cx, window, &scroller));
        handle.scroll_to_bottom();
        assert!(is_scrolled(cx, window, &scroller));
    }

    /// Whether the root view's own reading, last frame, included the offset
    /// of its scroll container.
    fn root_view_read_offset(cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
        let scroller = scroller(cx, window);
        with_window(cx, window, |window, _| {
            let source = window
                .fast_layers
                .scrolls
                .source(&scroller)
                .expect("the scroll container was painted");
            assert!(matches!(source, ScrollSource::Handle(_)));
            let record = window
                .rendered_frame
                .retained
                .records
                .first()
                .expect("the root view is retained");
            render_read_offset(&record.own_dependencies, &source)
        })
    }

    #[crate::test]
    fn a_render_that_reads_the_offset_depends_on_it(cx: &mut TestAppContext) {
        let window: AnyWindowHandle = cx.add_window(|_, _| page(ScrollHandle::new(), true)).into();
        draw(cx, window);
        assert!(root_view_read_offset(cx, window));
    }

    #[crate::test]
    fn one_that_does_not_does_not(cx: &mut TestAppContext) {
        let window: AnyWindowHandle = cx
            .add_window(|_, _| page(ScrollHandle::new(), false))
            .into();
        draw(cx, window);
        assert!(!root_view_read_offset(cx, window));
    }

    #[crate::test]
    fn a_view_that_read_the_offset_is_built_again_when_it_scrolls(cx: &mut TestAppContext) {
        let renders = Rc::new(Cell::new(0));
        let window: AnyWindowHandle = cx
            .add_window({
                let renders = renders.clone();
                move |_, cx| {
                    let handle = ScrollHandle::new();
                    let reader = cx.new(|_| Reader {
                        handle: handle.clone(),
                        renders,
                    });
                    Page {
                        handle,
                        read_offset_in_render: false,
                        reader: Some(reader),
                    }
                }
            })
            .into();
        draw(cx, window);
        draw(cx, window);
        let before = renders.get();
        wheel(cx, window, -30.);
        draw(cx, window);
        assert_eq!(renders.get(), before + 1, "the reader shows the new offset");
        draw(cx, window);
        assert_eq!(renders.get(), before + 1, "and is reused once it has");
    }

    /// A list whose view shows whether it is scrolled to its end.
    struct EndIndicator {
        state: crate::ListState,
    }

    impl Render for EndIndicator {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let at_end = self.state.is_scrolled_to_end() == Some(true);
            div()
                .size_full()
                .child(
                    crate::list(self.state.clone(), |index, _, _| {
                        div()
                            .h(px(20.))
                            .bg(rgb(0x100000 + index as u32))
                            .into_any_element()
                    })
                    .w(px(200.))
                    .h(px(100.)),
                )
                .child(div().w(px(10.)).h(px(if at_end { 20. } else { 10. })))
        }
    }

    #[crate::test]
    fn a_render_that_asks_whether_a_list_is_scrolled_to_its_end_depends_on_its_offset(
        cx: &mut TestAppContext,
    ) {
        let state = crate::ListState::new(40, crate::ListAlignment::Top, px(100.));
        let window: AnyWindowHandle = cx
            .add_window({
                let state = state.clone();
                move |_, _| EndIndicator { state }
            })
            .into();
        draw(cx, window);
        draw(cx, window);
        let read = with_window(cx, window, |window, _| {
            let record = window
                .rendered_frame
                .retained
                .records
                .first()
                .expect("the root view is retained");
            let source = ScrollSource::of_state(&state.0.borrow().version);
            render_read_offset(&record.own_dependencies, &source)
        });
        assert!(read);
    }
}

/// Tests of what a scroll container decides to do with its layer each frame
/// (M4), through the paint stream's hook around a container's children,
/// which asks [`decide`] and paints the rows into the layer when told to.
mod decisions {
    use super::invalidation::{draw, with_window};
    use crate::fast::layers::policy::{Decision, last_decision};
    use crate::{
        AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity,
        GlobalElementId, Hsla, InteractiveElement as _, IntoElement, MouseMoveEvent,
        ParentElement as _, Pixels, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
        StatefulInteractiveElement as _, Styled as _, TestAppContext, TouchPhase, Window,
        WindowHandle, div, point, px, rgb, size,
    };
    use std::{cell::Cell, rc::Rc};

    pub(super) const ROWS: usize = 40;
    pub(super) const ROW_HEIGHT: f32 = 20.;

    pub(super) fn viewport() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(200.), px(100.)),
        }
    }

    /// A row, with a hover style when `hover` is set, `width` wide.
    fn row(index: usize, hover: bool, width: f32) -> AnyElement {
        let row = div()
            .w(px(width))
            .h(px(ROW_HEIGHT))
            .bg(rgb(0x100000 + index as u32 * 0x10));
        if hover {
            row.id(("row", index))
                .hover(|style| style.bg(rgb(0x00ff00)))
                .into_any_element()
        } else {
            row.into_any_element()
        }
    }

    /// The rows in a view of their own, for a page whose scroll container
    /// holds a child view (pattern A).
    pub(super) struct Rows {
        pub(super) tint: u32,
    }

    impl Render for Rows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().children((0..ROWS).map(|index| {
                div()
                    .h(px(ROW_HEIGHT))
                    .bg(rgb(self.tint + index as u32 * 0x10))
            }))
        }
    }

    /// A page with a 100 px tall scroll container of forty 20 px rows at the
    /// top left of the window: in the page's own view (pattern B), or in a
    /// child view (pattern A), after `extra`, if any. The rows are as wide
    /// as the container, or twice as wide when `wide` is set, which the
    /// container, scrolling on y only, clips.
    pub(super) struct LayerPage {
        pub(super) handle: ScrollHandle,
        pub(super) rows: Option<Entity<Rows>>,
        pub(super) hover: bool,
        pub(super) read_offset_in_render: bool,
        pub(super) extra: Option<Rc<dyn Fn() -> AnyElement>>,
        pub(super) wide: bool,
        /// Whether its render asks for an animation frame.
        pub(super) animate: bool,
    }

    impl Render for LayerPage {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            if self.read_offset_in_render {
                let _ = self.handle.offset();
            }
            if self.animate {
                window.request_animation_frame();
            }
            let content_width = viewport().size.width.0 * if self.wide { 2. } else { 1. };
            let mut scroller = div()
                .id("scroller")
                .overflow_y_scroll()
                .track_scroll(&self.handle)
                .w(viewport().size.width)
                .h(viewport().size.height);
            if let Some(extra) = &self.extra {
                scroller = scroller.child(extra());
            }
            scroller = match &self.rows {
                Some(rows) => scroller.child(rows.clone()),
                None => {
                    scroller.children((0..ROWS).map(|index| row(index, self.hover, content_width)))
                }
            };
            div().size_full().bg(rgb(0xffffff)).child(scroller)
        }
    }

    pub(super) fn new_page(child_view: bool, cx: &mut App) -> LayerPage {
        LayerPage {
            handle: ScrollHandle::new(),
            rows: child_view.then(|| cx.new(|_| Rows { tint: 0x100000 })),
            hover: false,
            read_offset_in_render: false,
            extra: None,
            wide: false,
            animate: false,
        }
    }

    pub(super) fn page(cx: &mut TestAppContext, child_view: bool) -> WindowHandle<LayerPage> {
        let window = cx.add_window(move |_, cx| new_page(child_view, cx));
        draw(cx, window.into());
        draw(cx, window.into());
        window
    }

    pub(super) fn scroller_id(cx: &mut TestAppContext, window: AnyWindowHandle) -> GlobalElementId {
        with_window(cx, window, |window, _| {
            window
                .fast_layers
                .scrolls
                .containers()
                .find(|id| id.last() == Some(&"scroller".into()))
                .cloned()
                .expect("the scroll container was painted")
        })
    }

    /// Draws the frame that follows `change`, unless the change drew one.
    pub(super) fn frame_after(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        change: impl FnOnce(&mut TestAppContext),
    ) {
        let frame = with_window(cx, window, |window, _| window.fast_layers.frame);
        change(cx);
        if with_window(cx, window, |window, _| window.fast_layers.frame) == frame {
            draw(cx, window);
        }
    }

    /// Scrolls by `dy` with the wheel, returning what the scroll container
    /// decided in the frame that followed.
    pub(super) fn scroll(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        dy: f32,
    ) -> Option<Decision> {
        scroll_and(cx, window, dy, |_| {})
    }

    /// Scrolls by `dy` with the wheel after `change`, in one frame,
    /// returning what the scroll container decided in it.
    pub(super) fn scroll_and(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        dy: f32,
        change: impl FnOnce(&mut App),
    ) -> Option<Decision> {
        // In one update, for the change and the scroll to be taken in by the
        // same frame.
        frame_after(cx, window, |cx| {
            with_window(cx, window, |window, cx| {
                change(cx);
                window.dispatch_event(
                    crate::PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position: point(px(20.), px(20.)),
                        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                        modifiers: Default::default(),
                        touch_phase: TouchPhase::Moved,
                    }),
                    cx,
                );
            })
        });
        decision(cx, window)
    }

    /// What the scroll container decided in the last frame drawn, if it
    /// decided anything in it.
    pub(super) fn decision(cx: &mut TestAppContext, window: AnyWindowHandle) -> Option<Decision> {
        let id = scroller_id(cx, window);
        with_window(cx, window, |window, _| last_decision(window, &id))
    }

    /// Scrolls until the container has a layer, checking it is promoted on
    /// the second scrolled frame.
    pub(super) fn promote(cx: &mut TestAppContext, window: AnyWindowHandle) {
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Repaint));
    }

    #[crate::test]
    fn a_wheel_scroll_composites(cx: &mut TestAppContext) {
        let window = page(cx, false).into();
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        assert_eq!(scroll(cx, window, 20.), Some(Decision::Composite));
    }

    #[crate::test]
    fn a_child_view_page_composites(cx: &mut TestAppContext) {
        let window = page(cx, true).into();
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
    }

    #[crate::test]
    fn a_notified_content_view_repaints(cx: &mut TestAppContext) {
        let handle = page(cx, true);
        let window = handle.into();
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        let rows = handle
            .read_with(cx, |page, _| page.rows.clone().unwrap())
            .unwrap();
        // Notified in the frame a scroll draws, so that the page is built
        // again for the scroll and the rows' view is found changed.
        // Notified without being updated first: the view may have changed
        // what it renders in a way nothing it read shows.
        assert_eq!(
            scroll_and(cx, window, -20., |cx| cx.notify(rows.entity_id())),
            Some(Decision::Repaint)
        );
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        assert_eq!(
            scroll_and(cx, window, -20., |cx| rows.update(cx, |rows, cx| {
                rows.tint = 0x200000;
                cx.notify();
            })),
            Some(Decision::Repaint)
        );
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
    }

    #[crate::test]
    fn an_owner_notified_for_another_reason_while_scrolling_repaints(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        // What the owner renders changes through state nothing records a
        // read of, which only its notification tells.
        let height = Rc::new(Cell::new(10.));
        handle
            .update(cx, |page, _, cx| {
                let height = height.clone();
                page.extra = Some(Rc::new(move || {
                    div().h(px(height.get())).into_any_element()
                }));
                cx.notify();
            })
            .unwrap();
        draw(cx, window);
        let owner = handle.update(cx, |_, _, cx| cx.entity_id()).unwrap();
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        // Notified before the scroll.
        assert_eq!(
            scroll_and(cx, window, -20., |cx| {
                height.set(30.);
                cx.notify(owner);
            }),
            Some(Decision::Repaint)
        );
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        // Notified after it.
        frame_after(cx, window, |cx| {
            with_window(cx, window, |window, cx| {
                window.dispatch_event(
                    crate::PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position: point(px(20.), px(20.)),
                        delta: ScrollDelta::Pixels(point(px(0.), px(-20.))),
                        modifiers: Default::default(),
                        touch_phase: TouchPhase::Moved,
                    }),
                    cx,
                );
                height.set(10.);
                cx.notify(owner);
            })
        });
        assert_eq!(decision(cx, window), Some(Decision::Repaint));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
    }

    #[crate::test]
    fn content_wider_than_a_container_scrolling_on_y_composites(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| LayerPage {
            wide: true,
            ..new_page(false, cx)
        });
        let window = handle.into();
        draw(cx, window);
        draw(cx, window);
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
    }

    #[crate::test]
    fn an_owner_that_reads_the_offset_in_render_repaints(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        handle
            .update(cx, |page, _, cx| {
                page.read_offset_in_render = true;
                cx.notify();
            })
            .unwrap();
        draw(cx, window);
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Repaint));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Repaint));
    }

    #[crate::test]
    #[ignore = "a composited frame paints no hitboxes for the rows until the input stream (M5) carries them"]
    fn a_hover_change_in_the_content_repaints(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        handle
            .update(cx, |page, _, cx| {
                page.hover = true;
                cx.notify();
            })
            .unwrap();
        draw(cx, window);
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        frame_after(cx, window, |cx| {
            with_window(cx, window, |window, cx| {
                window.dispatch_event(
                    crate::PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(20.), px(70.)),
                        pressed_button: None,
                        modifiers: Default::default(),
                    }),
                    cx,
                );
            })
        });
        assert_eq!(decision(cx, window), Some(Decision::Repaint));
    }

    /// A view around the page that sets the text colour the page inherits.
    struct Themed {
        color: Hsla,
        page: Entity<LayerPage>,
    }

    impl Render for Themed {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .text_color(self.color)
                .child(self.page.clone())
        }
    }

    #[crate::test]
    fn a_style_change_of_the_scroll_div_repaints(cx: &mut TestAppContext) {
        // The scroll div's own style is set by the view that holds it, whose
        // notification repaints the layer anyway; what it inherits changes
        // without it, as the text colour of a view around it does.
        let handle = cx.add_window(|_, cx| Themed {
            color: crate::black(),
            page: cx.new(|cx| new_page(false, cx)),
        });
        let window: AnyWindowHandle = handle.into();
        draw(cx, window);
        draw(cx, window);
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        frame_after(cx, window, |cx| {
            handle
                .update(cx, |themed, _, cx| {
                    themed.color = crate::white();
                    cx.notify();
                })
                .unwrap();
        });
        assert_eq!(decision(cx, window), Some(Decision::Repaint));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
    }

    #[crate::test]
    fn exposing_past_the_margin_repaints(cx: &mut TestAppContext) {
        let window = page(cx, false).into();
        promote(cx, window);
        // Painted at the offset it was promoted at, -40 px, one viewport
        // (100 px) beyond each edge as far as the content goes. The margin is
        // a quarter of that: the fourth 20 px scroll leaves less than 25 px
        // painted below the viewport.
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Repaint));
        let id = scroller_id(cx, window);
        let painted_at = with_window(cx, window, |window, _| {
            let record = window.fast_layers.layers[&id].record.as_ref().unwrap();
            (record.scroll_offset, record.painted_region)
        });
        assert_eq!(
            painted_at,
            (
                point(px(0.), px(-120.)),
                Bounds::from_corners(point(px(0.), px(-100.)), point(px(200.), px(200.)))
            ),
            "re-centred on the viewport"
        );
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
    }

    /// Puts a 200 px square container scrolling on x, holding an 800 px wide
    /// child, at the top of the page's scroll container, painted by the
    /// page's own view.
    fn with_inner_scroller(
        cx: &mut TestAppContext,
        handle: WindowHandle<LayerPage>,
    ) -> ScrollHandle {
        let inner = ScrollHandle::new();
        let tracked = inner.clone();
        handle
            .update(cx, |page, _, cx| {
                page.extra = Some(Rc::new(move || {
                    div()
                        .id("inner")
                        .overflow_x_scroll()
                        .track_scroll(&tracked)
                        .w(px(200.))
                        .h(px(200.))
                        .child(div().w(px(800.)).h(px(200.)).bg(rgb(0x123456)))
                        .into_any_element()
                }));
                cx.notify();
            })
            .unwrap();
        draw(cx, handle.into());
        inner
    }

    #[crate::test]
    fn a_wheel_scroll_of_a_container_inside_the_content_repaints(cx: &mut TestAppContext) {
        // The inner container is painted by the view holding the outer one,
        // which the inner one's wheel listener notifies: a change of the
        // outer layer's content, not a scroll of it (spec §6.6).
        let handle = page(cx, false);
        let window = handle.into();
        let inner = with_inner_scroller(cx, handle);
        promote(cx, window);
        let before = inner.offset();
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Repaint));
        assert_ne!(
            inner.offset(),
            before,
            "the wheel scrolled the inner container"
        );
    }

    #[crate::test]
    fn a_programmatic_scroll_of_a_container_inside_the_content_repaints(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        let inner = with_inner_scroller(cx, handle);
        promote(cx, window);
        assert_eq!(
            scroll_and(cx, window, -20., |_| inner
                .set_offset(point(px(-100.), px(0.)))),
            Some(Decision::Repaint)
        );
    }
}

/// Tests of which scroll containers get a layer and for how long (M4).
mod policies {
    use super::decisions::{
        LayerPage, decision, frame_after, page, promote, scroll, scroll_and, scroller_id,
    };
    use super::invalidation::{draw, with_window};
    use crate::fast::layers::policy::Decision;
    use crate::{
        AnyElement, AnyWindowHandle, GlobalElementId, InteractiveElement as _, IntoElement,
        ParentElement as _, Path, Styled as _, TestAppContext, WindowHandle, anchored, canvas,
        deferred, div, point, px, size,
    };
    use std::rc::Rc;

    /// Puts `extra` at the top of the page's scroll container.
    fn with_extra(
        cx: &mut TestAppContext,
        handle: WindowHandle<LayerPage>,
        extra: impl Fn() -> AnyElement + 'static,
    ) {
        handle
            .update(cx, |page, _, cx| {
                page.extra = Some(Rc::new(extra));
                cx.notify();
            })
            .unwrap();
        draw(cx, handle.into());
    }

    fn has_layer(cx: &mut TestAppContext, window: AnyWindowHandle, id: &GlobalElementId) -> bool {
        with_window(cx, window, |window, _| {
            window.fast_layers.layers.contains_key(id)
        })
    }

    fn has_record(cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
        let id = scroller_id(cx, window);
        with_window(cx, window, |window, _| {
            window
                .fast_layers
                .layers
                .get(&id)
                .is_some_and(|layer| layer.record.is_some())
        })
    }

    fn layers_demoted(cx: &mut TestAppContext, window: AnyWindowHandle) -> u64 {
        with_window(cx, window, |window, _| window.layout_stats().layers_demoted)
    }

    #[crate::test]
    fn a_container_is_promoted_after_two_scrolled_frames(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        assert!(!has_record(cx, window));
        // A frame drawn in between for something else breaks the streak.
        frame_after(cx, window, |cx| {
            handle.update(cx, |_, _, cx| cx.notify()).unwrap();
        });
        assert_eq!(decision(cx, window), Some(Decision::Bypass));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        assert!(!has_record(cx, window));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Repaint));
        assert!(has_record(cx, window));
    }

    #[crate::test]
    fn deferred_draws_inside_make_it_ineligible(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        with_extra(cx, handle, || {
            div()
                .h(px(10.))
                .child(deferred(
                    anchored().child(div().w(px(50.)).h(px(50.)).bg(crate::red())),
                ))
                .into_any_element()
        });
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        assert!(!has_record(cx, window));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
    }

    #[crate::test]
    fn anchored_elements_inside_make_it_ineligible(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        // Positioned against the window's edges when prepainted, which a
        // composited layer would move with the content.
        with_extra(cx, handle, || {
            div()
                .h(px(10.))
                .child(anchored().child(div().w(px(50.)).h(px(50.)).bg(crate::red())))
                .into_any_element()
        });
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        // On today's path while it holds one, but not demoted (spec §6.5).
        assert_eq!(layers_demoted(cx, window), 0);
        assert!(!has_record(cx, window));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
    }

    #[crate::test]
    fn an_animation_frame_requested_by_the_owner_makes_it_ineligible(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        frame_after(cx, window, |cx| {
            handle
                .update(cx, |page, _, cx| {
                    page.animate = true;
                    cx.notify();
                })
                .unwrap();
        });
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        assert!(!has_record(cx, window));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
    }

    #[crate::test]
    fn a_focused_input_inside_makes_it_ineligible(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        let focus = with_window(cx, window, |_, cx| cx.focus_handle());
        let input_focus = focus.clone();
        with_extra(cx, handle, move || {
            div()
                .h(px(10.))
                .track_focus(&input_focus)
                .child(super::super::retained::text_input(
                    input_focus.clone(),
                    "inside",
                ))
                .into_any_element()
        });
        with_window(cx, window, |window, cx| window.focus(&focus, cx));
        draw(cx, window);
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        assert!(!has_record(cx, window));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
    }

    #[crate::test]
    fn a_layer_whose_input_loses_focus_is_composited_again_soon(cx: &mut TestAppContext) {
        // A focused input keeps the container on today's path only while
        // it is focused: it is not demoted, which would keep the container
        // waiting for 60 stable frames (spec §6.5).
        let handle = page(cx, false);
        let window = handle.into();
        let focus = with_window(cx, window, |_, cx| cx.focus_handle());
        let input_focus = focus.clone();
        with_extra(cx, handle, move || {
            div()
                .h(px(10.))
                .track_focus(&input_focus)
                .child(super::super::retained::text_input(
                    input_focus.clone(),
                    "inside",
                ))
                .into_any_element()
        });
        with_window(cx, window, |window, cx| window.focus(&focus, cx));
        draw(cx, window);
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        frame_after(cx, window, |cx| {
            with_window(cx, window, |window, cx| window.blur(cx));
        });
        let mut decisions = Vec::new();
        for _ in 0..12 {
            decisions.push(scroll(cx, window, 10.).unwrap());
        }
        assert!(
            decisions.contains(&Decision::Composite),
            "composited again within 12 frames: {decisions:?}"
        );
        assert_eq!(layers_demoted(cx, window), 0);
    }

    #[crate::test]
    fn content_with_paths_is_demoted(cx: &mut TestAppContext) {
        let handle = page(cx, false);
        let window = handle.into();
        with_extra(cx, handle, || {
            canvas(
                |_, _, _| {},
                |bounds, _, window, _| {
                    // Where it shows once scrolled by the 40 px promotion
                    // takes; out of view it is culled.
                    let origin = bounds.origin + point(px(0.), px(60.));
                    let mut path = Path::new(origin);
                    path.line_to(origin + point(px(10.), px(0.)));
                    path.line_to(origin + point(px(10.), px(10.)));
                    path.line_to(origin);
                    window.paint_path(path, crate::red());
                },
            )
            .w(px(10.))
            .h(px(100.))
            .into_any_element()
        });
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        assert_eq!(layers_demoted(cx, window), 1);
        assert!(!has_record(cx, window));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Bypass));
    }

    #[crate::test]
    fn churning_content_is_demoted_and_repromoted_after_60_stable_frames(cx: &mut TestAppContext) {
        let handle = page(cx, true);
        let window = handle.into();
        let rows = handle
            .read_with(cx, |page, _| page.rows.clone().unwrap())
            .unwrap();
        let change = |tint: u32| {
            let rows = rows.clone();
            move |cx: &mut crate::App| {
                rows.update(cx, |rows, cx| {
                    rows.tint = tint;
                    cx.notify();
                })
            }
        };
        promote(cx, window);
        // Changed on eight frames of the last sixteen, the layer is kept;
        // on the ninth, it is dropped.
        for tint in 1..=8 {
            assert_eq!(
                scroll_and(cx, window, -10., change(0x100000 + tint)),
                Some(Decision::Repaint)
            );
        }
        assert_eq!(layers_demoted(cx, window), 0);
        assert_eq!(
            scroll_and(cx, window, -10., change(0x200000)),
            Some(Decision::Bypass)
        );
        assert_eq!(layers_demoted(cx, window), 1);
        assert!(!has_record(cx, window));
        assert_eq!(scroll(cx, window, -10.), Some(Decision::Bypass));
        assert_eq!(scroll(cx, window, -10.), Some(Decision::Bypass));
        // A change while it is demoted starts the wait again.
        assert_eq!(
            scroll_and(cx, window, -10., change(0x300000)),
            Some(Decision::Bypass)
        );
        for _ in 0..59 {
            draw(cx, window);
        }
        assert_eq!(scroll(cx, window, -10.), Some(Decision::Bypass));
        assert_eq!(scroll(cx, window, -10.), Some(Decision::Repaint));
        assert_eq!(scroll(cx, window, -10.), Some(Decision::Composite));
    }

    #[crate::test]
    fn a_layer_not_composited_for_120_frames_is_dropped(cx: &mut TestAppContext) {
        let window = page(cx, false).into();
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        let id = scroller_id(cx, window);
        for _ in 0..119 {
            draw(cx, window);
        }
        assert!(has_layer(cx, window, &id));
        draw(cx, window);
        assert!(!has_layer(cx, window, &id));
    }

    #[crate::test]
    fn resize_drops_layers(cx: &mut TestAppContext) {
        let window = page(cx, false).into();
        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        let id = scroller_id(cx, window);
        cx.simulate_window_resize(window, size(px(800.), px(500.)));
        draw(cx, window);
        assert!(!has_layer(cx, window, &id));

        promote(cx, window);
        assert_eq!(scroll(cx, window, -20.), Some(Decision::Composite));
        cx.simulate_window_scale_factor_change(window, 1.5);
        draw(cx, window);
        assert!(!has_layer(cx, window, &id));
    }
}
