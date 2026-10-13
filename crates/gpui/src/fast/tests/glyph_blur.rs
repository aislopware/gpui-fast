//! Transformed and blurred glyphs (`Window::paint_glyph_transformed`): the
//! sprite they paint, and scroll layers drawing them as drawn without. See
//! [`crate::fast::glyph_blur`].

use std::sync::Arc;

use super::element_oracle::GlyphBoxTextSystem;
use crate::{
    AnyWindowHandle, App, AppContext as _, Context, FontId, GlyphId, InteractiveElement as _,
    IntoElement, NoopTextSystem, ParentElement as _, PlatformInput, Render, ScaledPixels,
    ScrollDelta, ScrollHandle, ScrollWheelEvent, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, TouchPhase, TransformationMatrix, Window, canvas, div, hsla, point, px, size,
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

/// A digit rolling in: scaled about its centre and blurred by `blur`.
struct Rolling {
    blur: f32,
}

impl Render for Rolling {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let blur = self.blur;
        div().size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let scale = window.scale_factor();
                    let pivot = (bounds.origin + point(px(4.), px(4.))).scale(scale);
                    let matrix = TransformationMatrix::unit()
                        .translate(pivot)
                        .scale(size(1.5, 1.5))
                        .translate(point(ScaledPixels(-pivot.x.0), ScaledPixels(-pivot.y.0)));
                    window
                        .paint_glyph_transformed(
                            bounds.origin + point(px(0.), px(10.)),
                            FontId(0),
                            GlyphId(7),
                            px(12.),
                            hsla(0., 0., 0., 1.),
                            matrix,
                            px(blur),
                        )
                        .unwrap();
                },
            )
            .w(px(20.))
            .h(px(20.)),
        )
    }
}

/// The glyph is one monochrome sprite with the caller's transformation and
/// the blur, in device pixels, in its tile; scroll layers count the quad
/// the blur widens it to; a blur of 0 is none.
#[test]
fn a_transformed_glyph_carries_its_transformation_and_blur() {
    let mut cx = text_cx();
    let window = cx.add_window(|_, _| Rolling { blur: 1.25 });
    let sprite = with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        let scene = &window.rendered_frame.scene;
        assert_eq!(scene.monochrome_sprites.len(), 1);
        assert!(scene.subpixel_sprites.is_empty());
        scene.monochrome_sprites[0]
    });
    assert_ne!(sprite.transformation, TransformationMatrix::unit());
    assert_eq!(sprite.transformation.rotation_scale, [[1.5, 0.], [0., 1.5]]);
    let sigma = crate::fast::glyph_blur::sigma(&sprite);
    assert_eq!(sigma, 2.5, "1.25 points at 2x");
    let drawn = crate::fast::glyph_blur::drawn_bounds(&sprite);
    assert_eq!(drawn, sprite.bounds.dilate(ScaledPixels(7.5)));
    let layered = crate::fast::layers::scene::drawn_bounds(&sprite.into());
    assert!(layered.size.width.0 >= 1.5 * drawn.size.width.0 - 0.01);

    window
        .update(&mut cx, |rolling, _, cx| {
            rolling.blur = 0.;
            cx.notify();
        })
        .unwrap();
    let sharp = with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        window.rendered_frame.scene.monochrome_sprites[0]
    });
    assert_eq!(sharp.tile.padding, 0);
    assert_eq!(crate::fast::glyph_blur::sigma(&sharp), 0.);
    assert_eq!(crate::fast::glyph_blur::drawn_bounds(&sharp), sharp.bounds);
}

/// Rows of rolling digits in a scrolling column, each row's digits
/// blurred more than the last.
struct Column {
    scroll: ScrollHandle,
}

impl Render for Column {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(hsla(0., 0., 1., 1.)).child(
            div()
                .id("column")
                .w(px(200.))
                .h(px(120.))
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .children((0..40).map(|ix| {
                    div().h(px(20.)).flex_none().child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let blur = (ix % 4) as f32 * 0.75;
                                window
                                    .paint_glyph_transformed(
                                        bounds.origin + point(px(4.), px(14.)),
                                        FontId(0),
                                        GlyphId(ix as u32),
                                        px(12.),
                                        hsla(0., 0., 0., 1.),
                                        TransformationMatrix::unit(),
                                        px(blur),
                                    )
                                    .unwrap();
                            },
                        )
                        .size_full(),
                    )
                })),
        )
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

/// A scroll layer of blurred glyphs draws, frame by frame, what drawing
/// them without the layer draws, halos included in the tiles they reach.
#[test]
fn blurred_glyphs_scroll_on_a_layer_as_without() {
    let mut cx = text_cx();
    let new = |cx: &mut TestAppContext| {
        cx.add_window(|_, _| Column {
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
    let mut composited = 0;
    for frame in 0..30 {
        let dy = if frame < 20 { -11. } else { 13. };
        let expected = wheel_and_draw(&mut cx, direct.into(), dy);
        let actual = wheel_and_draw(&mut cx, layered.into(), dy);
        assert_eq!(actual, expected, "frame {frame}");
        composited = with_window(&mut cx, layered.into(), |window, _| {
            window.layout_stats().layer_frames_composited
        });
    }
    if crate::fast::layers::COMPILED {
        assert!(composited > 0);
    }
}
