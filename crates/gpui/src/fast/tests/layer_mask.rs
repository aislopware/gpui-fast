//! Layer masks (`Window::begin_layer_mask`, `end_layer_mask`): their two
//! cuts' places around the group they bracket, frames drawn again from last
//! frame placing them as frames drawn from scratch do, and content masking
//! kept off scroll layers' tiles. See [`crate::fast::layer_mask`]; the
//! pixels are `gpui_apple`'s `fast::layer_mask` tests.

use std::sync::Arc;

use super::element_oracle::GlyphBoxTextSystem;
use crate::{
    AnyElement, AnyWindowHandle, App, Bounds, ContentMask, Context, Corners, Entity, LayerMask,
    LayerMaskCut, LayoutStats, NoopTextSystem, PlatformInput, Render, ScaledPixels, Scene,
    ScrollDelta, ScrollEvent, ScrollHandle, TestAppContext, TouchPhase, Window, WindowHandle,
    canvas, composition::ComposedBatch, div, hsla, point, prelude::*, px, size,
};

fn text_cx() -> TestAppContext {
    TestAppContext::with_text_system(Arc::new(GlyphBoxTextSystem(NoopTextSystem)))
}

fn with_window<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| f(window, cx))
        .unwrap()
}

/// onicode's project background: a canvas beginning the mask over the
/// whole card, the group (here a fill and a label for the image), and a
/// canvas ending it, siblings as the app paints them.
fn masked(mask: LayerMask, group: impl IntoElement) -> AnyElement {
    let cut = move |begin: bool| {
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if begin {
                    window.begin_layer_mask(bounds, Corners::all(px(11.)), mask);
                } else {
                    window.end_layer_mask();
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    };
    div()
        .relative()
        .size_full()
        .child(cut(true))
        .child(group)
        .child(cut(false))
        .into_any_element()
}

fn haze() -> LayerMask {
    LayerMask::project_background(false, true, false, 0.8)
}

/// A striped page, a masked card over it, and a composer painted over the
/// card after the mask ends.
struct Card;

impl Render for Card {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .children((0..10).map(|ix| div().h(px(20.)).bg(hsla(ix as f32 / 10., 0.5, 0.5, 1.))))
            .child(
                div()
                    .absolute()
                    .top(px(10.))
                    .left(px(20.))
                    .w(px(120.))
                    .h(px(80.))
                    .child(masked(
                        haze(),
                        div().size_full().bg(hsla(0.6, 0.8, 0.5, 1.)).child("art"),
                    )),
            )
            .child(
                div()
                    .absolute()
                    .top(px(60.))
                    .left(px(30.))
                    .w(px(80.))
                    .h(px(20.))
                    .bg(hsla(0., 0., 1., 1.)),
            )
    }
}

