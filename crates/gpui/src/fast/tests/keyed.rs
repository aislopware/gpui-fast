//! A stretch painted under a key and drawn again from last frame, in place
//! or moved, must draw what painting it afresh draws. See
//! [`crate::fast::keyed`].
//!
//! Each test drives a window drawing incrementally and one forgetting what
//! it retains before every frame through the same changes, and requires the
//! scenes, the hitboxes and the listeners to match in every frame.

use std::sync::Arc;

use rand::{Rng as _, SeedableRng as _, rngs::StdRng};

use super::element_oracle::GlyphBoxTextSystem;
use crate::{
    AppContext as _, Bounds, BoxShadow, ContentMask, Context, Corners, GlyphId, HitboxBehavior,
    IntoElement, LayoutStats, MouseDownEvent, NoopTextSystem, ParentElement as _, Pixels, Point,
    Render, Styled as _, TestAppContext, UnderlineStyle, Window, WindowHandle, canvas, div, font,
    hsla, point, px, size,
};

/// A stretch: the key that stands for what it paints, and where.
#[derive(Clone, Copy, Debug)]
struct Spec {
    key: u64,
    origin: Point<Pixels>,
}

/// A canvas painting stretches under their keys inside a clip, over a
/// hitbox.
struct Board {
    specs: Vec<Spec>,
    clip: Bounds<Pixels>,
    opacity: f32,
}

impl Render for Board {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let specs = self.specs.clone();
        let (clip, opacity) = (self.clip, self.opacity);
        div().size_full().child(
            canvas(
                |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                move |_, _, window, _| {
                    window.with_element_opacity(Some(opacity), |window| {
                        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                            for spec in &specs {
                                window.paint_keyed(spec.key, spec.origin, |window| {
                                    paint_stretch(spec.key, spec.origin, window);
                                });
                            }
                        });
                    });
                },
            )
            .size_full(),
        )
    }
}

/// Paints what `key` stands for at `origin`: made from the key alone, so
/// two stretches under one key paint the same relative to their origins.
fn paint_stretch(key: u64, origin: Point<Pixels>, window: &mut Window) {
    let mut rng = StdRng::seed_from_u64(key);
    // Mostly places moving keeps exact, a quarter of a logical pixel apart;
    // now and then one that it doesn't.
    let mut at = |rng: &mut StdRng, reach: f32| {
        if rng.random_ratio(1, 8) {
            px(rng.random_range(0.0..reach))
        } else {
            px((rng.random_range(0.0..reach) * 4.).floor() / 4.)
        }
    };
    let layered = rng.random_ratio(1, 3);
    let clipped = rng.random_ratio(1, 5);
    let listens = rng.random_ratio(1, 10);
    let items = rng.random_range(1..6);
    let mut plan = Vec::new();
    for _ in 0..items {
        let kind = rng.random_range(0..4);
        let place = point(origin.x + at(&mut rng, 60.), origin.y + at(&mut rng, 20.));
        let extent = size(at(&mut rng, 40.) + px(1.), at(&mut rng, 16.) + px(1.));
        let color = hsla(
            rng.random_range(0.0..1.0),
            0.6,
            0.5,
            rng.random_range(0.3..1.0),
        );
        let glyph = GlyphId(rng.random_range(1..200));
        plan.push((kind, place, extent, color, glyph));
    }
    let clip = Bounds::new(
        point(origin.x + at(&mut rng, 20.), origin.y + at(&mut rng, 6.)),
        size(at(&mut rng, 60.) + px(4.), at(&mut rng, 20.) + px(2.)),
    );
    let paint = |window: &mut Window| {
        let font_id = window.text_system().resolve_font(&font("Mono"));
        for &(kind, place, extent, color, glyph) in &plan {
            match kind {
                0 => window.paint_quad(crate::fill(Bounds::new(place, extent), color)),
                1 => window
                    .paint_glyph(
                        place + point(px(0.), px(12.)),
                        font_id,
                        glyph,
                        px(13.),
                        color,
                    )
                    .unwrap(),
                2 => window.paint_underline(
                    place,
                    extent.width,
                    &UnderlineStyle {
                        thickness: px(1.),
                        color: Some(color),
                        wavy: false,
                    },
                ),
                _ => window.paint_drop_shadows(
                    Bounds::new(place, extent),
                    Corners::all(px(2.)),
                    &[BoxShadow {
                        color,
                        offset: point(px(0.), px(1.)),
                        blur_radius: px(2.),
                        spread_radius: px(0.),
                        inset: false,
                    }],
                ),
            }
        }
    };
    let paint = |window: &mut Window| {
        if clipped {
            window.with_content_mask(Some(ContentMask { bounds: clip }), paint);
        } else {
            paint(window);
        }
    };
    if layered {
        window.paint_layer(Bounds::new(origin, size(px(100.), px(40.))), paint);
    } else {
        paint(window);
    }
    if listens {
        window.on_mouse_event(|_: &MouseDownEvent, _, _, _| {});
    }
}

