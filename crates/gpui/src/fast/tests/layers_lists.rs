//! Tests of scroll layers over virtual lists (M6): a `uniform_list` or
//! `list` whose layer is extended by the rows a scroll uncovers, rendering
//! only those.

use crate::fast::layers::policy::{Decision, last_decision};
use crate::{
    AnyWindowHandle, App, AppContext as _, Bounds, GlobalElementId, Hsla, ScaledPixels, Scene,
    ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, Window, point, px,
};

/// How tall the list is, in pixels.
const VIEWPORT_HEIGHT: f32 = 100.;
/// How wide the list is, in pixels.
const VIEWPORT_WIDTH: f32 = 200.;
/// How tall a row of a uniform list is, in pixels.
const ROW_HEIGHT: f32 = 20.;

fn with_window<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| f(window, cx))
        .unwrap()
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    with_window(cx, window, |window, cx| window.draw(cx).clear(cx));
}

/// Scrolls by `dy` with the wheel over the list, and draws the frame that
/// follows, unless the scroll drew it.
fn wheel(cx: &mut TestAppContext, window: AnyWindowHandle, dy: f32) {
    let frame = with_window(cx, window, |window, _| window.fast_layers.frame);
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
    });
    if with_window(cx, window, |window, _| window.fast_layers.frame) == frame {
        draw(cx, window);
    }
}

/// The id of the window's only layer.
fn layer_id(cx: &mut TestAppContext, window: AnyWindowHandle) -> GlobalElementId {
    with_window(cx, window, |window, _| {
        assert_eq!(window.fast_layers.layers.len(), 1, "one layer");
        window.fast_layers.layers.keys().next().unwrap().clone()
    })
}

/// What the window's only scroll container decided in the last frame.
fn decision(cx: &mut TestAppContext, window: AnyWindowHandle) -> Option<Decision> {
    let id = layer_id(cx, window);
    with_window(cx, window, |window, _| last_decision(window, &id))
}

/// The rows the window's only layer holds.
fn held_rows(cx: &mut TestAppContext, window: AnyWindowHandle) -> Vec<usize> {
    with_window(cx, window, |window, _| {
        let layer = window.fast_layers.layers.values().next().expect("a layer");
        layer.rows.painted.iter().copied().collect()
    })
}

/// A row's colour.
fn row_color(row: usize) -> Hsla {
    crate::hsla((row % 97) as f32 / 97., 0.5, 0.5, 1.)
}

/// The quads `scene` draws, with the layers it composites expanded: each
/// layer's content moved to where its tiles are composited and clipped to
/// their viewport, as drawing the content straight into the frame would
/// have put it. Each quad is its bounds, the part of them its content mask
/// lets it draw, and its colour, sorted.
fn expanded_quads(scene: &Scene) -> Vec<String> {
    let mut quads = Vec::new();
    let describe = |bounds: Bounds<ScaledPixels>,
                    mask: Bounds<ScaledPixels>,
                    background: &crate::Background| {
        let drawn = bounds.intersect(&mask);
        format!("{bounds:?} {drawn:?} {background:?}")
    };
    for quad in &scene.quads {
        quads.push(describe(
            quad.bounds,
            quad.content_mask.bounds,
            &quad.background,
        ));
    }
    for frame in &scene.layers.frames {
        let sprite = scene
            .polychrome_sprites
            .iter()
            .find_map(|sprite| {
                crate::decode_layer_tile(sprite.tile.texture_id, sprite.tile.tile_id)
                    .filter(|(key, _)| *key == frame.key)
                    .map(|(_, coord)| (coord, sprite))
            })
            .expect("a composited layer has tiles");
        let (coord, sprite) = sprite;
        let tile = frame.tile_bounds(coord);
        let translation = sprite.bounds.origin - tile.origin;
        let viewport = sprite.content_mask.bounds;
        for quad in &frame.content.quads {
            let bounds = Bounds {
                origin: quad.bounds.origin + translation,
                size: quad.bounds.size,
            };
            let mask = Bounds {
                origin: quad.content_mask.bounds.origin + translation,
                size: quad.content_mask.bounds.size,
            }
            .intersect(&viewport);
            if bounds.intersect(&mask).is_empty() {
                continue;
            }
            quads.push(describe(bounds, mask, &quad.background));
        }
    }
    quads.sort();
    quads
}

/// Whether the window's last frame composited a layer.
fn composites(cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
    with_window(cx, window, |window, _| {
        !window.rendered_frame.scene.layers.frames.is_empty()
    })
}

/// A pseudo-random sequence of wheel deltas, whole pixels in -60..=60.
fn wheel_deltas(count: usize) -> Vec<f32> {
    let mut state: u32 = 0x2545_f491;
    (0..count)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            ((state >> 16) % 121) as f32 - 60.
        })
        .collect()
}

