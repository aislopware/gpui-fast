//! Pixel tests of scroll layers on Metal: the same content drawn directly and
//! through layer tiles must come out byte for byte the same. They follow
//! `gpui_wgpu`'s `fast::layers::tests`.
//!
//! [`Harness`] draws scenes with a headless `MetalRenderer`, through
//! `render_frame` as a window's frames are drawn, into a texture it reads
//! back. Without a Metal device the tests print a skip and return.

use std::borrow::Cow;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AtlasKey, AtlasTile, Bounds, ContentMask, Corners, DevicePixels, Edges, FontId, GlyphId, Hsla,
    ImageId, LayerFrame, LayerKey, MonochromeSprite, Path, PlatformAtlas, PolychromeSprite, Quad,
    RenderGlyphParams, RenderImageParams, Rgba, ScaledPixels, Scene, SceneLayers, Shadow, Size,
    TileCoord, TransformationMatrix, Underline, decode_layer_tile, layer_tile_id,
    layer_tile_texture_id, linear_color_stop, linear_gradient, point, px, rgba, size,
};
use parking_lot::Mutex;

use crate::fast::layers::TileCache;
use crate::metal_renderer::binds::UPSTREAM_LOOP;
use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

const TILE: u32 = 512;

#[test]
fn harness_renders_a_quad_at_the_right_pixels() {
    let Some(mut harness) = Harness::new() else {
        return;
    };
    let mut scene = Scene::default();
    scene.insert_primitive(quad(sp(0., 0., 32., 32.), Hsla::from(rgba(0xffffffff))));
    scene.insert_primitive(quad(sp(4., 4., 8., 8.), Hsla::from(rgba(0xff0000ff))));
    scene.finish();
    let pixels = harness.render(&scene, device_size(32, 32));
    assert_eq!(pixel(&pixels, 32, 8, 8), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 32, 1, 1), [255, 255, 255, 255]);
}

/// Tiles composited where they were painted hold the content's own pixels,
/// across every tile edge.
#[test]
fn tiles_composited_in_place_equal_the_content_drawn_directly() {
    let Some(mut harness) = Harness::new() else {
        return;
    };
    let window = Window {
        size: device_size(1024, 1024),
        viewport: sp(0., 0., 1024., 1024.),
    };
    let tiles = [coord(0, 0), coord(1, 0), coord(0, 1), coord(1, 1)];
    composite_and_compare(&mut harness, &window, (0., 0.), (0., 0.), &tiles, false);
}

#[test]
fn tiles_of_negative_coordinates_composite_like_positive_ones() {
    let Some(mut harness) = Harness::new() else {
        return;
    };
    let window = Window {
        size: device_size(1024, 1024),
        viewport: sp(0., 0., 1024., 1024.),
    };
    let tiles = [coord(-1, -1), coord(0, -1), coord(-1, 0), coord(0, 0)];
    let side = TILE as f32;
    composite_and_compare(
        &mut harness,
        &window,
        (-side, -side),
        (side, side),
        &tiles,
        false,
    );
}

#[test]
fn a_composited_layer_equals_direct_drawing() {
    let Some(mut harness) = Harness::new() else {
        return;
    };
    let tiles = [coord(0, 0), coord(1, 0), coord(0, 1), coord(1, 1)];
    composite_and_compare(
        &mut harness,
        &scrolled_window(),
        (0., 0.),
        (37., -91.),
        &tiles,
        false,
    );
}

/// The same frame encoded by upstream's loop, whose polychrome sprite draw
/// composites tiles as the fork's bound draws do.
#[test]
fn a_composited_layer_equals_direct_drawing_through_upstreams_loop() {
    let Some(mut harness) = Harness::new() else {
        return;
    };
    let tiles = [coord(0, 0), coord(1, 0), coord(0, 1), coord(1, 1)];
    UPSTREAM_LOOP.with(|flag| flag.set(true));
    composite_and_compare(
        &mut harness,
        &scrolled_window(),
        (0., 0.),
        (37., -91.),
        &tiles,
        false,
    );
    UPSTREAM_LOOP.with(|flag| flag.set(false));
}

/// A path the frame draws over a layer, outside it, as the core draws paths
/// that nothing covers: the frame then takes `fast::paths`'s encoding.
#[test]
fn a_path_over_a_composited_layer_equals_direct_drawing() {
    let Some(mut harness) = Harness::new() else {
        return;
    };
    let tiles = [coord(0, 0), coord(1, 0), coord(0, 1), coord(1, 1)];
    composite_and_compare(
        &mut harness,
        &scrolled_window(),
        (0., 0.),
        (37., -91.),
        &tiles,
        true,
    );
}

