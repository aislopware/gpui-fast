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
}

/// Tests of what a scroll container decides to do with its layer each frame
/// (M4). The paint stream's hook around a container's children is stood in
/// for by two probes, painted before and after the rows, which ask
/// [`decide`] and, when it says [`Decision::Repaint`], record what the rows
/// read and painted into the layer as the hook does.
mod decisions {
    use super::invalidation::{draw, with_window};
    use crate::fast::dependencies::{DependencyRecording, RenderDependencies};
    use crate::fast::layers::policy::{Decision, decide, last_decision};
    use crate::fast::layers::record::LayerRecord;
    use crate::{
        AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity,
        GlobalElementId, Hsla, InteractiveElement as _, IntoElement, MouseMoveEvent,
        ParentElement as _, Pixels, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
        StatefulInteractiveElement as _, Styled as _, TestAppContext, TouchPhase, Window,
        WindowHandle, canvas, div, point, px, rgb, size,
    };
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    pub(super) const ROWS: usize = 40;
    pub(super) const ROW_HEIGHT: f32 = 20.;

    pub(super) fn viewport() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(200.), px(100.)),
        }
    }

    /// What the probes hand from one to the other within a frame.
    #[derive(Default)]
    pub(super) struct Probe {
        open: Option<Open>,
    }

    struct Open {
        id: GlobalElementId,
        recording: Option<DependencyRecording>,
        prepaint_dependencies: RenderDependencies,
        record: LayerRecord,
        paint: Option<PaintStart>,
    }

    struct PaintStart {
        recording: DependencyRecording,
        hovers: usize,
        paths: usize,
    }

    /// The probe before the children: decides, and starts recording them
    /// when they are painted into the layer.
    fn probe_before(
        probe: Rc<RefCell<Probe>>,
        content_width: f32,
        content_height: f32,
    ) -> impl IntoElement {
        let paint_probe = probe.clone();
        canvas(
            move |_, window, cx| {
                let id = crate::fast::global_id::current(window);
                let scroll_offset = window.element_offset();
                let content_size = size(px(content_width), px(content_height));
                let decision = decide(window, cx, &id, viewport(), content_size, scroll_offset);
                if decision != Decision::Repaint {
                    return;
                }
                let content = Bounds {
                    origin: viewport().origin + scroll_offset,
                    size: content_size,
                };
                // Overscan on the scrolled axis only, as the paint stream
                // paints it.
                let overscan = viewport().size.height;
                let painted_region = Bounds::from_corners(
                    point(viewport().left(), viewport().top() - overscan),
                    point(viewport().right(), viewport().bottom() + overscan),
                )
                .intersect(&content);
                let start = window.prepaint_index();
                probe.borrow_mut().open = Some(Open {
                    id,
                    recording: Some(cx.begin_recording_dependencies()),
                    prepaint_dependencies: RenderDependencies::default(),
                    record: LayerRecord {
                        painted_region,
                        viewport: viewport(),
                        scroll_offset,
                        prepaint_range: start.clone()..start,
                        ..LayerRecord::default()
                    },
                    paint: None,
                });
            },
            move |_, _, window, cx| {
                if let Some(open) = paint_probe.borrow_mut().open.as_mut() {
                    window.take_hover_reads();
                    open.record.paint_range.start = window.paint_index();
                    open.paint = Some(PaintStart {
                        recording: cx.begin_recording_dependencies(),
                        hovers: window.retained_state.hover_dependencies.len(),
                        paths: window.next_frame.scene.paths.len(),
                    });
                }
            },
        )
    }

    /// The probe after the children: keeps what they read and painted in
    /// the layer.
    fn probe_after(probe: Rc<RefCell<Probe>>) -> impl IntoElement {
        let paint_probe = probe.clone();
        canvas(
            move |_, window, cx| {
                if let Some(open) = probe.borrow_mut().open.as_mut()
                    && let Some(recording) = open.recording.take()
                {
                    open.prepaint_dependencies = cx.finish_recording_dependencies(recording).all;
                    open.record.prepaint_range.end = window.prepaint_index();
                }
            },
            move |_, _, window, cx| {
                let Some(mut open) = paint_probe.borrow_mut().open.take() else {
                    return;
                };
                let Some(paint) = open.paint.take() else {
                    return;
                };
                let paint_dependencies = cx.finish_recording_dependencies(paint.recording).all;
                window.take_hover_reads();
                let mut record = open.record;
                record.hovers = window.retained_state.hover_dependencies[paint.hovers..].into();
                record.paint_range.end = window.paint_index();
                record.has_paths = window.next_frame.scene.paths.len() > paint.paths;
                record.dependencies = open.prepaint_dependencies.union(&paint_dependencies);
                if let Some(layer) = window.fast_layers.layers.get_mut(&open.id) {
                    layer.record = Some(record);
                }
            },
        )
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
        pub(super) probe: Rc<RefCell<Probe>>,
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
            let content_height = ROWS as f32 * ROW_HEIGHT;
            let mut scroller = div()
                .id("scroller")
                .overflow_y_scroll()
                .track_scroll(&self.handle)
                .w(viewport().size.width)
                .h(viewport().size.height)
                .child(probe_before(
                    self.probe.clone(),
                    content_width,
                    content_height,
                ));
            if let Some(extra) = &self.extra {
                scroller = scroller.child(extra());
            }
            scroller = match &self.rows {
                Some(rows) => scroller.child(rows.clone()),
                None => {
                    scroller.children((0..ROWS).map(|index| row(index, self.hover, content_width)))
                }
            };
            div()
                .size_full()
                .child(scroller.child(probe_after(self.probe.clone())))
        }
    }

    pub(super) fn new_page(child_view: bool, cx: &mut App) -> LayerPage {
        LayerPage {
            handle: ScrollHandle::new(),
            probe: Rc::default(),
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
        assert_eq!(layers_demoted(cx, window), 1);
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