mod uniform {
    use super::{
        Decision, ROW_HEIGHT, VIEWPORT_HEIGHT, VIEWPORT_WIDTH, composites, decision, draw,
        expanded_quads, held_rows, row_color, wheel, wheel_deltas, with_window,
    };
    use crate::{
        AnyWindowHandle, Context, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, Window, WindowHandle, div, px, rgb,
    };
    use std::{cell::RefCell, ops::Range, rc::Rc};

    /// A white panel holding a uniform list of `count` rows of 20 px, 100 px
    /// tall, at the top left of the window. Every range of rows the list
    /// renders is logged.
    pub(super) struct UniformPage {
        pub(super) count: usize,
        pub(super) rendered: Rc<RefCell<Vec<Range<usize>>>>,
    }

    impl Render for UniformPage {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let rendered = self.rendered.clone();
            div().size_full().bg(rgb(0xffffff)).child(
                crate::uniform_list(
                    "list",
                    self.count,
                    cx.processor(move |_, range: Range<usize>, _, _| {
                        rendered.borrow_mut().push(range.clone());
                        range
                            .map(|row| {
                                div()
                                    .w(px(VIEWPORT_WIDTH))
                                    .h(px(ROW_HEIGHT))
                                    .bg(row_color(row))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .w(px(VIEWPORT_WIDTH))
                .h(px(VIEWPORT_HEIGHT)),
            )
        }
    }

    pub(super) fn page(
        cx: &mut TestAppContext,
        count: usize,
    ) -> (WindowHandle<UniformPage>, Rc<RefCell<Vec<Range<usize>>>>) {
        let rendered = Rc::new(RefCell::new(Vec::new()));
        let log = rendered.clone();
        let window = cx.add_window(move |_, _| UniformPage {
            count,
            rendered: log,
        });
        draw(cx, window.into());
        draw(cx, window.into());
        (window, rendered)
    }

    /// The rows rendered since the log was last taken, leaving out the
    /// measured item (row 0 alone).
    fn rendered_rows(log: &Rc<RefCell<Vec<Range<usize>>>>) -> Vec<Range<usize>> {
        std::mem::take(&mut *log.borrow_mut())
            .into_iter()
            .filter(|range| *range != (0..1))
            .collect()
    }

    /// Scrolls until the list has a layer: promoted on the second scrolled
    /// frame.
    fn promote(cx: &mut TestAppContext, window: AnyWindowHandle) {
        wheel(cx, window, -ROW_HEIGHT);
        assert_eq!(decision(cx, window), Some(Decision::Bypass));
        wheel(cx, window, -ROW_HEIGHT);
        assert_eq!(decision(cx, window), Some(Decision::Repaint));
    }

    #[crate::test]
    fn uniform_list_renders_only_new_rows_when_scrolling(cx: &mut TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let (handle, log) = page(cx, 1000);
        let window = handle.into();
        promote(cx, window);
        // Rows 2..7 show; five rows of overscan above (as far as row 0) and
        // below.
        assert_eq!(held_rows(cx, window), (0..12).collect::<Vec<_>>());
        rendered_rows(&log);

        for step in 0..10 {
            wheel(cx, window, -ROW_HEIGHT);
            assert_eq!(decision(cx, window), Some(Decision::Composite));
            let new_row = 12 + step;
            assert_eq!(
                rendered_rows(&log),
                vec![new_row..new_row + 1],
                "scrolling down one row renders the row entering the overscan"
            );
        }
        // Rows 12..17 show; rows that left the overscan were dropped.
        assert_eq!(held_rows(cx, window), (7..22).collect::<Vec<_>>());

        wheel(cx, window, ROW_HEIGHT);
        assert_eq!(decision(cx, window), Some(Decision::Composite));
        assert_eq!(
            rendered_rows(&log),
            vec![6..7],
            "scrolling up one row renders the row entering the overscan"
        );
        assert_eq!(held_rows(cx, window), (6..21).collect::<Vec<_>>());

        // Rows 11..16 show; a quarter of a row down, 11..17 do.
        wheel(cx, window, -ROW_HEIGHT / 4.);
        assert_eq!(decision(cx, window), Some(Decision::Composite));
        assert_eq!(rendered_rows(&log), vec![21..22]);
        wheel(cx, window, -ROW_HEIGHT / 4.);
        assert_eq!(decision(cx, window), Some(Decision::Composite));
        assert_eq!(
            rendered_rows(&log),
            Vec::<Range<usize>>::new(),
            "a scroll that uncovers no row renders none"
        );
    }

    #[crate::test]
    fn uniform_list_matches_layers_off(cx: &mut TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let (with_layers, _) = page(cx, 300);
        let (without_layers, _) = page(cx, 300);
        let with_layers: AnyWindowHandle = with_layers.into();
        let without_layers: AnyWindowHandle = without_layers.into();
        with_window(cx, without_layers, |window, _| {
            window.set_scroll_layers(false)
        });
        draw(cx, without_layers);

        let mut composited = 0;
        for (frame, dy) in wheel_deltas(50).into_iter().enumerate() {
            wheel(cx, with_layers, dy);
            wheel(cx, without_layers, dy);
            if composites(cx, with_layers) {
                composited += 1;
            }
            let expected = with_window(cx, without_layers, |window, _| {
                assert!(window.rendered_frame.scene.layers.frames.is_empty());
                expanded_quads(&window.rendered_frame.scene)
            });
            let actual = with_window(cx, with_layers, |window, _| {
                expanded_quads(&window.rendered_frame.scene)
            });
            assert_eq!(actual, expected, "frame {frame}, scrolled by {dy}");
        }
        assert!(composited > 40, "the layer was composited ({composited})");
    }

    #[crate::test]
    fn measure_item_is_skipped_on_composite_frames(cx: &mut TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let (handle, log) = page(cx, 1000);
        let window = handle.into();
        let measured = |log: &Rc<RefCell<Vec<Range<usize>>>>| {
            std::mem::take(&mut *log.borrow_mut())
                .into_iter()
                .filter(|range| *range == (0..1))
                .count()
        };
        measured(&log);
        // Layout and prepaint each measure row 0.
        wheel(cx, window, -ROW_HEIGHT);
        assert_eq!(measured(&log), 2);
        promote_from_second_frame(cx, window);
        measured(&log);

        wheel(cx, window, -ROW_HEIGHT);
        assert_eq!(decision(cx, window), Some(Decision::Composite));
        assert_eq!(measured(&log), 1, "only layout measures row 0");

        let frame = with_window(cx, window, |window, _| window.fast_layers.frame);
        handle
            .update(cx, |page, _, cx| {
                page.count = 999;
                cx.notify();
            })
            .unwrap();
        if with_window(cx, window, |window, _| window.fast_layers.frame) == frame {
            draw(cx, window);
        }
        assert_eq!(decision(cx, window), Some(Decision::Repaint));
        assert_eq!(measured(&log), 2, "a frame that changed measures again");
    }

    /// The second scrolled frame, which promotes the list.
    fn promote_from_second_frame(cx: &mut TestAppContext, window: AnyWindowHandle) {
        wheel(cx, window, -ROW_HEIGHT);
        assert_eq!(decision(cx, window), Some(Decision::Repaint));
    }
}

mod list {
    use super::{
        Decision, VIEWPORT_HEIGHT, VIEWPORT_WIDTH, composites, decision, draw, expanded_quads,
        held_rows, row_color, wheel, wheel_deltas, with_window,
    };
    use crate::{
        AnyWindowHandle, Context, IntoElement, ListAlignment, ListState, ParentElement as _,
        Render, Styled as _, TestAppContext, Window, WindowHandle, div, px, rgb,
    };
    use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

    /// How tall row `row` is: 20, 30 or 40 px.
    fn row_height(row: usize) -> f32 {
        20. + (row % 3) as f32 * 10.
    }

    /// A white panel holding a list of rows of varying heights, 100 px tall,
    /// at the top left of the window. Every row the list renders is logged.
    pub(super) struct ListPage {
        pub(super) state: ListState,
        pub(super) rendered: Rc<RefCell<Vec<usize>>>,
    }

    impl Render for ListPage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let rendered = self.rendered.clone();
            div().size_full().bg(rgb(0xffffff)).child(
                crate::list(self.state.clone(), move |row, _, _| {
                    rendered.borrow_mut().push(row);
                    div()
                        .w(px(VIEWPORT_WIDTH))
                        .h(px(row_height(row)))
                        .bg(row_color(row))
                        .into_any_element()
                })
                .w(px(VIEWPORT_WIDTH))
                .h(px(VIEWPORT_HEIGHT)),
            )
        }
    }

    fn page(
        cx: &mut TestAppContext,
        state: ListState,
    ) -> (WindowHandle<ListPage>, Rc<RefCell<Vec<usize>>>) {
        let rendered = Rc::new(RefCell::new(Vec::new()));
        let log = rendered.clone();
        let window = cx.add_window(move |_, _| ListPage {
            state,
            rendered: log,
        });
        draw(cx, window.into());
        draw(cx, window.into());
        (window, rendered)
    }

    fn promote(cx: &mut TestAppContext, window: AnyWindowHandle) {
        wheel(cx, window, -20.);
        assert_eq!(decision(cx, window), Some(Decision::Bypass));
        wheel(cx, window, -20.);
        assert_eq!(decision(cx, window), Some(Decision::Repaint));
    }

    /// The rows rendered since the log was last taken.
    fn rendered(log: &Rc<RefCell<Vec<usize>>>) -> BTreeSet<usize> {
        log.borrow_mut().drain(..).collect()
    }

    #[crate::test]
    fn list_renders_only_new_rows_when_scrolling(cx: &mut TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let state = ListState::new(1000, ListAlignment::Top, px(0.)).measure_all();
        let (handle, log) = page(cx, state);
        let window = handle.into();
        promote(cx, window);
        let mut held: BTreeSet<usize> = held_rows(cx, window).into_iter().collect();
        assert!(held.contains(&0) && held.len() > 5, "held {held:?}");
        rendered(&log);

        let mut rendered_any = false;
        for step in 0..30 {
            wheel(cx, window, -15.);
            assert_eq!(
                decision(cx, window),
                Some(Decision::Composite),
                "step {step}"
            );
            let rendered = rendered(&log);
            let now: BTreeSet<usize> = held_rows(cx, window).into_iter().collect();
            let added: BTreeSet<usize> = now.difference(&held).copied().collect();
            assert_eq!(
                rendered, added,
                "step {step}: only the rows new to the layer are rendered"
            );
            assert!(rendered.len() <= 2, "step {step}: {rendered:?}");
            rendered_any |= !rendered.is_empty();
            held = now;
        }
        assert!(rendered_any);
        assert!(
            held.first().copied().unwrap_or(0) > 0,
            "rows left behind are dropped"
        );

        for step in 0..10 {
            wheel(cx, window, 25.);
            assert_eq!(decision(cx, window), Some(Decision::Composite), "up {step}");
            let rendered = rendered(&log);
            let now: BTreeSet<usize> = held_rows(cx, window).into_iter().collect();
            let added: BTreeSet<usize> = now.difference(&held).copied().collect();
            assert_eq!(rendered, added, "up {step}");
            held = now;
        }
    }

    #[crate::test]
    fn list_matches_layers_off_with_varying_heights(cx: &mut TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let (with_layers, _) = page(cx, ListState::new(300, ListAlignment::Top, px(50.)));
        let (without_layers, _) = page(cx, ListState::new(300, ListAlignment::Top, px(50.)));
        let with_layers: AnyWindowHandle = with_layers.into();
        let without_layers: AnyWindowHandle = without_layers.into();
        with_window(cx, without_layers, |window, _| {
            window.set_scroll_layers(false)
        });
        draw(cx, without_layers);

        let mut composited = 0;
        let mut deltas = wheel_deltas(50);
        // And back up past where it started, through rows measured on the way.
        deltas.extend([60., 60., 60., 45., 60., 60., 60., 60., 33., 60.]);
        for (frame, dy) in deltas.into_iter().enumerate() {
            wheel(cx, with_layers, dy);
            wheel(cx, without_layers, dy);
            if composites(cx, with_layers) {
                composited += 1;
            }
            let expected = with_window(cx, without_layers, |window, _| {
                assert!(window.rendered_frame.scene.layers.frames.is_empty());
                expanded_quads(&window.rendered_frame.scene)
            });
            let actual = with_window(cx, with_layers, |window, _| {
                expanded_quads(&window.rendered_frame.scene)
            });
            assert_eq!(actual, expected, "frame {frame}, scrolled by {dy}");
        }
        assert!(composited > 40, "the layer was composited ({composited})");
    }

    #[crate::test]
    fn a_list_splice_repaints_the_layer(cx: &mut TestAppContext) {
        if !crate::fast::layers::COMPILED {
            return;
        }
        let state = ListState::new(1000, ListAlignment::Top, px(0.)).measure_all();
        let (handle, _) = page(cx, state);
        let window = handle.into();
        promote(cx, window);
        wheel(cx, window, -20.);
        assert_eq!(decision(cx, window), Some(Decision::Composite));

        let frame = with_window(cx, window, |window, _| window.fast_layers.frame);
        handle
            .update(cx, |page, _, cx| {
                page.state.splice(3..4, 2);
                cx.notify();
            })
            .unwrap();
        if with_window(cx, window, |window, _| window.fast_layers.frame) == frame {
            draw(cx, window);
        }
        assert_eq!(decision(cx, window), Some(Decision::Repaint));

        wheel(cx, window, -20.);
        assert_eq!(decision(cx, window), Some(Decision::Composite));
    }
}
