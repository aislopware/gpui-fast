//! Edge fades: their ramps lie on device pixels, nest edge by edge, and
//! frames drawn again from last frame, moved or not, through scroll layers
//! or not, fade as frames drawn from scratch do. See
//! [`crate::fast::edge_fade`].

use std::sync::Arc;

use super::element_oracle::GlyphBoxTextSystem;
use crate::{
    AnyWindowHandle, App, Context, EdgeFade, EdgeFadeRamps, Entity, IntoElement, LayoutStats,
    NoopTextSystem, PlatformInput, Render, ScaledPixels, Scene, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, TestAppContext, TouchPhase, Window, WindowHandle, div, edge_fade, hsla,
    point, prelude::*, px,
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

/// Every fade the last frame of `window` drew `kind` primitives with, as
/// ramps, with the bounds of the primitive.
fn faded_quads(window: &Window) -> Vec<(crate::Bounds<ScaledPixels>, EdgeFadeRamps)> {
    let scene: &Scene = &window.rendered_frame.scene;
    scene
        .quads
        .iter()
        .filter_map(|quad| {
            scene
                .edge_fade_ramps(quad.background.pad)
                .map(|ramps| (quad.bounds, ramps))
        })
        .collect()
}

fn faded_glyphs(window: &Window) -> Vec<EdgeFadeRamps> {
    let scene: &Scene = &window.rendered_frame.scene;
    scene
        .monochrome_sprites
        .iter()
        .filter_map(|sprite| scene.edge_fade_ramps(sprite.pad))
        .collect()
}

/// A faded box at a place and of a size that are not whole device pixels.
struct Placed {
    top: f32,
    left: f32,
}

impl Render for Placed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .mt(px(self.top))
                .ml(px(self.left))
                .w(px(100.3))
                .h(px(60.7))
                .child(edge_fade(
                    div().size_full().bg(hsla(0.6, 0.5, 0.5, 1.)).child("faded"),
                    EdgeFade::y(px(10.)),
                )),
        )
    }
}

/// A fade's region is snapped out to whole device pixels, as a clip is, and
/// each ramp runs from nothing at its edge to whole a width in.
#[test]
fn a_fade_s_ramps_lie_on_device_pixels() {
    let mut cx = text_cx();
    let window = cx.add_window(|_, _| Placed {
        top: 3.3,
        left: 7.7,
    });
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let scale = window.scale_factor();
        assert_eq!(scale, 2.);
        let faded = faded_quads(window);
        assert_eq!(faded.len(), 1, "{faded:?}");
        let (bounds, ramps) = faded[0];
        // The box's edges, where layout snapped them, on device pixels.
        assert_eq!(ramps.edge[1], bounds.top().0);
        assert_eq!(ramps.edge[3], bounds.bottom().0);
        assert_eq!(ramps.edge[1].fract(), 0.);
        assert_eq!(ramps.edge[3].fract(), 0.);
        let (top, bottom) = (ramps.edge[1], ramps.edge[3]);
        assert_eq!(ramps.rate[0], 0., "the left edge does not fade");
        assert_eq!(ramps.rate[2], 0., "the right edge does not fade");
        let x = ScaledPixels(bounds.origin.x.0 + 10.);
        let at = |y: f32| ramps.alpha_at(point(x, ScaledPixels(y)));
        // Ten logical pixels are twenty device pixels.
        assert!(at(top + 0.5) < 0.01, "{}", at(top + 0.5));
        assert!((at(top + 10.) - 0.5).abs() < 1e-3, "{}", at(top + 10.));
        assert_eq!(at(top + 20.), 1.);
        assert_eq!(at(bottom - 20.), 1.);
        assert!(at(bottom - 0.5) < 0.01);
        // The glyphs of the label fade alike.
        assert!(faded_glyphs(window).iter().all(|glyph| *glyph == ramps));
        assert!(!faded_glyphs(window).is_empty());
    });
}

/// A list fading at its top and bottom holding a title fading at its right.
struct Nested;

impl Render for Nested {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div().mt(px(20.)).w(px(200.)).h(px(100.)).child(edge_fade(
                div().size_full().flex().flex_col().child(
                    div().w(px(80.)).h(px(20.)).child(edge_fade(
                        div()
                            .size_full()
                            .bg(hsla(0.2, 0.5, 0.5, 1.))
                            .child("a title"),
                        EdgeFade::new(crate::Edges {
                            right: px(16.),
                            top: px(2.),
                            ..Default::default()
                        }),
                    )),
                ),
                EdgeFade::y(px(12.)),
            )),
        )
    }
}

