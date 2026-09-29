//! Natives placed by the frame: their draw order among GPUI's primitives,
//! whether the platform shows them, where they take the pointer and the
//! keyboard. See [`crate::fast::composition`].

use rand::{Rng as _, SeedableRng as _, rngs::StdRng};

use crate::{
    AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Quad, Render, ScaledPixels, Scene, StatefulInteractiveElement as _,
    Styled as _, TestAppContext, Window, WindowHandle, anchored,
    composition::{
        ComposedBatch, NativeHost, NativeHostOptions, NativeId, NativePlacement, TestNativeHost,
        native_view,
    },
    deferred, div, point,
    prelude::FluentBuilder as _,
    px, rgb, size,
};

fn test_host(host: &NativeHost) -> &TestNativeHost {
    host.platform()
        .as_any()
        .downcast_ref::<TestNativeHost>()
        .expect("the test platform's host")
}

fn draw<V: 'static>(cx: &mut TestAppContext, window: WindowHandle<V>) {
    cx.update_window(window.into(), |_, window, cx| window.draw_and_present(cx))
        .unwrap();
}

fn scene_quad(bounds: Bounds<ScaledPixels>) -> Quad {
    Quad {
        bounds,
        content_mask: crate::ContentMask {
            bounds: Bounds::new(
                point(ScaledPixels(0.), ScaledPixels(0.)),
                size(ScaledPixels(1e4), ScaledPixels(1e4)),
            ),
        },
        background: rgb(0x336699).into(),
        ..Quad::default()
    }
}

fn scaled(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x), ScaledPixels(y)),
        size(ScaledPixels(w), ScaledPixels(h)),
    )
}

/// What a finished scene draws, in drawing order: `quad <order>`,
/// `hole <order>` and so on.
fn drawing_order(scene: &Scene) -> Vec<(char, u32)> {
    let mut drawn = Vec::new();
    for batch in scene.composed_batches() {
        match batch {
            ComposedBatch::Holes(range) => drawn.extend(
                scene.natives().holes[range]
                    .iter()
                    .map(|hole| ('h', hole.order)),
            ),
            ComposedBatch::Primitives(crate::PrimitiveBatch::Quads(range)) => {
                drawn.extend(scene.quads[range].iter().map(|quad| ('q', quad.order)))
            }
            ComposedBatch::Primitives(crate::PrimitiveBatch::Shadows(range)) => drawn.extend(
                scene.shadows[range]
                    .iter()
                    .map(|shadow| ('s', shadow.order)),
            ),
            ComposedBatch::Primitives(other) => panic!("unexpected batch {other:?}"),
        }
    }
    drawn
}

struct Stack {
    host: NativeHost,
    popover: bool,
}

impl Render for Stack {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(rgb(0x101010))
            .child(
                native_view(&self.host)
                    .absolute()
                    .top(px(10.))
                    .left(px(10.))
                    .size(px(100.))
                    .rounded(px(8.))
                    .bg(rgb(0x202020)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(60.))
                    .left(px(60.))
                    .size(px(100.))
                    .bg(rgb(0x303030)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(10.))
                    .left(px(300.))
                    .size(px(50.))
                    .bg(rgb(0x404040)),
            )
            .when(self.popover, |this| {
                this.child(
                    div().absolute().top(px(0.)).left(px(0.)).child(deferred(
                        anchored()
                            .position(point(px(20.), px(20.)))
                            .child(div().size(px(40.)).bg(rgb(0x505050)).occlude()),
                    )),
                )
            })
    }
}

fn host(window: &mut Window, cx: &mut crate::App) -> NativeHost {
    window
        .create_native_host(NativeHostOptions::default(), cx)
        .expect("the test platform composes natives")
}

