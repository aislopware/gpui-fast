//! Rounded clips (`Window::with_rounded_clip`, carried in the table of edge
//! fades): they lie on device pixels, cut every primitive, nest with fades,
//! and frames drawn again from last frame, moved or not, through scroll
//! layers or not, clip as frames drawn from scratch do. See
//! [`crate::fast::edge_fade`].

use std::sync::Arc;

use super::element_oracle::GlyphBoxTextSystem;
use crate::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, EdgeFade, EdgeFadeRamps, Element, ElementId,
    Entity, GlobalElementId, InspectorElementId, IntoElement, LayoutId, LayoutStats,
    NoopTextSystem, Pixels, PlatformInput, Render, ScaledPixels, Scene, ScrollDelta, ScrollHandle,
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

/// Its child, clipped to its bounds with corners of `radius`, in its
/// prepaint and its paint.
struct Clipped {
    child: AnyElement,
    radius: Pixels,
}

fn clipped(radius: f32, child: impl IntoElement) -> Clipped {
    Clipped {
        child: child.into_any_element(),
        radius: px(radius),
    }
}

impl IntoElement for Clipped {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Clipped {
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
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rounded_clip(bounds, self.radius, |window| {
            self.child.prepaint(window, cx)
        });
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
        window.with_rounded_clip(bounds, self.radius, |window| self.child.paint(window, cx));
    }
}

/// The fades the quads of `scene` are drawn in, as ramps, with each quad's
/// bounds and content mask.
fn clipped_quads(
    scene: &Scene,
) -> Vec<(Bounds<ScaledPixels>, Bounds<ScaledPixels>, EdgeFadeRamps)> {
    scene
        .quads
        .iter()
        .filter_map(|quad| {
            scene
                .edge_fade_ramps(quad.background.pad)
                .map(|ramps| (quad.bounds, quad.content_mask.bounds, ramps))
        })
        .collect()
}

/// A card at a place and of a size that are not whole device pixels.
struct Card {
    radius: f32,
}

impl Render for Card {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .mt(px(20.7))
                .ml(px(10.3))
                .w(px(100.4))
                .h(px(60.2))
                .child(clipped(
                    self.radius,
                    div()
                        .size_full()
                        .bg(hsla(0.6, 0.5, 0.5, 1.))
                        .child("clipped"),
                )),
        )
    }
}

/// A clip's rectangle is snapped out to whole device pixels, as a fade's
/// region is, and the content mask narrows to it; a clip of no radius is
/// the content mask alone.
#[test]
fn a_rounded_clip_lies_on_device_pixels() {
    let mut cx = text_cx();
    let window = cx.add_window(|_, _| Card { radius: 9. });
    cx.test_window(window.into())
        .simulate_scale_factor_change(2.);
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let scene = &window.rendered_frame.scene;
        let quads = clipped_quads(scene);
        assert_eq!(quads.len(), 1, "the card's fill is clipped");
        let (_, mask, ramps) = quads[0];
        assert!(ramps.clips() && ramps.rate.iter().all(|rate| *rate == 0.));
        // The card as laid out, at whole device pixels.
        assert_eq!(ramps.clip, [21., 41., 222., 161.]);
        assert!(ramps.clip.iter().all(|side| side.fract() == 0.));
        assert_eq!(ramps.radii, [18.; 4]);
        assert!(mask.size.width.0 <= 222. - 21. && mask.size.height.0 <= 161. - 41.);
        let glyphs = scene
            .monochrome_sprites
            .iter()
            .filter(|sprite| scene.edge_fade_ramps(sprite.pad) == Some(ramps))
            .count();
        assert!(glyphs > 0, "the text is clipped too");
        // Outside every corner, inside the body.
        let at = |x: f32, y: f32| ramps.coverage_at(point(ScaledPixels(x), ScaledPixels(y)));
        assert_eq!(at(21.5, 41.5), 0.);
        assert_eq!(at(221.5, 160.5), 0.);
        assert_eq!(at(120., 100.), 1.);
        assert_eq!(at(21.5, 100.), 1.);
    });
    window
        .update(&mut cx, |card, _, cx| {
            card.radius = 0.;
            cx.notify();
        })
        .unwrap();
    with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        assert!(clipped_quads(&window.rendered_frame.scene).is_empty());
    });
}

/// A rounded card moving down the window, holding a fade, a nested view
/// notified alone, and rows sliding inside it.
struct Sliding {
    spacer: f32,
    inner: f32,
    nested: Entity<Nested>,
}

struct Nested {
    count: usize,
}

impl Render for Nested {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(24.))
            .bg(hsla(0.4, 0.5, 0.5, 1.))
            .child(format!("nested {}", self.count))
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
            .child(div().flex_none().h(px(self.spacer)))
            .child(
                div().w(px(160.)).h(px(90.)).child(clipped(
                    12.,
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(div().flex_none().h(px(self.inner)))
                        .child(self.nested.clone())
                        .child(edge_fade(
                            div().flex().flex_col().children(rows),
                            EdgeFade::y(px(8.)),
                        )),
                )),
            )
    }
}