/// A mask's begin takes a draw order over the stripes under its region, the
/// group lies between it and its end, and what is painted after the end
/// over the region is above it; a renderer meets each cut as a batch of
/// its own between them. The cuts carry the mask, the corners and the
/// scale in device pixels.
#[test]
fn a_mask_brackets_its_group_between_what_it_covers_and_what_follows() {
    let mut cx = text_cx();
    let window = cx.add_window(|_, _| Card);
    cx.test_window(window.into())
        .simulate_scale_factor_change(2.);
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let scene = &window.rendered_frame.scene;
        let cuts = scene.layer_masks();
        assert_eq!(cuts.len(), 2);
        let (begin, end) = (cuts[0], cuts[1]);
        assert!(begin.begin && !end.begin);
        assert_eq!(begin.id, end.id);
        assert_eq!(begin.bounds, end.bounds);
        assert_eq!(
            begin.bounds,
            Bounds::new(
                point(ScaledPixels(40.), ScaledPixels(20.)),
                size(ScaledPixels(240.), ScaledPixels(160.))
            )
        );
        assert_eq!(begin.corner_radii, Corners::all(ScaledPixels(22.)));
        assert_eq!(begin.scale, 2.);
        assert_eq!(begin.mask, haze());
        assert_eq!(begin.mask.mode(), 4);
        assert!(begin.order < end.order);

        let region = begin.clipped_bounds();
        let over = |quad: &&crate::Quad| quad.bounds.intersects(&region);
        let stripes: Vec<_> = scene
            .quads
            .iter()
            .filter(over)
            .filter(|quad| {
                quad.bounds.size.height == ScaledPixels(40.) && quad.bounds.origin.x.0 == 0.
            })
            .collect();
        assert!(stripes.len() >= 3);
        assert!(stripes.iter().all(|quad| quad.order < begin.order));
        let art = scene
            .quads
            .iter()
            .find(|quad| quad.bounds == region)
            .expect("the group's fill");
        assert!(begin.order < art.order && art.order < end.order);
        assert!(
            scene
                .monochrome_sprites
                .iter()
                .all(|sprite| begin.order < sprite.order && sprite.order < end.order),
            "the label is in the group"
        );
        let composer = scene
            .quads
            .iter()
            .find(|quad| {
                quad.bounds.size.height == ScaledPixels(40.) && quad.bounds.origin.x.0 == 60.
            })
            .expect("the composer");
        assert!(
            composer.order > end.order,
            "the composer is painted over the mask"
        );

        // A renderer meets the begin after the stripes and before the art,
        // and the end after the art and before the composer.
        let batches: Vec<_> = scene.composed_batches().collect();
        let met = |index: usize| {
            batches
                .iter()
                .position(
                    |batch| matches!(batch, ComposedBatch::Masks(range) if range.contains(&index)),
                )
                .expect("the cut is met")
        };
        let (at_begin, at_end) = (met(0), met(1));
        assert!(at_begin < at_end);
        let quad_orders = |batches: &[ComposedBatch]| -> Vec<u32> {
            batches
                .iter()
                .filter_map(|batch| match batch {
                    ComposedBatch::Primitives(crate::PrimitiveBatch::Quads(range)) => Some(range),
                    _ => None,
                })
                .flat_map(|range| scene.quads[range.clone()].iter().map(|quad| quad.order))
                .collect()
        };
        assert!(
            quad_orders(&batches[..at_begin])
                .iter()
                .all(|order| *order < begin.order)
        );
        assert!(quad_orders(&batches[at_begin..at_end]).contains(&art.order));
        assert!(quad_orders(&batches[at_end..]).contains(&composer.order));
        assert!(!quad_orders(&batches[..at_end]).contains(&composer.order));
    });
}

fn cut(begin: bool, bounds: Bounds<ScaledPixels>) -> LayerMaskCut {
    LayerMaskCut {
        order: 0,
        id: 0,
        begin,
        bounds,
        content_mask: ContentMask {
            bounds: Bounds::new(
                point(ScaledPixels(0.), ScaledPixels(0.)),
                size(ScaledPixels(1000.), ScaledPixels(1000.)),
            ),
        },
        corner_radii: Corners::default(),
        mask: LayerMask::project_background(true, false, false, 1.),
        scale: 1.,
        quads_before: 0,
    }
}

fn sp(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x), ScaledPixels(y)),
        size(ScaledPixels(width), ScaledPixels(height)),
    )
}