/// Opens the two windows, the second sharing the first's atlas.
fn windows(cx: &mut TestAppContext, board: impl Fn() -> Board) -> [WindowHandle<Board>; 2] {
    let incremental = cx.add_window(|_, _| board());
    let from_scratch = cx.add_window(|_, _| board());
    let atlas = cx
        .update_window(incremental.into(), |_, window, _| {
            window.sprite_atlas.clone()
        })
        .unwrap();
    cx.update_window(from_scratch.into(), |_, window, _| {
        window.sprite_atlas = atlas;
    })
    .unwrap();
    [incremental, from_scratch]
}

/// Changes both boards as `change` says, draws the second from scratch,
/// requires the frames to match, and returns the first's stats.
fn step(
    cx: &mut TestAppContext,
    [incremental, from_scratch]: [WindowHandle<Board>; 2],
    change: impl Fn(&mut Board),
) -> LayoutStats {
    for window in [incremental, from_scratch] {
        window
            .update(cx, |board, _, cx| {
                change(board);
                cx.notify();
            })
            .unwrap();
    }
    let draw = |cx: &mut TestAppContext, window: WindowHandle<Board>, forget: bool| {
        cx.update_window(window.into(), |_, window, cx| {
            if forget {
                window.forget_retained_state();
                window.draw(cx).clear(cx);
            }
            let stats = window.layout_stats();
            window.reset_layout_stats();
            let listeners = window.rendered_frame.mouse_listeners.len();
            (window.describe_rendered_frame(), listeners, stats)
        })
        .unwrap()
    };
    let (expected, expected_listeners, _) = draw(cx, from_scratch, true);
    let (actual, listeners, stats) = draw(cx, incremental, false);
    assert_eq!(actual, expected);
    assert_eq!(listeners, expected_listeners);
    stats
}

fn text_cx() -> TestAppContext {
    TestAppContext::with_text_system(Arc::new(GlyphBoxTextSystem(NoopTextSystem)))
}

/// Rows of a grid under a clip that stands still, keyed by content.
fn rows(rows: u64, first: u64) -> Vec<Spec> {
    (0..rows)
        .map(|row| Spec {
            key: first + row,
            origin: point(px(10.), px(10. + 20. * row as f32)),
        })
        .collect()
}

/// A still grid is drawn again from last frame, every row of it.
#[test]
fn a_still_stretch_is_drawn_again() {
    let mut cx = text_cx();
    let clip = Bounds::new(point(px(5.), px(5.)), size(px(300.), px(200.)));
    let windows = windows(&mut cx, || Board {
        specs: rows(8, 100),
        clip,
        opacity: 1.,
    });
    step(&mut cx, windows, |_| {});
    let stats = step(&mut cx, windows, |_| {});
    assert!(stats.paints_replayed >= 6, "{stats:?}");
    assert_eq!(stats.paints_moved, 0);
}