/// A repainted layer's next generation re-rasterizes its dirty tile, which
/// then shows the new content.
#[test]
fn a_repainted_tile_shows_the_new_content() {
    let Some(mut harness) = Harness::new() else {
        return;
    };
    let window = scrolled_window();
    let translation = (37., -91.);
    let key = LayerKey(6);
    let tiles = [coord(0, 0), coord(1, 0), coord(0, 1), coord(1, 1)];

    let first = layer_frame(key, 1, content(&harness, (0., 0.), None), &tiles);
    harness.render(
        &composited(&window, first, &tiles, translation, false),
        window.size,
    );
    let rasterized = harness.cache().rasterized;

    // The content with a green square in tile (0, 0) only.
    fn changed(
        scene: &mut Scene,
        harness: &Harness,
        offset: (f32, f32),
        clip: Option<Bounds<ScaledPixels>>,
    ) {
        add_content(scene, harness, offset, clip);
        let mut square = quad(
            sp(200. + offset.0, 300. + offset.1, 40., 40.),
            Hsla::from(rgba(0x00ff00ff)),
        );
        if let Some(clip) = clip {
            square.content_mask = ContentMask { bounds: clip };
        }
        scene.insert_primitive(square);
    }
    let mut new_content = Scene::default();
    changed(&mut new_content, &harness, (0., 0.), None);
    new_content.finish();
    let second = layer_frame(key, 2, new_content, &[coord(0, 0)]);
    let actual = harness.render(
        &composited(&window, second, &tiles, translation, false),
        window.size,
    );
    assert_eq!(harness.cache().rasterized, rasterized + 1);

    let mut direct = Scene::default();
    direct.insert_primitive(panel(&window));
    changed(&mut direct, &harness, translation, Some(window.viewport));
    direct.insert_primitive(scrollbar());
    direct.finish();
    let expected = harness.render(&direct, window.size);
    assert_same_pixels(&actual, &expected, window.size.width.0 as usize);
}

/// A tile sprite of a layer the cache holds nothing of draws nothing, and
/// never reaches the atlas.
#[test]
fn a_tile_the_cache_lacks_draws_nothing() {
    let Some(mut harness) = Harness::new() else {
        return;
    };
    let window = scrolled_window();
    let mut expected = Scene::default();
    expected.insert_primitive(panel(&window));
    expected.insert_primitive(scrollbar());
    expected.finish();
    let expected = harness.render(&expected, window.size);

    // The scene composites tiles of layer 9 but describes layer 8 only.
    let layer = layer_frame(LayerKey(8), 1, Scene::default(), &[]);
    let mut scene = Scene::default();
    scene.insert_primitive(panel(&window));
    scene.push_layer(window.viewport);
    for tile in [coord(0, 0), coord(1, 0)] {
        scene.insert_primitive(tile_sprite(
            LayerKey(9),
            &layer,
            tile,
            (37., -91.),
            window.viewport,
        ));
    }
    scene.pop_layer();
    scene.insert_primitive(scrollbar());
    scene.layers.frames.push(layer);
    scene.finish();
    let actual = harness.render(&scene, window.size);
    assert_same_pixels(&actual, &expected, window.size.width.0 as usize);
}

#[test]
fn resize_releases_every_tile() {
    let Some(mut harness) = Harness::new() else {
        return;
    };
    let window = scrolled_window();
    let tiles = [coord(0, 0), coord(1, 0), coord(0, 1), coord(1, 1)];
    composite_and_compare(&mut harness, &window, (0., 0.), (37., -91.), &tiles, false);
    assert!(harness.cache().holds(LayerKey(5), coord(0, 0)));

    harness.renderer.update_drawable_size(window.size);
    assert!(harness.cache().is_empty());

    // The next frame rasterizes the tiles it composites again.
    let rasterized = harness.cache().rasterized;
    composite_and_compare(&mut harness, &window, (0., 0.), (37., -91.), &tiles, false);
    assert!(harness.cache().rasterized > rasterized);
}

#[test]
fn a_changed_generation_rerasterizes_only_dirty_tiles() {
    let mut cache = TileCache::default();
    let key = LayerKey(3);
    let all = [coord(0, 0), coord(1, 0), coord(0, 1), coord(1, 1)];
    let mut rasterized = 0;
    let mut frame =
        |cache: &mut TileCache, generation, dirty: &[TileCoord], shown: &[TileCoord]| {
            let layers = scene_layers(key, generation, dirty);
            let planned = cache.begin_frame(&layers, shown.iter().map(|tile| (key, *tile)));
            rasterized += planned.len();
            planned
                .into_iter()
                .map(|(_, tile)| tile)
                .collect::<Vec<_>>()
        };

    // A new layer: every dirty tile, shown or not.
    let mut sorted = all.to_vec();
    sorted.sort();
    assert_eq!(frame(&mut cache, 1, &all, &all[..2]), sorted);
    // The same generation again: nothing.
    assert_eq!(frame(&mut cache, 1, &all, &all[..2]), vec![]);
    // The next generation: only its dirty tile, although others are shown.
    assert_eq!(
        frame(&mut cache, 2, &[coord(1, 0)], &all),
        vec![coord(1, 0)]
    );
    assert_eq!(frame(&mut cache, 2, &[coord(1, 0)], &all), vec![]);
    // A skipped generation may have dirtied any tile: shown tiles again.
    assert_eq!(frame(&mut cache, 4, &[], &all[..1]), vec![coord(0, 0)]);
    // A shown tile the cache never had.
    assert_eq!(frame(&mut cache, 4, &[], &[coord(2, 0)]), vec![coord(2, 0)]);
    assert_eq!(rasterized, 7);
}