/// Fades nest edge by edge: what is drawn inside both fades at every edge
/// either fades, and where both fade one edge the one reaching further in
/// is drawn.
#[test]
fn nested_fades_meet_edge_by_edge() {
    let mut cx = text_cx();
    let window = cx.add_window(|_, _| Nested);
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let glyphs = faded_glyphs(window);
        assert!(!glyphs.is_empty());
        let title = glyphs[0];
        assert!(glyphs.iter().all(|glyph| *glyph == title));
        // The list's top and bottom: 20 and 120 logical pixels.
        assert_eq!(title.edge[1], 40.);
        assert_eq!(title.edge[3], 240.);
        assert_eq!(title.rate[1], 1. / 24.);
        // The title's right: 80 logical pixels.
        assert_eq!(title.edge[2], 160.);
        assert!(title.rate[2] < 0.);
        assert_eq!(title.rate[0], 0.);
    });
}

/// Rows below a spacer that grows by half a logical pixel a frame, a device
/// pixel at a scale factor of 2: a faded block moving with its fade, and a
/// still fade over a clip whose rows slide under it.
struct Sliding {
    spacer: f32,
    inner: f32,
    depth: f32,
}

fn row(label: String, hue: f32) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .h(px(20.))
        .bg(hsla(hue, 0.5, 0.5, 1.))
        .child(label)
        .child(div().size(px(6.)).bg(hsla(hue, 0.3, 0.3, 1.)))
}

/// A row whose title fades at its ends, inside the row's own fade.
fn faded_row(label: String, hue: f32) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .h(px(20.))
        .bg(hsla(hue, 0.5, 0.5, 1.))
        .child(div().w(px(60.)).h(px(20.)).child(edge_fade(
            div().size_full().child(label),
            EdgeFade::x(px(8.)),
        )))
        .child(div().size(px(6.)).bg(hsla(hue, 0.3, 0.3, 1.)))
}

impl Render for Sliding {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_none().h(px(self.spacer)))
            .child(edge_fade(
                div()
                    .flex()
                    .flex_col()
                    .children((0..3).map(|ix| row(format!("moving {ix}"), ix as f32 / 3.))),
                EdgeFade::y(px(6.)),
            ))
            .child(
                div()
                    .mt(px(10.))
                    .h(px(60.))
                    .overflow_hidden()
                    .child(edge_fade(
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .child(div().flex_none().h(px(self.inner)))
                            .children(
                                (0..5).map(|ix| row(format!("sliding {ix}"), ix as f32 / 5.)),
                            ),
                        EdgeFade::y(px(10.)).depth(crate::Edges::all(self.depth)),
                    )),
            )
    }
}

/// Opens a window drawing incrementally and one drawing from scratch.
fn windows<V: Render>(cx: &mut TestAppContext, new: impl Fn() -> V) -> [WindowHandle<V>; 2] {
    let incremental = cx.add_window(|_, _| new());
    let from_scratch = cx.add_window(|_, _| new());
    let atlas = with_window(cx, incremental.into(), |window, _| {
        window.sprite_atlas.clone()
    });
    with_window(cx, from_scratch.into(), |window, _| {
        window.sprite_atlas = atlas;
    });
    [incremental, from_scratch]
}

/// Changes both windows' views as `change` says, draws them, the second
/// from scratch, requires the frames to match and returns the incremental
/// window's stats.
fn step<V: Render>(
    cx: &mut TestAppContext,
    [incremental, from_scratch]: [WindowHandle<V>; 2],
    change: impl Fn(&mut V),
) -> LayoutStats {
    for window in [incremental, from_scratch] {
        window
            .update(cx, |view, _, cx| {
                change(view);
                cx.notify();
            })
            .unwrap();
    }
    let draw = |cx: &mut TestAppContext, window: WindowHandle<V>, forget: bool| {
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
    let (expected, _) = draw(cx, from_scratch, true);
    let (actual, stats) = draw(cx, incremental, false);
    assert_eq!(actual, expected);
    stats
}

/// Faded rows drawn again moved fade as rows painted afresh where they
/// moved to: a fade around what moved moves with it, and one around a clip
/// that stood still stays.
#[test]
fn faded_rows_drawn_again_moved_fade_as_rows_painted_afresh() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Sliding {
        spacer: 0.,
        inner: 0.,
        depth: 1.,
    });
    // With their fade, then under a fade that stands still.
    let mut moved = [0, 0];
    for frame in 0..24 {
        let stats = step(&mut cx, windows, |sliding| {
            if frame < 12 {
                sliding.spacer += 0.5;
            } else {
                sliding.inner += 0.5;
            }
        });
        moved[frame / 12] += stats.elements_moved;
    }
    assert!(moved[0] > 0 && moved[1] > 0, "{moved:?} drawn again moved");
}

