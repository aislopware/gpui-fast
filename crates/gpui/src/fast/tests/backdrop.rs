//! Backdrop blurs (`Window::paint_backdrop_blur`): their place among the
//! primitives they cover and those painted over them, and frames drawn
//! again from last frame, moved or not, through scroll layers or not,
//! placing them as frames drawn from scratch do. See
//! [`crate::fast::backdrop`].

use std::sync::Arc;

use super::element_oracle::GlyphBoxTextSystem;
use crate::fast::layers::policy::{Decision, last_decision};
use crate::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, Corners, EdgeFade, Element, ElementId,
    Entity, GlobalElementId, InspectorElementId, IntoElement, LayoutId, LayoutStats, ListAlignment,
    ListState, NoopTextSystem, Pixels, PlatformInput, Render, ScaledPixels, ScrollDelta,
    ScrollHandle, ScrollWheelEvent, TestAppContext, TouchPhase, Window, WindowHandle, canvas,
    composition::ComposedBatch, div, edge_fade, hsla, point, prelude::*, px,
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

/// A frosted surface: what is under it blurred by `radius`, then its
/// translucent fill and `child` over that.
fn frosted(radius: f32, child: impl IntoElement) -> AnyElement {
    div()
        .relative()
        .size_full()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    window.paint_backdrop_blur(bounds, Corners::all(px(8.)), px(radius))
                },
            )
            .absolute()
            .size_full(),
        )
        .child(
            div()
                .size_full()
                .rounded(px(8.))
                .bg(hsla(0., 0., 1., 0.4))
                .child(child),
        )
        .into_any_element()
}

/// A striped page and, over its top left, a frosted popover at a place that
/// is not whole device pixels, half opaque.
struct Popover {
    radius: f32,
}

impl Render for Popover {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .children((0..10).map(|ix| div().h(px(20.)).bg(hsla(ix as f32 / 10., 0.5, 0.5, 1.))))
            .child(
                div()
                    .absolute()
                    .top(px(10.3))
                    .left(px(20.7))
                    .w(px(100.4))
                    .h(px(60.2))
                    .opacity(0.5)
                    .child(frosted(self.radius, "menu")),
            )
    }
}

/// A blur takes a draw order over the stripes it covers and under its fill
/// and text, lies on device pixels, takes the element opacity, and is met
/// by a renderer between the batches drawn under it and over it; one of no
/// radius places nothing.
#[test]
fn a_blur_lies_between_what_it_covers_and_what_is_painted_over_it() {
    let mut cx = text_cx();
    let window = cx.add_window(|_, _| Popover { radius: 12. });
    cx.test_window(window.into())
        .simulate_scale_factor_change(2.);
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let scene = &window.rendered_frame.scene;
        let blurs = scene.backdrop_blurs();
        assert_eq!(blurs.len(), 1);
        let blur = blurs[0];
        assert_eq!(blur.blur_radius, ScaledPixels(24.));
        assert_eq!(blur.corner_radii, Corners::all(ScaledPixels(16.)));
        assert_eq!(blur.opacity, 0.5);
        let bounds = blur.bounds;
        for side in [
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        ] {
            assert_eq!(side.0.fract(), 0., "{bounds:?} on device pixels");
        }
        let overlapping = |quad: &&crate::Quad| quad.bounds.intersects(&blur.bounds);
        let stripes: Vec<_> = scene
            .quads
            .iter()
            .filter(overlapping)
            .filter(|quad| quad.bounds.size.height == ScaledPixels(40.))
            .collect();
        assert!(stripes.len() >= 3, "the blur covers stripes");
        assert!(stripes.iter().all(|quad| quad.order < blur.order));
        let fill = scene
            .quads
            .iter()
            .filter(overlapping)
            .find(|quad| quad.bounds.size.height != ScaledPixels(40.))
            .expect("the popover's fill");
        assert!(fill.order > blur.order, "the fill is drawn over the blur");
        assert!(
            scene
                .monochrome_sprites
                .iter()
                .all(|sprite| sprite.order > blur.order),
            "the text is drawn over the blur"
        );
        // A renderer meets the blur after the stripes, before the fill.
        let batches: Vec<_> = scene.composed_batches().collect();
        let at = batches
            .iter()
            .position(|batch| matches!(batch, ComposedBatch::Blur(0)))
            .expect("the blur is met");
        let quads_before: Vec<u32> = batches[..at]
            .iter()
            .filter_map(|batch| match batch {
                ComposedBatch::Primitives(crate::PrimitiveBatch::Quads(range)) => Some(range),
                _ => None,
            })
            .flat_map(|range| scene.quads[range.clone()].iter().map(|quad| quad.order))
            .collect();
        assert!(quads_before.iter().all(|order| *order < blur.order));
        assert!(quads_before.len() >= stripes.len());
        assert!(!quads_before.contains(&fill.order));
    });
    window
        .update(&mut cx, |popover, _, cx| {
            popover.radius = 0.;
            cx.notify();
        })
        .unwrap();
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let scene = &window.rendered_frame.scene;
        assert!(scene.backdrop_blurs().is_empty());
        assert!(
            !scene
                .composed_batches()
                .any(|batch| matches!(batch, ComposedBatch::Blur(_)))
        );
    });
}