#[test]
fn what_is_painted_after_a_native_and_over_it_is_drawn_above_it() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| Stack {
        host: host(window, cx),
        popover: true,
    });
    draw(&mut cx, window);
    cx.update_window(window.into(), |_, window, _| {
        let scene = &window.rendered_frame.scene;
        let hole = scene.natives().holes[0].order;
        let quad_order = |color: u32| {
            scene
                .quads
                .iter()
                .find(|quad| quad.background.solid == rgb(color).into())
                .map(|quad| quad.order)
                .unwrap()
        };
        assert!(
            quad_order(0x101010) < hole,
            "the window's background is under it"
        );
        assert!(
            quad_order(0x202020) < hole,
            "the native view's own background is under it"
        );
        assert!(
            quad_order(0x303030) > hole,
            "an overlapping sibling is over it"
        );
        assert!(
            quad_order(0x404040) <= hole,
            "a sibling beside it is not pushed over it"
        );
        assert!(quad_order(0x505050) > hole, "a popover is over it");

        let drawn = drawing_order(scene);
        let hole_at = drawn.iter().position(|(kind, _)| *kind == 'h').unwrap();
        for (ix, (kind, order)) in drawn.iter().enumerate() {
            if *kind == 'q' && *order > hole {
                assert!(ix > hole_at, "{drawn:?}");
            }
            if *kind == 'q' && *order < hole {
                assert!(ix < hole_at, "{drawn:?}");
            }
        }
    })
    .unwrap();
}

#[test]
fn a_native_in_a_paint_layer_puts_the_rest_of_the_layer_above_it() {
    let mut scene = Scene::default();
    scene.insert_primitive(scene_quad(scaled(0., 0., 200., 200.)));
    scene.push_layer(scaled(0., 0., 100., 100.));
    scene.insert_primitive(scene_quad(scaled(0., 0., 50., 50.)));
    scene.insert_native(NativePlacement {
        order: 0,
        id: NativeId(0),
        hitbox: None,
        focus: None,
        bounds: scaled(10., 10., 40., 40.),
        content_mask: crate::ContentMask {
            bounds: scaled(0., 0., 1000., 1000.),
        },
        corner_radii: Default::default(),
        opacity: 1.,
    });
    scene.insert_primitive(scene_quad(scaled(20., 20., 50., 50.)));
    scene.pop_layer();
    scene.insert_primitive(scene_quad(scaled(0., 0., 100., 100.)));
    scene.finish();

    let hole = scene.natives().holes[0].order;
    let orders: Vec<u32> = scene.quads.iter().map(|quad| quad.order).collect();
    assert!(
        orders[0] < hole,
        "the window below: {orders:?}, hole {hole}"
    );
    assert!(
        orders[1] < hole,
        "the layer before the native: {orders:?}, hole {hole}"
    );
    assert!(
        orders[2] > hole,
        "the layer after the native: {orders:?}, hole {hole}"
    );
    assert!(
        orders[3] > orders[2],
        "what follows the layer is still above the layer: {orders:?}"
    );
    let drawn = drawing_order(&scene);
    assert_eq!(
        drawn.iter().map(|(kind, _)| *kind).collect::<String>(),
        "qqhqq",
        "{drawn:?}"
    );
}

struct Picture {
    host: NativeHost,
}

impl Render for Picture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        native_view(&self.host).size(px(64.))
    }
}

struct Tiles {
    picture: Entity<Picture>,
    shown: bool,
    offset: f32,
}

impl Render for Tiles {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .pl(px(self.offset))
            .child(div().size(px(20.)).child("label"))
            .when(self.shown, |this| this.child(self.picture.clone()))
    }
}