/// A card moving, the rows inside it sliding and a nested view notified
/// alone, drawn incrementally, clip as a window drawn from scratch: a clip
/// around what moved moves with it, one around what slid stays, and fades
/// keep nesting inside it.
#[test]
fn a_rounded_clip_drawn_again_clips_as_drawn_afresh() {
    let mut cx = text_cx();
    let new = |cx: &mut TestAppContext| {
        cx.add_window(|_, cx| Sliding {
            spacer: 0.,
            inner: 0.,
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
                    match frame % 3 {
                        0 => sliding.spacer += 0.5,
                        1 => sliding.inner += 1.,
                        _ => {}
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
        assert!(
            actual.iter().any(|line| line.contains(" fade ")),
            "frame {frame} clips"
        );
        stats.views_reused += frame_stats.views_reused;
        stats.elements_moved += frame_stats.elements_moved;
    }
    assert!(stats.views_reused > 0, "{stats:?}");
}

/// A scrolling column inside a rounded card over an opaque background:
/// the card's clip around the scroll container when `inside` is false, or
/// a clip of each row's own when it is.
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
                if inside {
                    div().h(px(20.)).child(clipped(6., row)).into_any_element()
                } else {
                    row.into_any_element()
                }
            }));
        let content: AnyElement = if inside {
            content.into_any_element()
        } else {
            clipped(16., content).into_any_element()
        };
        div()
            .size_full()
            .bg(hsla(0., 0., 1., 1.))
            .child(div().w(px(300.)).h(px(200.)).child(content))
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
/// requiring each frame to draw the same, and returns the layered
/// window's stats.
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
        assert!(expected.iter().any(|line| line.contains(" fade ")));
        stats = with_window(&mut cx, layered.into(), |window, _| window.layout_stats());
    }
    stats
}

/// A clip around a scroll container is laid over its layer's tiles, which
/// move under it, as a fade around one is.
#[test]
fn a_scroll_layer_inside_a_rounded_clip_composites() {
    let stats = scroll_both(false);
    if crate::fast::layers::COMPILED {
        assert!(stats.layer_frames_composited > 0, "{stats:?}");
    }
}

/// Content clipping of its own composites its layer's tiles: its clips
/// move with the tiles, inside the layer's own around them.
#[test]
fn content_clipping_of_its_own_composites_its_tiles() {
    let stats = scroll_both(true);
    if crate::fast::layers::COMPILED {
        assert!(stats.layer_frames_composited > 0, "{stats:?}");
    }
}

/// A card clipping its overflow with rounded corners, a view inside it.
struct OverflowCard {
    hidden: bool,
    radius: f32,
    inner: Entity<OverflowCardInner>,
}

struct OverflowCardInner;

impl Render for OverflowCardInner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(hsla(0.3, 0.5, 0.5, 1.))
            .child(div().w(px(80.)).h(px(30.)).bg(hsla(0.6, 0.5, 0.5, 1.)))
    }
}

impl Render for OverflowCard {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let card = div()
            .w(px(200.))
            .h(px(120.))
            .rounded(px(self.radius))
            .overflow_hidden()
            .bg(hsla(0., 0., 0.9, 1.))
            .child(self.inner.clone());
        if self.hidden { card.invisible() } else { card }
    }
}

/// A rounded card's children, drawn again from last frame or not, after it
/// was hidden or its corners changed, draw as a window drawing it afresh.
#[test]
fn a_rounded_overflow_drawn_again_draws_as_afresh() {
    let mut cx = text_cx();
    let new = |cx: &mut TestAppContext, hidden: bool, radius: f32| {
        cx.add_window(move |_, cx| OverflowCard {
            hidden,
            radius,
            inner: cx.new(|_| OverflowCardInner),
        })
    };
    let kept = new(&mut cx, false, 12.);
    with_window(&mut cx, kept.into(), |window, cx| window.draw(cx).clear(cx));
    for (hidden, radius) in [(true, 12.), (false, 12.), (false, 0.), (false, 20.)] {
        kept.update(&mut cx, |card, _, cx| {
            card.hidden = hidden;
            card.radius = radius;
            cx.notify();
        })
        .unwrap();
        let actual = with_window(&mut cx, kept.into(), |window, cx| {
            window.draw(cx).clear(cx);
            window.painted_primitives()
        });
        let fresh = new(&mut cx, hidden, radius);
        let expected = with_window(&mut cx, fresh.into(), |window, cx| {
            window.draw(cx).clear(cx);
            window.painted_primitives()
        });
        assert_eq!(actual, expected, "hidden {hidden}, radius {radius}");
    }
}

/// A quad painted in a rounded clip equals the background it was painted
/// with, as `Window::painted_quads` gives it: the clip its background's
/// padding holds is the scene's to draw with, not the quad's colour.
#[test]
fn a_quad_painted_in_a_rounded_clip_keeps_its_background() {
    let mut cx = text_cx();
    let window = cx.add_window(|_, _| Card { radius: 9. });
    let background: crate::Background = hsla(0.6, 0.5, 0.5, 1.).into();
    let (in_clip, painted) = with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let in_clip = window
            .rendered_frame
            .scene
            .quads
            .iter()
            .any(|quad| quad.background.pad != crate::fast::edge_fade::NONE);
        (in_clip, window.painted_quads())
    });
    assert!(in_clip, "the card's fill is drawn in its clip");
    assert!(
        painted.iter().any(|quad| quad.background == background),
        "{painted:?}"
    );
}