/// Masks apart, painted one after the other, may share their begins' and
/// their ends' draw orders: each end still pairs with its own begin, and the
/// cuts of one order are one batch. A nested mask's cuts lie inside its
/// parent's.
#[test]
fn masks_pair_by_id_apart_or_nested() {
    let mut scene = Scene::default();
    let (left, right) = (sp(0., 0., 50., 50.), sp(100., 0., 50., 50.));
    for bounds in [left, right] {
        scene.insert_layer_mask(cut(true, bounds));
        scene.insert_primitive(crate::Quad {
            bounds,
            content_mask: cut(true, bounds).content_mask,
            ..Default::default()
        });
        scene.insert_layer_mask(cut(false, bounds));
    }
    let outer = sp(0., 100., 100., 100.);
    let inner = sp(10., 110., 50., 50.);
    scene.insert_layer_mask(cut(true, outer));
    scene.insert_layer_mask(cut(true, inner));
    scene.insert_layer_mask(cut(false, inner));
    scene.insert_layer_mask(cut(false, outer));
    scene.finish();
    let cuts = scene.layer_masks();
    assert_eq!(cuts.len(), 8);
    let find = |bounds, begin| {
        *cuts
            .iter()
            .find(|cut| cut.bounds == bounds && cut.begin == begin)
            .unwrap()
    };
    let (left_begin, left_end) = (find(left, true), find(left, false));
    let (right_begin, right_end) = (find(right, true), find(right, false));
    assert_eq!(left_begin.order, right_begin.order);
    assert_eq!(left_end.order, right_end.order);
    assert_eq!(left_begin.id, left_end.id);
    assert_eq!(right_begin.id, right_end.id);
    assert_ne!(left_begin.id, right_begin.id);
    let batches: Vec<_> = scene
        .composed_batches()
        .filter_map(|batch| match batch {
            ComposedBatch::Masks(range) => Some(range),
            _ => None,
        })
        .collect();
    assert!(
        batches
            .iter()
            .any(|range| range.len() >= 2 && cuts[range.clone()].iter().all(|cut| cut.begin)),
        "the begins at one order are one batch: {batches:?}"
    );
    let (outer_begin, outer_end) = (find(outer, true), find(outer, false));
    let (inner_begin, inner_end) = (find(inner, true), find(inner, false));
    assert!(outer_begin.order < inner_begin.order);
    assert!(inner_begin.order < inner_end.order);
    assert!(inner_end.order < outer_end.order);
    assert_eq!(outer_begin.id, outer_end.id);
    assert_eq!(inner_begin.id, inner_end.id);
}

/// A mask never ended is not drawn, an end without a begin is dropped, and
/// a mask whose region shows nothing places no cut but still pairs.
#[test]
fn an_unended_mask_and_a_stray_end_draw_nothing() {
    let mut scene = Scene::default();
    scene.insert_layer_mask(cut(false, sp(0., 0., 10., 10.)));
    scene.insert_layer_mask(cut(true, sp(0., 0., 0., 10.)));
    scene.insert_layer_mask(cut(true, sp(20., 0., 10., 10.)));
    scene.insert_layer_mask(cut(false, sp(20., 0., 10., 10.)));
    scene.insert_layer_mask(cut(false, sp(0., 0., 0., 10.)));
    scene.insert_layer_mask(cut(true, sp(40., 0., 10., 10.)));
    scene.finish();
    let cuts = scene.layer_masks();
    assert_eq!(cuts.len(), 2, "{cuts:?}");
    assert!(cuts.iter().all(|cut| cut.bounds.origin.x.0 == 20.));
}

/// A masked card moving down the window, with a masked nested view
/// notified alone.
struct Sliding {
    spacer: f32,
    nested: Entity<Nested>,
}

struct Nested {
    count: usize,
}

impl Render for Nested {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().h(px(30.)).w(px(120.)).child(masked(
            LayerMask::project_background(true, false, false, 1.),
            div()
                .size_full()
                .bg(hsla(0.3, 0.5, 0.5, 1.))
                .child(format!("nested {}", self.count)),
        ))
    }
}

impl Render for Sliding {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(0., 0., 0.9, 1.))
            .child(div().flex_none().h(px(self.spacer)))
            .child(
                div().w(px(160.)).h(px(120.)).child(masked(
                    haze(),
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .bg(hsla(0.7, 0.5, 0.5, 1.))
                        .child(self.nested.clone())
                        .child("card"),
                )),
            )
    }
}