/// A layer's first generation dirties the overscan too: a frame rasterizes
/// the tiles it composites and a few of the others, nearest first, and the
/// frames after it the rest, a few at a time.
#[test]
fn dirty_tiles_not_composited_are_rasterized_a_few_a_frame_nearest_first() {
    use super::tile_cache::PREFETCH_TILES;
    let mut cache = TileCache::default();
    let key = LayerKey(7);
    // Ten rows of four tiles, the viewport over rows 0 and 1.
    let all: Vec<TileCoord> = (0..10)
        .flat_map(|y| (0..4).map(move |x| coord(x, y)))
        .collect();
    let shown: Vec<TileCoord> = all.iter().copied().filter(|tile| tile.y < 2).collect();
    let layers = scene_layers(key, 1, &all);
    let mut frame = || {
        cache
            .begin_frame(&layers, shown.iter().map(|tile| (key, *tile)))
            .into_iter()
            .map(|(_, tile)| tile)
            .collect::<Vec<_>>()
    };

    let first = frame();
    assert_eq!(first.len(), shown.len() + PREFETCH_TILES);
    assert!(shown.iter().all(|tile| first.contains(tile)));
    assert!(
        first
            .iter()
            .filter(|tile| !shown.contains(tile))
            .all(|tile| tile.y == 2),
        "the next row first: {first:?}"
    );
    let mut rasterized = first;
    loop {
        let next = frame();
        if next.is_empty() {
            break;
        }
        assert!(next.len() <= PREFETCH_TILES);
        rasterized.extend(next);
    }
    rasterized.sort();
    let mut all = all;
    all.sort();
    assert_eq!(rasterized, all, "every tile once");
}

#[test]
fn tiles_are_evicted_least_recently_composited_first_within_the_budget() {
    const SIDE: u32 = 16;
    let tile_bytes = (SIDE * SIDE * 4) as u64;
    let key = LayerKey(4);
    let mut cache = TileCache::default();
    let layers = SceneLayers {
        frames: vec![LayerFrame {
            tile_size: SIDE,
            ..layer_frame(key, 1, Scene::default(), &[])
        }],
    };
    let frame = |cache: &mut TileCache, shown: &[TileCoord], budget: u64| {
        cache.begin_frame(&layers, shown.iter().map(|tile| (key, *tile)));
        cache.evict_to_budget(budget);
    };
    let (a, b, c, d, e) = (
        coord(0, 0),
        coord(1, 0),
        coord(2, 0),
        coord(3, 0),
        coord(4, 0),
    );

    frame(&mut cache, &[a, b, c], 3 * tile_bytes);
    frame(&mut cache, &[b, c, d], 3 * tile_bytes);
    assert!(
        !cache.holds(key, a),
        "the least recently composited tile goes"
    );
    assert!(cache.holds(key, b) && cache.holds(key, c) && cache.holds(key, d));

    frame(&mut cache, &[c], 3 * tile_bytes);
    frame(&mut cache, &[d, e], tile_bytes);
    assert!(!cache.holds(key, b) && !cache.holds(key, c));
    assert!(
        cache.holds(key, d) && cache.holds(key, e),
        "tiles composited this frame stay, over budget or not"
    );
}

#[test]
fn tile_sprites_split_into_runs_of_one_tile() {
    let mut cache = TileCache::default();
    let key = LayerKey(2);
    let layer = layer_frame(key, 1, Scene::default(), &[]);
    let viewport = sp(0., 0., 1024., 1024.);
    let mut scene = Scene::default();
    for tile in [coord(0, 0), coord(1, 0), coord(0, 1)] {
        scene.insert_primitive(tile_sprite(key, &layer, tile, (0., 0.), viewport));
    }
    scene.layers.frames.push(layer);
    scene.finish();
    cache.note_composited(&scene);
    let runs: Vec<(TileCoord, std::ops::Range<usize>)> = cache.tile_runs(0..3).collect();
    assert_eq!(runs.len(), 3);
    for (tile, run) in &runs {
        assert_eq!(run.len(), 1);
        let sprite = &scene.polychrome_sprites[run.start];
        assert_eq!(
            decode_layer_tile(sprite.tile.texture_id, sprite.tile.tile_id),
            Some((key, *tile))
        );
    }
    assert_eq!(cache.tile_runs(1..2).count(), 1);
}