/// A frosted card moving down the window, holding a fade and a frosted
/// nested view notified alone.
struct Sliding {
    spacer: f32,
    nested: Entity<Nested>,
}

struct Nested {
    count: usize,
}

impl Render for Nested {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(30.))
            .w(px(120.))
            .child(frosted(4., format!("nested {}", self.count)))
    }
}

impl Render for Sliding {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = (0..6).map(|ix| {
            div()
                .h(px(20.))
                .bg(hsla(ix as f32 / 6., 0.5, 0.5, 1.))
                .child(format!("row {ix}"))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(0., 0., 0.9, 1.))
            .child(div().flex_none().h(px(self.spacer)))
            .child(
                div().w(px(160.)).h(px(120.)).child(frosted(
                    10.,
                    div()
                        .flex()
                        .flex_col()
                        .child(self.nested.clone())
                        .child(edge_fade(
                            div().flex().flex_col().children(rows),
                            EdgeFade::y(px(8.)),
                        )),
                )),
            )
    }
}

/// A frosted card moving, with a frosted nested view notified alone, drawn
/// incrementally, places its blurs as a window drawn from scratch does.
#[test]
fn a_blur_drawn_again_lies_as_drawn_afresh() {
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
                .filter(|line| line.contains("backdrop blur"))
                .count(),
            2,
            "frame {frame}"
        );
        stats.views_reused += frame_stats.views_reused;
    }
    assert!(stats.views_reused > 0, "{stats:?}");
}

/// A scrolling column over an opaque background, frosted rows inside it
/// when `inside`, or a frosted popover over it when not.
struct Scrolled {
    scroll: ScrollHandle,
    inside: bool,
}

impl Render for Scrolled {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let inside = self.inside;
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
                if inside && ix % 7 == 3 {
                    row.child(
                        div()
                            .absolute()
                            .w(px(80.))
                            .h(px(20.))
                            .child(frosted(6., "")),
                    )
                    .relative()
                } else {
                    row
                }
            }));
        div()
            .relative()
            .size_full()
            .bg(hsla(0., 0., 1., 1.))
            .child(div().w(px(300.)).h(px(200.)).child(content))
            .when(!inside, |page| {
                page.child(
                    div()
                        .absolute()
                        .top(px(40.))
                        .left(px(60.))
                        .w(px(160.))
                        .h(px(100.))
                        .child(frosted(20., "popover")),
                )
            })
    }
}

fn wheel_and_draw(cx: &mut TestAppContext, window: AnyWindowHandle, dy: f32) -> Vec<String> {
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
        window.painted_primitives()
    })
}