/// A masked card moving, with a masked nested view notified alone, drawn
/// incrementally, places its cuts as a window drawn from scratch does.
#[test]
fn a_mask_drawn_again_lies_as_drawn_afresh() {
    let mut cx = text_cx();
    let new = |cx: &mut TestAppContext| {
        cx.add_window(|_, cx| Sliding {
            spacer: 0.,
            nested: cx.new(|_| Nested { count: 0 }),
        })
    };
    let windows = [new(&mut cx), new(&mut cx)];
    let atlas = with_window(&mut cx, windows[0].into(), |window, _| {
        window.sprite_atlas.clone()
    });
    with_window(&mut cx, windows[1].into(), |window, _| {
        window.sprite_atlas = atlas;
    });
    let mut stats = LayoutStats::default();
    for frame in 0..30 {
        for window in windows {
            window
                .update(&mut cx, |sliding, _, cx| {
                    if frame % 3 == 0 {
                        sliding.spacer += 0.5;
                    }
                    if frame % 3 == 2 {
                        sliding.nested.update(cx, |nested, cx| {
                            nested.count += 1;
                            cx.notify();
                        });
                    } else {
                        cx.notify();
                    }
                })
                .unwrap();
        }
        let draw = |cx: &mut TestAppContext, window: WindowHandle<Sliding>, forget: bool| {
            with_window(cx, window.into(), |window, cx| {
                if forget {
                    window.forget_retained_state();
                }
                window.draw(cx).clear(cx);
                let stats = window.layout_stats();
                window.reset_layout_stats();
                (window.describe_rendered_frame(), stats)
            })
        };
        let (expected, _) = draw(&mut cx, windows[1], true);
        let (actual, frame_stats) = draw(&mut cx, windows[0], false);
        assert_eq!(actual, expected, "frame {frame}");
        assert_eq!(
            actual
                .iter()
                .filter(|line| line.contains("layer mask"))
                .count(),
            4,
            "frame {frame}"
        );
        stats.views_reused += frame_stats.views_reused;
    }
    assert!(stats.views_reused > 0, "{stats:?}");
}

/// A scrolling column whose every seventh row is masked.
struct Scrolled {
    scroll: ScrollHandle,
}

impl Render for Scrolled {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .id("scrolled")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children((0..60).map(move |ix| {
                let row = div()
                    .h(px(20.))
                    .bg(hsla(ix as f32 / 60., 0.5, 0.5, 1.))
                    .child(format!("row {ix}"));
                if ix % 7 == 3 {
                    row.child(div().absolute().w(px(80.)).h(px(20.)).child(masked(
                        haze(),
                        div().size_full().bg(hsla(0.1, 0.9, 0.5, 1.)),
                    )))
                    .relative()
                } else {
                    row
                }
            }));
        div()
            .size_full()
            .bg(hsla(0., 0., 1., 1.))
            .child(div().w(px(300.)).h(px(200.)).child(content))
    }
}

fn wheel_and_draw(cx: &mut TestAppContext, window: AnyWindowHandle, dy: f32) -> Vec<String> {
    with_window(cx, window, |window, cx| {
        window.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: point(px(20.), px(20.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                modifiers: Default::default(),
                touch_phase: TouchPhase::Moved,
                source: crate::ScrollSource::Pointer,
                momentum_phase: None,
            }),
            cx,
        );
        window.draw(cx).clear(cx);
        window.painted_primitives()
    })
}

/// Content masking a group is drawn without its layer's tiles, which hold
/// it over the layer's background alone, and draws as without layers.
#[test]
fn content_that_masks_is_drawn_without_tiles() {
    let mut cx = text_cx();
    let new = |cx: &mut TestAppContext| {
        cx.add_window(|_, _| Scrolled {
            scroll: ScrollHandle::new(),
        })
    };
    let (layered, direct) = (new(&mut cx), new(&mut cx));
    for (window, layers) in [(layered, true), (direct, false)] {
        with_window(&mut cx, window.into(), |window, cx| {
            window.set_scroll_layers(layers);
            window.draw(cx).clear(cx);
        });
    }
    for frame in 0..30 {
        let dy = if frame < 20 { -13. } else { 17. };
        let expected = wheel_and_draw(&mut cx, direct.into(), dy);
        let actual = wheel_and_draw(&mut cx, layered.into(), dy);
        assert_eq!(actual, expected, "frame {frame}");
        assert!(
            expected.iter().any(|line| line.contains("layer mask")),
            "frame {frame} masks"
        );
    }
    let stats = with_window(&mut cx, layered.into(), |window, _| window.layout_stats());
    assert_eq!(stats.layer_frames_composited, 0, "{stats:?}");
}