/// A fade that only deepens, its region standing still, draws again what
/// it fades with the new fade.
#[test]
fn a_fade_deepening_over_still_rows_matches() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Sliding {
        spacer: 0.,
        inner: 0.,
        depth: 0.,
    });
    for _ in 0..10 {
        step(&mut cx, windows, |sliding| sliding.depth += 0.1);
    }
}

/// A view holding a nested view inside a fade: the nested view, notified
/// alone, is built again inside the outer one drawn from last frame, in the
/// fade it was drawn in.
struct Outer {
    inner: Entity<Inner>,
}

struct Inner {
    count: usize,
}

impl Render for Outer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div().mt(px(10.)).w(px(160.)).h(px(80.)).child(edge_fade(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child("outer")
                    .child(self.inner.clone()),
                EdgeFade::y(px(20.)),
            )),
        )
    }
}

impl Render for Inner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(40.))
            .bg(hsla(0.4, 0.5, 0.5, 1.))
            .child(format!("inner {}", self.count))
    }
}

#[test]
fn a_view_built_again_inside_a_fade_inherits_it() {
    let mut cx = text_cx();
    let new = |cx: &mut TestAppContext| {
        cx.add_window(|_, cx| Outer {
            inner: cx.new(|_| Inner { count: 0 }),
        })
    };
    let windows = [new(&mut cx), new(&mut cx)];
    let mut reused = 0;
    for _ in 0..6 {
        for window in windows {
            window
                .update(&mut cx, |outer, _, cx| {
                    outer.inner.update(cx, |inner, cx| {
                        inner.count += 1;
                        cx.notify();
                    })
                })
                .unwrap();
        }
        let draw = |cx: &mut TestAppContext, window: WindowHandle<Outer>, forget: bool| {
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
        let (actual, stats) = draw(&mut cx, windows[0], false);
        assert_eq!(actual, expected);
        reused += stats.views_reused;
    }
    assert!(reused > 0, "the outer view was never drawn again");
}

/// A scrolling column of rows inside a fade that deepens with what lies
/// hidden past each edge, over an opaque background, with titles that fade
/// of their own when `own` is set, and an opaque background of its own
/// inside the fade when `backed` is.
struct Scrolled {
    scroll: ScrollHandle,
    own: bool,
    backed: bool,
}

impl Render for Scrolled {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let own = self.own;
        let backed = self.backed;
        div().size_full().bg(hsla(0., 0., 1., 1.)).child(
            div().w(px(300.)).h(px(200.)).child(
                edge_fade(
                    div()
                        .id("scrolled")
                        .size_full()
                        .flex()
                        .flex_col()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .when(backed, |this| this.bg(hsla(0.6, 0.2, 0.95, 1.)))
                        .children((0..60).map(move |ix| {
                            let label = format!("row {ix}");
                            if own {
                                faded_row(label, ix as f32 / 60.).into_any_element()
                            } else {
                                div()
                                    .h(px(20.))
                                    .bg(hsla(ix as f32 / 60., 0.5, 0.5, 1.))
                                    .child(label)
                                    .into_any_element()
                            }
                        })),
                    EdgeFade::y(px(24.)),
                )
                .hidden_by_scroll(&self.scroll),
            ),
        )
    }
}

/// Scrolls `window` by `dy` with the wheel and draws the frame that
/// follows, returning what it painted and its stats.
fn wheel_and_draw(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    dy: f32,
) -> (Vec<String>, LayoutStats) {
    with_window(cx, window, |window, cx| {
        window.dispatch_event(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: point(px(20.), px(20.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                modifiers: Default::default(),
                touch_phase: TouchPhase::Moved,
                momentum_phase: None,
            }),
            cx,
        );
        window.draw(cx).clear(cx);
        (window.painted_primitives(), window.layout_stats())
    })
}