#[test]
fn a_native_shows_while_it_is_placed_and_a_replayed_placement_costs_the_platform_nothing() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| {
        let host = host(window, cx);
        Tiles {
            picture: cx.new(|_| Picture { host }),
            shown: true,
            offset: 0.,
        }
    });
    let host = window
        .update(&mut cx, |tiles, _, cx| tiles.picture.read(cx).host.clone())
        .unwrap();
    let presents = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, _| {
            let mut window = window.platform_window.as_test().unwrap().0.lock();
            let composition = &mut window.composition;
            (
                composition.transactional_presents,
                composition.plain_presents,
            )
        })
        .unwrap()
    };

    draw(&mut cx, window);
    let test = test_host(&host);
    assert!(!test.is_hidden());
    assert_eq!(test.calls(), 1);
    let placement = test.placement().unwrap();
    assert_eq!(placement.bounds.size, size(px(64.), px(64.)));
    assert_eq!(placement.bounds.origin.x, px(20.));
    let (transactional, _) = presents(&mut cx);

    window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
    draw(&mut cx, window);
    let (retaining, reused) = cx
        .update_window(window.into(), |_, window, _| {
            (
                window.retained_state.view_retention,
                window.rendered_frame.retained.reused_any(),
            )
        })
        .unwrap();
    assert_eq!(
        reused, retaining,
        "the picture was drawn from the last frame"
    );
    assert!(!test.is_hidden());
    assert_eq!(test.calls(), 1, "a replayed placement changes nothing");
    assert_eq!(presents(&mut cx).0, transactional);

    window
        .update(&mut cx, |tiles, _, cx| {
            tiles.offset = 10.;
            cx.notify();
        })
        .unwrap();
    draw(&mut cx, window);
    assert_eq!(test.calls(), 2);
    assert_eq!(test.placement().unwrap().bounds.origin.x, px(30.));
    assert_eq!(presents(&mut cx).0, transactional + 1);

    window
        .update(&mut cx, |tiles, _, cx| {
            tiles.shown = false;
            cx.notify();
        })
        .unwrap();
    draw(&mut cx, window);
    assert!(test.is_hidden(), "a native no element placed is hidden");
    assert_eq!(test.calls(), 3);

    window
        .update(&mut cx, |tiles, _, cx| {
            tiles.shown = true;
            cx.notify();
        })
        .unwrap();
    draw(&mut cx, window);
    assert!(!test.is_hidden());
    assert_eq!(test.calls(), 4);
}

struct Focusable {
    host: NativeHost,
    native_focus: FocusHandle,
    other_focus: FocusHandle,
}

impl Render for Focusable {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                native_view(&self.host)
                    .size(px(50.))
                    .track_focus(&self.native_focus),
            )
            .child(div().size(px(20.)).track_focus(&self.other_focus))
    }
}

#[test]
fn a_native_and_its_focus_handle_share_the_keyboard() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| Focusable {
        host: host(window, cx),
        native_focus: cx.focus_handle(),
        other_focus: cx.focus_handle(),
    });
    draw(&mut cx, window);
    let (host, native_focus, other_focus) = window
        .update(&mut cx, |view, _, _| {
            (
                view.host.clone(),
                view.native_focus.clone(),
                view.other_focus.clone(),
            )
        })
        .unwrap();
    let keyboard = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, _| {
            window
                .platform_window
                .as_test()
                .unwrap()
                .0
                .lock()
                .composition
                .keyboard
        })
        .unwrap()
    };
    assert_eq!(keyboard(&mut cx), None);

    test_host(&host).simulate_focus(true);
    cx.run_until_parked();
    draw(&mut cx, window);
    window
        .update(&mut cx, |_, window, _| {
            assert!(
                native_focus.is_focused(window),
                "the native took the keyboard"
            );
        })
        .unwrap();
    assert_eq!(keyboard(&mut cx), Some(host.id()));

    window
        .update(&mut cx, |_, window, cx| window.focus(&other_focus, cx))
        .unwrap();
    draw(&mut cx, window);
    assert_eq!(keyboard(&mut cx), None, "the keyboard went back to GPUI");

    window
        .update(&mut cx, |_, window, cx| window.focus(&native_focus, cx))
        .unwrap();
    draw(&mut cx, window);
    test_host(&host).simulate_focus(false);
    cx.run_until_parked();
    draw(&mut cx, window);
    window
        .update(&mut cx, |_, window, _| {
            assert!(
                !native_focus.is_focused(window),
                "the user clicked away from the native"
            );
        })
        .unwrap();
    assert_eq!(keyboard(&mut cx), None);

    window
        .update(&mut cx, |_, window, cx| window.focus(&native_focus, cx))
        .unwrap();
    draw(&mut cx, window);
    assert_eq!(
        keyboard(&mut cx),
        Some(host.id()),
        "GPUI gave it the keyboard"
    );
}

/// A random arrangement of natives among other hitboxes: overlapping
/// siblings, children drawn over a native, a clipped native, a faded one and
/// a popover over all of them.
struct Arrangement {
    hosts: Vec<NativeHost>,
    seed: u64,
}