// --- layer helpers --- //

/// A window of `size` whose scroll container shows its layer in `viewport`.
struct Window {
    size: Size<DevicePixels>,
    viewport: Bounds<ScaledPixels>,
}

fn scrolled_window() -> Window {
    Window {
        size: device_size(700, 500),
        viewport: sp(40., 30., 600., 400.),
    }
}

fn panel(window: &Window) -> Quad {
    quad(
        sp(
            0.,
            0.,
            window.size.width.0 as f32,
            window.size.height.0 as f32,
        ),
        Hsla::from(background()),
    )
}

fn scrollbar() -> Quad {
    quad(sp(620., 40., 12., 120.), Hsla::from(rgba(0x888888ff)))
}

fn coord(x: i32, y: i32) -> TileCoord {
    TileCoord { x, y }
}

fn background() -> Rgba {
    rgba(0x336699ff)
}

fn layer_frame(key: LayerKey, generation: u64, content: Scene, dirty: &[TileCoord]) -> LayerFrame {
    LayerFrame {
        key,
        generation,
        background: background(),
        tile_size: TILE,
        content: Rc::new(content).into(),
        dirty_tiles: dirty.to_vec(),
    }
}

fn scene_layers(key: LayerKey, generation: u64, dirty: &[TileCoord]) -> SceneLayers {
    SceneLayers {
        frames: vec![layer_frame(key, generation, Scene::default(), dirty)],
    }
}

/// The sprite that composites `tile` of `layer`'s content, under the key
/// `key`, moved by `translation` and clipped to `viewport`.
fn tile_sprite(
    key: LayerKey,
    layer: &LayerFrame,
    tile: TileCoord,
    translation: (f32, f32),
    viewport: Bounds<ScaledPixels>,
) -> PolychromeSprite {
    let bounds = layer.tile_bounds(tile);
    PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false.into(),
        opacity: 1.,
        bounds: sp(
            bounds.origin.x.0 + translation.0,
            bounds.origin.y.0 + translation.1,
            TILE as f32,
            TILE as f32,
        ),
        content_mask: ContentMask { bounds: viewport },
        corner_radii: Corners::default(),
        tile: AtlasTile {
            texture_id: layer_tile_texture_id(key),
            tile_id: layer_tile_id(tile),
            padding: 0,
            bounds: Bounds {
                origin: point(DevicePixels(0), DevicePixels(0)),
                size: device_size(TILE as i32, TILE as i32),
            },
        },
    }
}

/// A frame of `window` that composites `tiles` of `layer` at `translation`,
/// between the panel under the viewport and a scrollbar over it, and, with
/// `path`, a path over both.
fn composited(
    window: &Window,
    layer: LayerFrame,
    tiles: &[TileCoord],
    translation: (f32, f32),
    path: bool,
) -> Scene {
    let mut scene = Scene::default();
    scene.insert_primitive(panel(window));
    scene.push_layer(window.viewport);
    for &tile in tiles {
        scene.insert_primitive(tile_sprite(
            layer.key,
            &layer,
            tile,
            translation,
            window.viewport,
        ));
    }
    scene.pop_layer();
    scene.insert_primitive(scrollbar());
    if path {
        add_path(&mut scene);
    }
    scene.layers.frames.push(layer);
    scene.finish();
    scene
}

/// Draws a window whose viewport shows the layer's content, painted at
/// `origin` in content space and moved by `translation`, once from the
/// content directly and once from the layer's tiles, twice (the second
/// frame from the cached tiles), and compares the pixels.
fn composite_and_compare(
    harness: &mut Harness,
    window: &Window,
    origin: (f32, f32),
    translation: (f32, f32),
    tiles: &[TileCoord],
    path: bool,
) {
    let mut direct = Scene::default();
    direct.insert_primitive(panel(window));
    add_content(
        &mut direct,
        harness,
        (origin.0 + translation.0, origin.1 + translation.1),
        Some(window.viewport),
    );
    direct.insert_primitive(scrollbar());
    if path {
        add_path(&mut direct);
    }
    direct.finish();
    let expected = harness.render(&direct, window.size);
    let width = window.size.width.0 as usize;

    let layer = layer_frame(LayerKey(5), 1, content(harness, origin, None), tiles);
    let scene = composited(window, layer, tiles, translation, path);
    let rasterized = harness.cache().rasterized;
    let actual = harness.render(&scene, window.size);
    assert_eq!(harness.cache().rasterized, rasterized + tiles.len());
    assert_same_pixels(&actual, &expected, width);

    // A frame that only scrolled draws the cached tiles again.
    let actual = harness.render(&scene, window.size);
    assert_eq!(harness.cache().rasterized, rasterized + tiles.len());
    assert_same_pixels(&actual, &expected, width);
}