/// Scrolls a window with scroll layers and one without the same way,
/// requiring each frame to draw the same, blurs in their places, and
/// returns the layered window's stats.
fn scroll_both(inside: bool) -> LayoutStats {
    let mut cx = text_cx();
    let new = |cx: &mut TestAppContext| {
        cx.add_window(|_, _| Scrolled {
            scroll: ScrollHandle::new(),
            inside,
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
    for frame in 0..30 {
        let dy = if frame < 20 { -13. } else { 17. };
        let expected = wheel_and_draw(&mut cx, direct.into(), dy);
        let actual = wheel_and_draw(&mut cx, layered.into(), dy);
        assert_eq!(actual, expected, "frame {frame}");
        assert!(
            expected.iter().any(|line| line.contains("backdrop blur")),
            "frame {frame} blurs"
        );
        stats = with_window(&mut cx, layered.into(), |window, _| window.layout_stats());
    }
    stats
}

/// A frosted popover over a scroll container blurs the layer's tiles as
/// they are composited under it: the layer keeps compositing.
#[test]
fn a_blur_over_a_scroll_layer_leaves_it_compositing() {
    let stats = scroll_both(false);
    if crate::fast::layers::COMPILED {
        assert!(stats.layer_frames_composited > 0, "{stats:?}");
    }
}

/// Content blurring what is under it is drawn without its layer's tiles,
/// which hold it over the layer's background alone.
#[test]
fn content_that_blurs_is_drawn_without_tiles() {
    let stats = scroll_both(true);
    assert_eq!(stats.layer_frames_composited, 0, "{stats:?}");
}

/// A list of frosted rows, or a uniform list of them.
struct FrostedRows {
    uniform: bool,
    state: ListState,
}

fn frosted_row(ix: usize) -> AnyElement {
    div()
        .w(px(200.))
        .h(px(40.))
        .bg(hsla((ix % 17) as f32 / 17., 0.5, 0.5, 1.))
        .child(div().w(px(120.)).h(px(30.)).child(frosted(5., "")))
        .into_any_element()
}

impl Render for FrostedRows {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = div().size_full().bg(hsla(0., 0., 1., 1.));
        if self.uniform {
            page.child(
                crate::uniform_list(
                    "list",
                    300,
                    cx.processor(|_, range: std::ops::Range<usize>, _, _| {
                        range.map(frosted_row).collect::<Vec<_>>()
                    }),
                )
                .w(px(200.))
                .h(px(100.)),
            )
        } else {
            page.child(
                crate::list(self.state.clone(), |ix, _, _| frosted_row(ix))
                    .w(px(200.))
                    .h(px(100.)),
            )
        }
    }
}

/// A list whose rows blur what is under them keeps its layer: the rows are
/// replayed, blurs and all, each over what the rows before it drew, as
/// drawn without layers.
#[test]
fn a_list_of_frosted_rows_replays_their_blurs() {
    let mut cx = text_cx();
    for uniform in [false, true] {
        let new = |cx: &mut TestAppContext, layers: bool| {
            let window = cx.add_window(move |_, _| FrostedRows {
                uniform,
                state: ListState::new(300, ListAlignment::Top, px(0.)).measure_all(),
            });
            with_window(cx, window.into(), |window, cx| {
                window.set_scroll_layers(layers);
                window.draw(cx).clear(cx);
            });
            AnyWindowHandle::from(window)
        };
        let (layered, direct) = (new(&mut cx, true), new(&mut cx, false));
        let mut composited = 0;
        for (frame, dy) in [-20., -20., -15., -15., -15., -15., 25., -9., -9.]
            .into_iter()
            .enumerate()
        {
            let expected = wheel_and_draw(&mut cx, direct, dy);
            let actual = wheel_and_draw(&mut cx, layered, dy);
            assert_eq!(actual, expected, "uniform {uniform}, frame {frame}");
            let blurs = expected
                .iter()
                .filter(|line| line.contains("backdrop blur"))
                .count();
            assert!(
                blurs >= 3,
                "uniform {uniform}, frame {frame}: {blurs} blurs"
            );
            if crate::fast::layers::COMPILED {
                let decision = with_window(&mut cx, layered, |window, _| {
                    let id = window.fast_layers.layers.keys().next().cloned();
                    id.and_then(|id| last_decision(window, &id))
                });
                if decision == Some(Decision::Composite) {
                    composited += 1;
                }
            }
        }
        if crate::fast::layers::COMPILED {
            assert!(composited > 0, "uniform {uniform}: the list composited");
        }
    }
}

/// `blur_radius` in device pixels is what [`Window::paint_backdrop_blur`]
/// takes in logical pixels, scaled.
#[test]
fn a_blurs_radius_is_scaled_to_device_pixels() {
    let mut cx = text_cx();
    let window = cx.add_window(|_, _| Popover { radius: 3.5 });
    cx.test_window(window.into())
        .simulate_scale_factor_change(1.5);
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let blur = window.rendered_frame.scene.backdrop_blurs()[0];
        assert_eq!(blur.blur_radius, ScaledPixels(5.25));
        assert_eq!(
            blur.reach().origin.x,
            blur.bounds.origin.x - ScaledPixels(17.)
        );
    });
}

/// Its child painted in one paint layer over a blur of what is under it,
/// as zeron's `frost::frosted` paints a frosted card.
struct InLayer {
    child: AnyElement,
}

impl IntoElement for InLayer {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for InLayer {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.paint_layer(bounds, |window| {
            window.paint_backdrop_blur(bounds, Corners::all(px(12.)), px(16.));
            self.child.paint(window, cx);
        });
    }
}

struct LayerCard;

impl Render for LayerCard {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(hsla(0.3, 0.5, 0.5, 1.))
            .child(div().h(px(40.)).bg(hsla(0.6, 0.5, 0.5, 1.)))
            .child(
                div().w(px(140.)).h(px(80.)).child(InLayer {
                    child: div()
                        .size_full()
                        .rounded(px(12.))
                        .bg(hsla(0., 0., 1., 0.15))
                        .child("card")
                        .into_any_element(),
                }),
            )
    }
}

/// A blur painted first in a paint layer, whose primitives share one draw
/// order, still lies under the rest of the layer: the fill and the text
/// painted after it in the layer are drawn over it.
#[test]
fn a_blur_painted_first_in_a_paint_layer_lies_under_the_rest_of_it() {
    let mut cx = text_cx();
    let window = cx.add_window(|_, _| LayerCard);
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let scene = &window.rendered_frame.scene;
        let blur = scene.backdrop_blurs()[0];
        let card = |quad: &&crate::Quad| quad.bounds == blur.bounds;
        let fill = scene.quads.iter().find(card).expect("the card's fill");
        assert!(
            fill.order > blur.order,
            "{} over {}",
            fill.order,
            blur.order
        );
        let under: Vec<_> = scene
            .quads
            .iter()
            .filter(|quad| !card(quad) && quad.bounds.intersects(&blur.reach()))
            .collect();
        assert_eq!(under.len(), 2, "the page and the band are under it");
        assert!(under.iter().all(|quad| quad.order < blur.order));
        assert!(!scene.monochrome_sprites.is_empty());
        assert!(
            scene
                .monochrome_sprites
                .iter()
                .all(|sprite| sprite.order > blur.order)
        );
    });
}