/// Output scrolling a row a frame under a clip that stands still: every row
/// but the new one is drawn again moved, and the one scrolled out of the
/// clip's top is clipped as painting it there would.
#[test]
fn rows_scrolling_under_a_still_clip_are_drawn_again_moved() {
    let mut cx = text_cx();
    let clip = Bounds::new(point(px(5.), px(15.)), size(px(300.), px(150.)));
    let windows = windows(&mut cx, || Board {
        specs: rows(8, 100),
        clip,
        opacity: 1.,
    });
    step(&mut cx, windows, |_| {});
    let mut moved = 0;
    for first in 101..110 {
        moved += step(&mut cx, windows, |board| board.specs = rows(8, first)).paints_moved;
    }
    assert!(moved > 20, "moved {moved}");
}

/// Stretches moved, re-keyed, added and dropped at random, under a clip that
/// moves or stands still, all match what painting afresh paints.
#[test]
fn randomized_keyed_stretches_match_painting_afresh() {
    let mut replayed = 0;
    let mut moved = 0;
    for seed in 0..48 {
        let mut cx = text_cx();
        let mut rng = StdRng::seed_from_u64(seed);
        let start = rows(rng.random_range(1..10), rng.random_range(0..1_000));
        let clip = Bounds::new(point(px(5.), px(5.)), size(px(260.), px(180.)));
        let windows = windows(&mut cx, || Board {
            specs: start.clone(),
            clip,
            opacity: 1.,
        });
        step(&mut cx, windows, |_| {});
        for _ in 0..30 {
            let mut change: Vec<Box<dyn Fn(&mut Board)>> = Vec::new();
            for _ in 0..rng.random_range(1..4) {
                let index: usize = rng.random_range(0..16);
                let value: f32 = rng.random_range(-12.0..12.0);
                change.push(match rng.random_range(0..9) {
                    // By whole device pixels.
                    0..=2 => Box::new(move |board: &mut Board| {
                        let dy = px((value * 2.).round() / 2.);
                        let len = board.specs.len().max(1);
                        if let Some(spec) = board.specs.get_mut(index % len) {
                            spec.origin.y += dy;
                        }
                    }),
                    // Every stretch, as output scrolling does.
                    3 => Box::new(move |board: &mut Board| {
                        let dy = px((value * 2.).round() / 2.);
                        for spec in &mut board.specs {
                            spec.origin.y += dy;
                        }
                    }),
                    // By part of a device pixel.
                    4 => Box::new(move |board: &mut Board| {
                        let len = board.specs.len().max(1);
                        if let Some(spec) = board.specs.get_mut(index % len) {
                            spec.origin.x += px(value / 7.);
                        }
                    }),
                    5 => Box::new(move |board: &mut Board| {
                        let len = board.specs.len().max(1);
                        if let Some(spec) = board.specs.get_mut(index % len) {
                            spec.key = spec.key.wrapping_mul(31).wrapping_add(index as u64);
                        }
                    }),
                    6 => Box::new(move |board: &mut Board| {
                        board.specs.push(Spec {
                            key: index as u64 * 7 + 3,
                            origin: point(px(value.abs() * 10.), px(value.abs() * 12.)),
                        });
                    }),
                    7 => Box::new(move |board: &mut Board| {
                        if !board.specs.is_empty() {
                            let index = index % board.specs.len();
                            board.specs.remove(index);
                        }
                    }),
                    // The clip moves, by whole device pixels or not, or grows.
                    _ => Box::new(move |board: &mut Board| {
                        if index.is_multiple_of(2) {
                            board.clip.origin.y += px((value * 2.).round() / 2.);
                        } else if index.is_multiple_of(3) {
                            board.clip.size.height += px(value.abs());
                        } else {
                            board.clip.origin.x += px(value / 5.);
                        }
                    }),
                });
            }
            if rng.random_ratio(1, 12) {
                let opacity = rng.random_range(0.5..1.0);
                change.push(Box::new(move |board: &mut Board| board.opacity = opacity));
            }
            let stats = step(&mut cx, windows, |board| {
                for change in &change {
                    change(board);
                }
            });
            replayed += stats.paints_replayed;
            moved += stats.paints_moved;
        }
    }
    assert!(replayed > 1000, "replayed {replayed}");
    assert!(moved > 200, "moved {moved}");
}