/// monocode's modes and Haze's blurs, by its `background_mask_marker`.
#[test]
fn a_masks_mode_and_blurs_are_monocodes() {
    let mode =
        |empty, haze, preview| LayerMask::project_background(empty, haze, preview, 1.).mode();
    assert_eq!(mode(true, false, false), 1);
    assert_eq!(mode(true, false, true), 1);
    assert_eq!(mode(false, false, true), 2);
    assert_eq!(mode(true, true, false), 3);
    assert_eq!(mode(false, true, false), 4);
    assert_eq!(mode(false, true, true), 5);
    let sigmas =
        |empty, preview| LayerMask::project_background(empty, true, preview, 1.).haze_sigmas();
    assert_eq!(sigmas(true, false), Some((7., 18.)));
    assert_eq!(sigmas(false, false), Some((5., 12.)));
    assert_eq!(sigmas(false, true), Some((2., 5.)));
    assert_eq!(
        LayerMask::project_background(false, false, false, 2.).opacity,
        1.
    );
    assert_eq!(
        LayerMask::project_background(false, false, false, 0.5).haze_sigmas(),
        None
    );
}

/// What a finished scene's composed batches draw, in order: each quad by
/// the paint step its top left radius was set to, and each hole, blur and
/// mask cut, with its draw order.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Met {
    Quad(usize, u32),
    Hole(u32),
    /// A blur's order and how many quads of it are drawn before it.
    Blur(u32, u32),
    Mask(LayerMaskCut),
}

impl Met {
    fn order(&self) -> u32 {
        match self {
            Met::Quad(_, order) | Met::Hole(order) | Met::Blur(order, _) => *order,
            Met::Mask(cut) => cut.order,
        }
    }

    /// How many quads of its order a blur or a mask's cut is drawn after.
    fn quads_before(&self) -> Option<u32> {
        match self {
            Met::Blur(_, before) => Some(*before),
            Met::Mask(cut) => Some(cut.quads_before),
            Met::Quad(..) | Met::Hole(_) => None,
        }
    }
}

fn met(scene: &Scene) -> Vec<Met> {
    let mut met = Vec::new();
    for batch in scene.composed_batches() {
        match batch {
            ComposedBatch::Primitives(crate::PrimitiveBatch::Quads(range)) => met.extend(
                scene.quads[range]
                    .iter()
                    .map(|quad| Met::Quad(quad.corner_radii.top_left.0 as usize, quad.order)),
            ),
            ComposedBatch::Primitives(other) => panic!("unexpected batch {other:?}"),
            ComposedBatch::Holes(range) => met.extend(
                scene.natives().holes[range]
                    .iter()
                    .map(|hole| Met::Hole(hole.order)),
            ),
            ComposedBatch::Blurs(range) => met.extend(
                scene.backdrop_blurs()[range]
                    .iter()
                    .map(|blur| Met::Blur(blur.order, blur.quads_before)),
            ),
            ComposedBatch::Masks(range) => {
                met.extend(scene.layer_masks()[range].iter().copied().map(Met::Mask))
            }
        }
    }
    met
}