/// Scrolls a window with scroll layers and one without the same way,
/// requiring every frame to draw the same thing, the layer's tiles fading
/// as the content would, and returns the layered window's stats.
fn scroll_both(own: bool, backed: bool) -> LayoutStats {
    let mut cx = text_cx();
    let new = |cx: &mut TestAppContext| {
        cx.add_window(|_, _| Scrolled {
            scroll: ScrollHandle::new(),
            own,
            backed,
        })
    };
    let (layered, direct) = (new(&mut cx), new(&mut cx));
    for (window, layers) in [(layered, true), (direct, false)] {
        with_window(&mut cx, window.into(), |window, cx| {
            window.set_scroll_layers(layers);
            window.draw(cx).clear(cx);
        });
    }
    let mut stats = LayoutStats::default();
    let mut fades = Vec::new();
    for frame in 0..30 {
        let dy = if frame < 20 { -13. } else { 17. };
        let (expected, _) = wheel_and_draw(&mut cx, direct.into(), dy);
        let (actual, layered_stats) = wheel_and_draw(&mut cx, layered.into(), dy);
        assert_eq!(actual, expected, "frame {frame}");
        stats = layered_stats;
        fades.push(expected.iter().any(|line| line.contains(" fade ")));
    }
    assert!(fades.iter().all(|faded| *faded));
    stats
}

/// A scroll container's fade is laid over its layer's tiles, which move
/// under it: what is composited fades as content drawn without the layer.
#[test]
fn a_scroll_layer_s_tiles_fade_as_its_content() {
    let stats = scroll_both(false, false);
    if crate::fast::layers::COMPILED {
        assert!(stats.layer_frames_composited > 0, "{stats:?}");
    }
}

/// Content setting fades of its own is drawn without its layer's tiles,
/// since those fades would move with the tiles.
#[test]
fn content_fading_of_its_own_is_drawn_without_tiles() {
    let stats = scroll_both(true, false);
    assert_eq!(stats.layer_frames_composited, 0, "{stats:?}");
}

/// A background painted inside the fade fades too, so a layer can't clear
/// its tiles with it: the frame is drawn without the layer, as without
/// one, though the background is not yet given its fade as the layer
/// looks under its viewport.
#[test]
fn a_faded_background_is_not_baked_into_tiles() {
    let stats = scroll_both(false, true);
    assert_eq!(stats.layer_frames_composited, 0, "{stats:?}");
}

/// A fade region sliding by whole device pixels gives a new fade every
/// frame: the table keeps only what is drawn, and frames keep matching
/// across its sweeps.
#[test]
fn the_table_of_fades_stays_small_while_fades_keep_changing() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Sliding {
        spacer: 0.,
        inner: 0.,
        depth: 0.5,
    });
    for frame in 0..900 {
        step(&mut cx, windows, |sliding| {
            sliding.spacer = (frame % 300) as f32 * 0.5;
            sliding.depth = 0.3 + (frame % 7) as f32 * 0.1;
        });
    }
    let (slots, sweeps) = with_window(&mut cx, windows[0].into(), |window, _| {
        (
            window.rendered_frame.scene.edge_fades().len(),
            window.fast_edge_fade.sweeps(),
        )
    });
    assert!(sweeps > 0, "the table was never swept");
    assert!(slots < 600, "{slots} slots");
}

/// A keyed stretch painting a box inside a fade of its own, relative to its
/// origin, under a still fade: moved, its own fade moves with it and the
/// one around it stays.
struct KeyedInFade {
    top: f32,
}

impl Render for KeyedInFade {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let top = self.top;
        div().size_full().child(
            div().mt(px(20.)).w(px(200.)).h(px(120.)).child(edge_fade(
                crate::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let origin = point(bounds.origin.x + px(10.), bounds.origin.y + px(top));
                        window.paint_keyed(7, origin, |window| {
                            let stretch = crate::Bounds::new(origin, crate::size(px(80.), px(30.)));
                            window.with_edge_fade(stretch, EdgeFade::x(px(12.)), |window| {
                                window.paint_quad(crate::fill(stretch, hsla(0.7, 0.5, 0.5, 1.)));
                            });
                        });
                    },
                )
                .size_full(),
                EdgeFade::y(px(16.)),
            )),
        )
    }
}

#[test]
fn a_keyed_stretch_moved_keeps_its_own_fade_and_takes_the_one_around() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || KeyedInFade { top: 0. });
    let mut moved = 0;
    for _ in 0..20 {
        moved += step(&mut cx, windows, |keyed| keyed.top += 0.5).paints_moved;
    }
    assert!(moved > 0, "the stretch was never drawn again moved");
}