fn assert_same_pixels(actual: &[u8], expected: &[u8], width: usize) {
    assert_eq!(actual.len(), expected.len());
    let differing: Vec<(usize, usize)> = (0..actual.len() / 4)
        .filter(|i| actual[i * 4..i * 4 + 4] != expected[i * 4..i * 4 + 4])
        .map(|i| (i % width, i / width))
        .collect();
    if differing.is_empty() {
        return;
    }
    let (min_x, max_x) = differing
        .iter()
        .fold((usize::MAX, 0), |(lo, hi), (x, _)| (lo.min(*x), hi.max(*x)));
    let (min_y, max_y) = differing
        .iter()
        .fold((usize::MAX, 0), |(lo, hi), (_, y)| (lo.min(*y), hi.max(*y)));
    panic!(
        "{} pixels differ, within x {min_x}..={max_x}, y {min_y}..={max_y}; first at {:?}: {:?} instead of {:?}",
        differing.len(),
        differing[0],
        pixel(actual, width, differing[0].0, differing[0].1),
        pixel(expected, width, differing[0].0, differing[0].1),
    );
}

/// Content of every primitive kind a layer holds, spread over the four
/// tiles of (0, 0)..(1024, 1024) and across their edges, moved by `offset`
/// and clipped to `clip`.
fn content(harness: &Harness, offset: (f32, f32), clip: Option<Bounds<ScaledPixels>>) -> Scene {
    let mut scene = Scene::default();
    add_content(&mut scene, harness, offset, clip);
    scene.finish();
    scene
}

/// Inserts [`content`]'s primitives into `scene`.
fn add_content(
    scene: &mut Scene,
    harness: &Harness,
    offset: (f32, f32),
    clip: Option<Bounds<ScaledPixels>>,
) {
    let clip = clip.unwrap_or(no_mask().bounds);
    let (ox, oy) = offset;
    let at = |x: f32, y: f32, w: f32, h: f32| sp(x + ox, y + oy, w, h);
    let mask = |bounds: Bounds<ScaledPixels>| ContentMask {
        bounds: bounds.intersect(&clip),
    };

    scene.insert_primitive(Quad {
        content_mask: mask(clip),
        ..quad(at(100., 100., 200., 150.), Hsla::from(rgba(0xcc3322ff)))
    });
    // A bordered, rounded quad across the tile edge at x = 512.
    scene.insert_primitive(Quad {
        bounds: at(450.5, 60., 150., 120.),
        content_mask: mask(at(0., 0., 1024., 1024.)),
        background: Hsla::from(rgba(0xeeeeeeff)).into(),
        border_color: Hsla::from(rgba(0x222222ff)),
        corner_radii: Corners::all(ScaledPixels(12.)),
        border_widths: Edges::all(ScaledPixels(3.)),
        ..Default::default()
    });
    // The shadow's tail crosses the tile edge its bounds stop short of.
    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: ScaledPixels(8.),
        bounds: at(380., 420., 120., 70.),
        corner_radii: Corners::all(ScaledPixels(6.)),
        content_mask: mask(clip),
        color: Hsla::from(rgba(0x00000080)),
        element_bounds: at(380., 420., 120., 70.),
        element_corner_radii: Corners::all(ScaledPixels(6.)),
        inset: 0,
        pad: 0,
    });
    scene.insert_primitive(Quad {
        bounds: at(600., 600., 300., 200.),
        content_mask: mask(clip),
        background: linear_gradient(
            45.,
            linear_color_stop(rgba(0xff0000ff), 0.),
            linear_color_stop(rgba(0x0000ffff), 1.),
        ),
        corner_radii: Corners::all(ScaledPixels(20.)),
        ..Default::default()
    });
    scene.insert_primitive(Underline {
        order: 0,
        pad: 0,
        bounds: at(50., 505., 900., 8.),
        content_mask: mask(clip),
        color: Hsla::from(rgba(0xffcc00ff)),
        thickness: ScaledPixels(2.),
        wavy: true.into(),
    });
    scene.insert_primitive(Underline {
        order: 0,
        pad: 0,
        bounds: at(60., 530., 700., 2.),
        content_mask: mask(clip),
        color: Hsla::from(rgba(0x114411ff)),
        thickness: ScaledPixels(1.),
        wavy: false.into(),
    });
    // A clipped quad whose mask crosses a tile edge.
    scene.insert_primitive(Quad {
        bounds: at(700., 200., 200., 200.),
        content_mask: mask(at(490., 250., 300., 100.)),
        background: Hsla::from(rgba(0x44aa44ff)).into(),
        ..Default::default()
    });

    let mono = glyph_tile(harness, 1);
    scene.insert_primitive(MonochromeSprite {
        order: 0,
        pad: 0,
        bounds: at(505., 700., 16., 16.),
        content_mask: mask(clip),
        color: Hsla::from(rgba(0xffffffff)),
        tile: mono,
        transformation: TransformationMatrix::unit(),
    });
    scene.insert_primitive(PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false.into(),
        opacity: 1.,
        bounds: at(300., 500., 20., 20.),
        content_mask: mask(clip),
        corner_radii: Corners::all(ScaledPixels(4.)),
        tile: image_tile(harness),
    });
}