impl Render for Arrangement {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut rng = StdRng::seed_from_u64(self.seed);
        let mut random_box = |rng: &mut StdRng| {
            (
                px(rng.random_range(0.0..400.)),
                px(rng.random_range(0.0..300.)),
                px(rng.random_range(20.0..200.)),
                px(rng.random_range(20.0..200.)),
            )
        };
        let mut root = div().relative().size_full();
        for (ix, host) in self.hosts.iter().enumerate() {
            let (x, y, w, h) = random_box(&mut rng);
            let mut native = native_view(host)
                .absolute()
                .left(x)
                .top(y)
                .w(w)
                .h(h)
                .when(ix % 3 == 1, |this| this.opacity(0.5));
            if rng.random_bool(0.5) {
                let (x, y, w, h) = random_box(&mut rng);
                native = native.child(
                    div()
                        .id(("button", ix))
                        .absolute()
                        .left(x / 4.)
                        .top(y / 4.)
                        .w(w / 4.)
                        .h(h / 4.)
                        .on_click(|_, _, _| {}),
                );
            }
            if ix % 3 == 2 {
                let (x, y, w, h) = random_box(&mut rng);
                root = root.child(
                    div()
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(w)
                        .h(h)
                        .overflow_hidden()
                        .child(native.left(px(-10.)).top(px(-10.))),
                );
            } else {
                root = root.child(native);
            }
            if rng.random_bool(0.5) {
                let (x, y, w, h) = random_box(&mut rng);
                root = root.child(
                    div()
                        .id(("sibling", ix))
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(w)
                        .h(h)
                        .on_mouse_down(crate::MouseButton::Left, |_, _, _| {}),
                );
            }
        }
        let (x, y, w, h) = random_box(&mut rng);
        root.child(deferred(
            anchored()
                .position(point(x, y))
                .child(div().w(w).h(h).occlude()),
        ))
    }
}

#[test]
fn a_native_takes_the_pointer_exactly_where_its_hitbox_is_the_topmost() {
    let mut cx = TestAppContext::single();
    let mut checked = 0;
    let mut hits = 0;
    for seed in 0..40 {
        let window = cx.add_window(|window, cx| Arrangement {
            hosts: (0..5).map(|_| host(window, cx)).collect(),
            seed,
        });
        draw(&mut cx, window);
        cx.update_window(window.into(), |_, window, _| {
            let hit_map = window.native_hit_map().unwrap();
            let placements = &window.rendered_frame.scene.natives().placements;
            let mut rng = StdRng::seed_from_u64(seed + 1000);
            for _ in 0..500 {
                let position = point(
                    px(rng.random_range(0.0..700.)),
                    px(rng.random_range(0.0..600.)),
                );
                let topmost = window
                    .rendered_frame
                    .hit_test(position)
                    .ids
                    .first()
                    .copied();
                let native = topmost.and_then(|topmost| {
                    placements
                        .iter()
                        .find(|placement| placement.hitbox == Some(topmost))
                        .map(|placement| placement.id)
                });
                assert_eq!(
                    hit_map.native_at(position),
                    native,
                    "seed {seed} at {position:?}"
                );
                checked += 1;
                hits += native.is_some() as usize;
            }
        })
        .unwrap();
    }
    assert!(
        hits > checked / 20,
        "only {hits} of {checked} points hit a native"
    );
}

struct FullWindow {
    host: NativeHost,
    overlay: bool,
}

impl Render for FullWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        native_view(&self.host)
            .size_full()
            .when(self.overlay, |this| {
                this.child(div().size(px(10.)).bg(rgb(0xffffff)))
            })
    }
}

#[test]
fn a_native_filling_the_window_with_nothing_over_it_covers_the_window() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| FullWindow {
        host: host(window, cx),
        overlay: false,
    });
    draw(&mut cx, window);
    let covers = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, _| {
            let natives = window.presented_natives().unwrap();
            (natives.any_hole, natives.covers_window)
        })
        .unwrap()
    };
    assert_eq!(covers(&mut cx), (true, true));
    window
        .update(&mut cx, |view, _, cx| {
            view.overlay = true;
            cx.notify();
        })
        .unwrap();
    draw(&mut cx, window);
    assert_eq!(
        covers(&mut cx),
        (true, false),
        "a HUD over it keeps GPUI shown"
    );
}