/// Seeded random scenes of quads, natives, blurs and nested masks: the
/// composed batches meet everything once, in draw order, a cut before what
/// shares its order; and every quad over a mask's region is met before its
/// begin when painted before it, between its cuts when painted in its
/// group, and after its end when painted after it.
#[test]
fn every_quad_over_a_masks_region_is_met_on_its_side_of_the_cuts() {
    use crate::{BackdropBlur, composition::NativePlacement};
    use rand::{Rng as _, SeedableRng as _, rngs::StdRng};

    // Larger than any box, so that nothing is clipped.
    let everywhere = ContentMask {
        bounds: sp(0., 0., 500., 500.),
    };
    for seed in 0..60 {
        let mut rng = StdRng::seed_from_u64(seed);
        let random_box = |rng: &mut StdRng| {
            sp(
                rng.random_range(0.0..300f32).floor(),
                rng.random_range(0.0..300f32).floor(),
                rng.random_range(4.0..120f32).floor(),
                rng.random_range(4.0..120f32).floor(),
            )
        };
        let mut scene = Scene::default();
        // Each quad painted: its step and bounds.
        let mut quads: Vec<(usize, Bounds<ScaledPixels>)> = Vec::new();
        // Each mask: its region, and the steps it began and ended at.
        let mut masks: Vec<(Bounds<ScaledPixels>, usize, Option<usize>)> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        for step in 0..40 {
            match rng.random_range(0..10) {
                0..5 => {
                    let bounds = random_box(&mut rng);
                    scene.insert_primitive(crate::Quad {
                        bounds,
                        content_mask: everywhere,
                        corner_radii: Corners {
                            top_left: ScaledPixels(step as f32),
                            ..Corners::default()
                        },
                        ..Default::default()
                    });
                    quads.push((step, bounds));
                }
                5 => scene.insert_native(NativePlacement {
                    order: 0,
                    id: crate::composition::NativeId(step as u64),
                    hitbox: None,
                    focus: None,
                    bounds: random_box(&mut rng),
                    content_mask: everywhere,
                    corner_radii: Corners::default(),
                    opacity: 1.,
                }),
                6 => scene.insert_backdrop_blur(BackdropBlur {
                    order: 0,
                    fade: 0,
                    bounds: random_box(&mut rng),
                    content_mask: everywhere,
                    corner_radii: Corners::default(),
                    blur_radius: ScaledPixels(4.),
                    opacity: 1.,
                    quads_before: 0,
                }),
                7 | 8 if open.len() < 3 => {
                    let bounds = random_box(&mut rng);
                    scene.insert_layer_mask(cut(true, bounds));
                    open.push(masks.len());
                    masks.push((bounds, step, None));
                }
                _ => {
                    if let Some(mask) = open.pop() {
                        scene.insert_layer_mask(cut(false, masks[mask].0));
                        masks[mask].2 = Some(step);
                    }
                }
            }
        }
        while let Some(mask) = open.pop() {
            scene.insert_layer_mask(cut(false, masks[mask].0));
            masks[mask].2 = Some(usize::MAX);
        }
        scene.finish();

        let met = met(&scene);
        let count = |f: fn(&Met) -> bool| met.iter().filter(|m| f(m)).count();
        assert_eq!(
            count(|m| matches!(m, Met::Quad(..))),
            quads.len(),
            "seed {seed}"
        );
        assert_eq!(
            count(|m| matches!(m, Met::Mask(_))),
            2 * masks.len(),
            "seed {seed}"
        );
        assert_eq!(
            count(|m| matches!(m, Met::Hole(_))),
            scene.natives().holes.len()
        );
        assert_eq!(
            count(|m| matches!(m, Met::Blur(..))),
            scene.backdrop_blurs().len()
        );
        for pair in met.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(a.order() <= b.order(), "seed {seed}: {a:?} then {b:?}");
        }
        // A blur or a mask's cut is drawn as monocode drew its marker quad: after the quads
        // of its order painted before it, and before the rest.
        for (ix, cut) in met.iter().enumerate() {
            if let Some(before) = cut.quads_before() {
                let drawn = met[..ix]
                    .iter()
                    .filter(|m| matches!(m, Met::Quad(_, order) if *order == cut.order()))
                    .count();
                assert_eq!(drawn, before as usize, "seed {seed}: {cut:?}");
            }
        }
        let at = |want: &dyn Fn(&Met) -> bool| met.iter().position(want).unwrap();
        for (id, (region, began, ended)) in masks.iter().enumerate() {
            let ended = ended.unwrap();
            let id = id as u32;
            let begin = at(&|m| matches!(m, Met::Mask(cut) if cut.id == id && cut.begin));
            let end = at(&|m| matches!(m, Met::Mask(cut) if cut.id == id && !cut.begin));
            assert!(begin < end, "seed {seed}, mask {id}");
            for (step, bounds) in &quads {
                if !bounds.intersects(region) {
                    continue;
                }
                let quad = at(&|m| matches!(m, Met::Quad(painted, _) if painted == step));
                let side = if step < began {
                    quad < begin
                } else if *step < ended {
                    begin < quad && quad < end
                } else {
                    quad > end
                };
                assert!(
                    side,
                    "seed {seed}: quad {step} against mask {id} ({began}..{ended})"
                );
            }
        }
    }
}