/// A board whose one stretch pushes a clip reaching past the board's own
/// and paints a quad sticking out of that clip.
struct Overreach {
    top: f32,
}

impl Render for Overreach {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let top = self.top;
        div().size_full().child(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let board = Bounds::new(point(px(0.), px(60.)), size(px(200.), px(100.)));
                    window.with_content_mask(Some(ContentMask { bounds: board }), |window| {
                        let origin = point(px(10.), px(top));
                        window.paint_keyed(7, origin, |window| {
                            let clip = Bounds::new(
                                point(origin.x - px(20.), origin.y - px(10.)),
                                size(px(300.), px(300.)),
                            );
                            window.with_content_mask(
                                Some(ContentMask { bounds: clip }),
                                |window| {
                                    let quad = Bounds::new(
                                        point(origin.x, origin.y - px(30.)),
                                        size(px(50.), px(80.)),
                                    );
                                    window.paint_quad(crate::fill(quad, hsla(0.5, 0.5, 0.5, 1.)));
                                },
                            );
                        });
                    });
                },
            )
            .size_full(),
        )
    }
}

/// The stretch's own clip is the board's where it begins, so the quad is
/// clipped by the board; moved down, it is clipped by its own clip, which
/// moved with it, and not by the board's, which stood still.
#[test]
fn a_clip_of_the_stretchs_own_equal_to_the_one_around_is_not_taken_for_it() {
    let mut cx = text_cx();
    let incremental = cx.add_window(|_, _| Overreach { top: 65. });
    let from_scratch = cx.add_window(|_, _| Overreach { top: 65. });
    for top in [65., 75., 85.] {
        let mut frames = Vec::new();
        for (window, forget) in [(from_scratch, true), (incremental, false)] {
            window
                .update(&mut cx, |board, _, cx| {
                    board.top = top;
                    cx.notify();
                })
                .unwrap();
            frames.push(
                cx.update_window(window.into(), |_, window, cx| {
                    if forget {
                        window.forget_retained_state();
                        window.draw(cx).clear(cx);
                    }
                    window.describe_rendered_frame()
                })
                .unwrap(),
            );
        }
        assert_eq!(frames[1], frames[0], "at {top}");
    }
}

/// Two boards, each a view of its own.
struct Pair {
    boards: [crate::Entity<Board>; 2],
}

impl Render for Pair {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(div().w(px(300.)).h_full().child(self.boards[0].clone()))
            .child(div().w(px(300.)).h_full().child(self.boards[1].clone()))
    }
}

/// A board drawn again whole while the other is built carries its keyed
/// stretches into the frame, so it finds them when it is built again.
#[test]
fn a_view_drawn_again_whole_keeps_its_stretches() {
    let mut cx = text_cx();
    let clip = Bounds::new(point(px(0.), px(0.)), size(px(600.), px(400.)));
    let window = cx.add_window(|_, cx| Pair {
        boards: [0, 1].map(|board| {
            cx.new(|_| Board {
                specs: rows(6, 100 + board * 50),
                clip,
                opacity: 1.,
            })
        }),
    });
    let boards = window
        .update(&mut cx, |pair, _, _| pair.boards.clone())
        .unwrap();
    let draw = |cx: &mut TestAppContext, board: usize| {
        boards[board].update(cx, |_, cx| cx.notify());
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let stats = window.layout_stats();
            window.reset_layout_stats();
            stats
        })
        .unwrap()
    };
    draw(&mut cx, 0);
    draw(&mut cx, 0);
    draw(&mut cx, 0);
    // A row whose paint is all culled, at the board's edge, is painted
    // again rather than carried.
    let stats = draw(&mut cx, 1);
    assert!(stats.paints_replayed >= 5, "{stats:?}");
}
