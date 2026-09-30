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

    fn row_color(row: usize) -> Hsla {
        crate::hsla(row as f32 / 64., 0.5, 0.5, 1.)
    }

    /// A panel, white at first, holding a 100 px scroll container of `rows` rows of
    /// 20 px, each its own colour.
    struct Rows {
        rows: usize,
        panel: Hsla,
        scroll: crate::ScrollHandle,
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
                    .h(crate::px(100.))
                    .children(
                        (0..self.rows).map(|i| crate::div().h(crate::px(20.)).bg(row_color(i))),
                    ),
            )
        }
    }

    fn rows_window(cx: &mut crate::TestAppContext, rows: usize) -> crate::WindowHandle<Rows> {
        let window = cx.add_window(|_, _| Rows {
            rows,
            panel: crate::white(),
            scroll: crate::ScrollHandle::new(),
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
}