/// A path of lines and a curve over the viewport's edge, in window space.
fn add_path(scene: &mut Scene) {
    let mut path = Path::new(point(px(480.), px(300.)));
    path.line_to(point(px(660.5), px(330.)));
    path.curve_to(point(px(500.), px(460.)), point(px(600.), px(450.)));
    path.line_to(point(px(480.), px(300.)));
    path.content_mask = ContentMask {
        bounds: no_mask().bounds.map(|c| px(c.0)),
    };
    path.color = Hsla::from(rgba(0x8800ffcc)).into();
    scene.insert_primitive(path.scale(1.));
}

/// A 16×16 monochrome glyph of a made-up font, uploaded to the renderer's
/// atlas.
fn glyph_tile(harness: &Harness, glyph: u32) -> AtlasTile {
    let key = AtlasKey::Glyph(RenderGlyphParams {
        font_id: FontId(9_999),
        glyph_id: GlyphId(glyph),
        font_size: px(12.),
        subpixel_variant: point(0, 0),
        scale_factor: 1.,
        is_emoji: false,
        subpixel_rendering: false,
        dilation: 0,
    });
    let bytes: Vec<u8> = (0..16 * 16).map(|i| ((i * 37) % 256) as u8).collect();
    harness
        .renderer
        .sprite_atlas()
        .get_or_insert_with(key, &mut || {
            Ok(Some((device_size(16, 16), Cow::Owned(bytes.clone()))))
        })
        .expect("glyph uploaded")
        .expect("glyph tile")
}

/// A 20×20 image uploaded to the renderer's atlas.
fn image_tile(harness: &Harness) -> AtlasTile {
    let key = AtlasKey::Image(RenderImageParams {
        image_id: ImageId(9_999),
        frame_index: 0,
    });
    let bytes: Vec<u8> = (0..20 * 20)
        .flat_map(|i: u32| {
            [
                (i * 7 % 256) as u8,
                (i * 13 % 256) as u8,
                (i * 3 % 256) as u8,
                255,
            ]
        })
        .collect();
    harness
        .renderer
        .sprite_atlas()
        .get_or_insert_with(key, &mut || {
            Ok(Some((device_size(20, 20), Cow::Owned(bytes.clone()))))
        })
        .expect("image uploaded")
        .expect("image tile")
}

// --- helpers --- //

fn sp(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(w), ScaledPixels(h)),
    }
}

/// A content mask that clips nothing the tests draw.
fn no_mask() -> ContentMask<ScaledPixels> {
    ContentMask {
        bounds: sp(-10_000., -10_000., 20_000., 20_000.),
    }
}

fn quad(bounds: Bounds<ScaledPixels>, color: Hsla) -> Quad {
    Quad {
        bounds,
        content_mask: no_mask(),
        background: color.into(),
        ..Default::default()
    }
}

fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    size(DevicePixels(width), DevicePixels(height))
}

/// The RGBA bytes of the pixel at (`x`, `y`) of a `width`-wide readback.
fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let at = (y * width + x) * 4;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

/// Draws scenes with a headless renderer, as a window's frames are drawn.
struct Harness {
    renderer: MetalRenderer,
}

impl Harness {
    /// A harness on the system's Metal device, or `None` when there is none
    /// (the tests then skip).
    fn new() -> Option<Harness> {
        if metal::Device::system_default().is_none() && metal::Device::all().is_empty() {
            eprintln!("skipped: no Metal device");
            return None;
        }
        let pool = Arc::new(Mutex::new(InstanceBufferPool::default()));
        Some(Harness {
            renderer: MetalRenderer::new_headless(pool),
        })
    }

    fn cache(&self) -> &TileCache {
        &self.renderer.fast_layers
    }

    /// Draws `scene` into a `size` texture as the renderer draws a frame,
    /// and returns its pixels as RGBA bytes, row by row.
    fn render(&mut self, scene: &Scene, size: Size<DevicePixels>) -> Vec<u8> {
        self.renderer
            .render_scene_to_image(scene, size)
            .expect("scene rendered")
            .into_raw()
    }
}

/// What scrolling a settings-like page costs the renderer, drawn directly
/// and through layer tiles: per frame, the CPU time `render_frame` takes to
/// encode and the GPU time of the frame's command buffer, plus the frame
/// that rasterizes every tile. The page, at 2x, is 1600 × 5000 device
/// pixels of 40-pixel rows, each a background, a label of 40 glyphs, a
/// toggle and a separator; the viewport is 1600 × 1000 and scrolls 12
/// pixels a frame.
///
/// `cargo test -p gpui_apple --release --lib scroll_frame_gpu_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, not a check"]
fn scroll_frame_gpu_cost() {
    use objc::{msg_send, sel, sel_impl};
    use std::time::{Duration, Instant};

    let Some(mut harness) = Harness::new() else {
        return;
    };
    const FRAMES: usize = 300;
    const STEP: f32 = 12.;
    let window = Window {
        size: device_size(1800, 1200),
        viewport: sp(100., 100., 1600., 1000.),
    };
    let glyphs: Vec<AtlasTile> = (0..26).map(|glyph| glyph_tile(&harness, glyph)).collect();
    let page = |scene: &mut Scene, offset: (f32, f32), clip: Bounds<ScaledPixels>| {
        let mask = ContentMask { bounds: clip };
        for row in 0..125 {
            let y = offset.1 + row as f32 * 40.;
            if y + 40. < clip.origin.y.0 || y > clip.origin.y.0 + clip.size.height.0 {
                // What a list or a culling paint leaves out.
                continue;
            }
            let x = offset.0;
            scene.insert_primitive(Quad {
                content_mask: mask,
                ..quad(
                    sp(x, y, 1600., 40.),
                    Hsla::from(if row % 2 == 0 {
                        rgba(0x1e1e22ff)
                    } else {
                        rgba(0x232328ff)
                    }),
                )
            });
            for (i, tile) in glyphs.iter().cycle().skip(row).take(40).enumerate() {
                scene.insert_primitive(MonochromeSprite {
                    order: 0,
                    pad: 0,
                    bounds: sp(x + 24. + i as f32 * 17., y + 12., 16., 16.),
                    content_mask: mask,
                    color: Hsla::from(rgba(0xe6e6e6ff)),
                    tile: *tile,
                    transformation: TransformationMatrix::unit(),
                });
            }
            scene.insert_primitive(Quad {
                bounds: sp(x + 1500., y + 8., 48., 24.),
                content_mask: mask,
                background: Hsla::from(rgba(0x3fa66bff)).into(),
                border_color: Hsla::from(rgba(0x2b2b30ff)),
                corner_radii: Corners::all(ScaledPixels(12.)),
                border_widths: Edges::all(ScaledPixels(1.)),
                ..Default::default()
            });
            scene.insert_primitive(Underline {
                order: 0,
                pad: 0,
                bounds: sp(x + 16., y + 39., 1568., 1.),
                content_mask: mask,
                color: Hsla::from(rgba(0x333338ff)),
                thickness: ScaledPixels(1.),
                wavy: false.into(),
            });
        }
    };

    let texture = {
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_width(window.size.width.0 as u64);
        descriptor.set_height(window.size.height.0 as u64);
        descriptor.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
        descriptor
            .set_usage(metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead);
        descriptor.set_storage_mode(metal::MTLStorageMode::Private);
        harness.renderer.device.new_texture(&descriptor)
    };
    // Instructions the calling thread has retired: unlike time, steady on a
    // busy machine.
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
    let encoded = std::cell::Cell::new(0u64);
    // CPU time encoding, GPU time of the frame's command buffer, and wall
    // time until it completed, from the start of encoding.
    let draw = |renderer: &mut MetalRenderer, scene: &Scene| -> (Duration, Duration, Duration) {
        objc2::rc::autoreleasepool(|_| {
            let start = Instant::now();
            let before = instructions();
            let command_buffer = renderer
                .render_frame(scene, &texture, window.size)
                .expect("frame encoded");
            let cpu = start.elapsed();
            encoded.set(instructions() - before);
            command_buffer.commit();
            command_buffer.wait_until_completed();
            let wall = start.elapsed();
            // SAFETY: `GPUStartTime` and `GPUEndTime` are `MTLCommandBuffer`
            // properties, read after the buffer completed.
            let (gpu_start, gpu_end): (f64, f64) = unsafe {
                (
                    msg_send![command_buffer.as_ref(), GPUStartTime],
                    msg_send![command_buffer.as_ref(), GPUEndTime],
                )
            };
            (
                cpu,
                Duration::from_secs_f64((gpu_end - gpu_start).max(0.)),
                wall,
            )
        })
    };
    let offset_at = |frame: usize| (frame as f32 * STEP) % 3900.;
    let report = |name: &str, samples: &mut Vec<(Duration, Duration, Duration)>| {
        let median = |mut values: Vec<Duration>| {
            values.sort();
            values[values.len() / 2]
        };
        let p95 = |mut values: Vec<Duration>| {
            values.sort();
            values[values.len() * 95 / 100]
        };
        let cpu: Vec<_> = samples.iter().map(|s| s.0).collect();
        let gpu: Vec<_> = samples.iter().map(|s| s.1).collect();
        let wall: Vec<_> = samples.iter().map(|s| s.2).collect();
        eprintln!(
            "{name}: encode median {:?} p95 {:?}; gpu median {:?} p95 {:?}; to completion median {:?} p95 {:?}",
            median(cpu.clone()),
            p95(cpu),
            median(gpu.clone()),
            p95(gpu),
            median(wall.clone()),
            p95(wall),
        );
    };

    // Directly: the visible rows drawn into the frame at the offset.
    let mut direct = Vec::with_capacity(FRAMES);
    let mut direct_instructions = 0;
    for frame in 0..FRAMES + 30 {
        let offset = offset_at(frame);
        let mut scene = Scene::default();
        scene.insert_primitive(panel(&window));
        page(&mut scene, (100., 100. - offset), window.viewport);
        scene.insert_primitive(scrollbar());
        scene.finish();
        let sample = draw(&mut harness.renderer, &scene);
        if frame >= 30 {
            direct.push(sample);
            direct_instructions += encoded.get();
        }
    }
    report("direct", &mut direct);
    eprintln!(
        "direct: {} instructions encoding a frame",
        direct_instructions / FRAMES as u64
    );

    // Through tiles: the whole page painted once into a layer; each frame
    // composites the tiles over the viewport.
    let mut content = Scene::default();
    page(&mut content, (0., 0.), sp(0., 0., 1600., 5000.));
    content.finish();
    let all_tiles: Vec<TileCoord> = (0..10)
        .flat_map(|y| (0..4).map(move |x| coord(x, y)))
        .collect();
    let layer = layer_frame(LayerKey(11), 1, content, &all_tiles);
    let composite = |offset: f32| {
        let tiles: Vec<TileCoord> = all_tiles
            .iter()
            .copied()
            .filter(|tile| {
                let bounds = layer.tile_bounds(*tile);
                bounds.origin.y.0 < offset + 1000. && bounds.origin.y.0 + TILE as f32 > offset
            })
            .collect();
        composited(&window, layer.clone(), &tiles, (100., 100. - offset), false)
    };
    let raster = draw(&mut harness.renderer, &composite(0.));
    eprintln!(
        "raster all {} tiles ({} MB): to completion {:?}",
        harness.cache().rasterized,
        harness.cache().rasterized,
        raster.2
    );
    eprintln!("raster: {} instructions encoding", encoded.get());
    let mut tiled = Vec::with_capacity(FRAMES);
    let mut tiled_instructions = 0;
    for frame in 0..FRAMES + 30 {
        let sample = draw(&mut harness.renderer, &composite(offset_at(frame)));
        if frame >= 30 {
            tiled.push(sample);
            tiled_instructions += encoded.get();
        }
    }
    report("tiles", &mut tiled);
    eprintln!(
        "tiles: {} instructions encoding a frame",
        tiled_instructions / FRAMES as u64
    );

    // The same content as a new generation, every tile dirty: rasterized
    // again into the textures the cache holds.
    let before = instructions();
    let scenes = layer.tile_scenes(&all_tiles);
    eprintln!(
        "tile_scenes of all {} tiles: {} instructions, {} sprites",
        all_tiles.len(),
        instructions() - before,
        scenes
            .iter()
            .map(|scene| scene.monochrome_sprites.len())
            .sum::<usize>()
    );
    let before = instructions();
    for tile in &all_tiles {
        std::hint::black_box(layer.tile_scene(*tile));
    }
    eprintln!(
        "tile_scene of each: {} instructions",
        instructions() - before
    );
    let mut layer = layer;
    for generation in 2..5 {
        layer.generation = generation;
        let tiles: Vec<TileCoord> = all_tiles.iter().copied().filter(|t| t.y < 2).collect();
        let scene = composited(&window, layer.clone(), &tiles, (100., 100.), false);
        let again = draw(&mut harness.renderer, &scene);
        eprintln!(
            "raster all {} tiles again, textures kept: encode {:?} ({} instructions), to completion {:?}",
            all_tiles.len(),
            again.0,
            encoded.get(),
            again.2
        );
    }
}
