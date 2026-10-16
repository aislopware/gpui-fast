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
            ComposedBatch::Blurs(range) => drawn.extend(
                scene.backdrop_blurs()[range]
                    .iter()
                    .map(|blur| ('b', blur.order)),
            ),
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

struct Banner {
    height: f32,
}

impl Render for Banner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(40.)).h(px(self.height))
    }
}

struct Column {
    banner: Entity<Banner>,
    picture: Entity<Picture>,
}

impl Render for Column {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .child(self.banner.clone())
            .child(self.picture.clone())
    }
}

/// Only the banner above the native is notified: the column around both is
/// spliced, the banner asks for another layout, and the native's own view,
/// never notified, is moved and places its native where it now is.
#[test]
fn a_native_follows_its_view_when_only_a_sibling_view_is_notified() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| {
        let host = host(window, cx);
        Column {
            banner: cx.new(|_| Banner { height: 10. }),
            picture: cx.new(|_| Picture { host }),
        }
    });
    let (banner, host) = window
        .update(&mut cx, |column, _, cx| {
            (column.banner.clone(), column.picture.read(cx).host.clone())
        })
        .unwrap();

    draw(&mut cx, window);
    let test = test_host(&host);
    assert_eq!(test.placement().unwrap().bounds.origin.y, px(10.));

    cx.update(|cx| {
        banner.update(cx, |banner, cx| {
            banner.height = 30.;
            cx.notify();
        })
    });
    draw(&mut cx, window);
    assert!(!test.is_hidden());
    assert_eq!(test.placement().unwrap().bounds.origin.y, px(30.));
    assert_eq!(test.calls(), 2);

    // The column notified with nothing moved: the placement is replayed as is.
    window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
    draw(&mut cx, window);
    assert_eq!(test.placement().unwrap().bounds.origin.y, px(30.));
    assert_eq!(test.calls(), 2);
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

/// A test that lets the test platform draw its windows, as most do, finds
/// each native placed where its view put it, and hidden once it is gone,
/// without presenting frames itself.
#[test]
fn the_test_platform_places_natives_where_its_own_draws_put_them() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| Stack {
        host: host(window, cx),
        popover: false,
    });
    let host = window
        .update(&mut cx, |stack, _, _| stack.host.clone())
        .unwrap();
    window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
    let placement = test_host(&host)
        .placement()
        .expect("placed by the test draw");
    assert_eq!(placement.bounds.origin, point(px(10.), px(10.)));
    assert_eq!(placement.bounds.size, size(px(100.), px(100.)));
    assert!(!test_host(&host).is_hidden());
    let presented = cx
        .update_window(window.into(), |_, window, _| {
            window.presented_natives().cloned()
        })
        .unwrap();
    assert_eq!(presented.unwrap().placement(host.id()), Some(&placement));
}

struct Gallery {
    picture: Option<Entity<Picture>>,
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .child(div().size(px(20.)).child("label"))
            .children(self.picture.clone())
    }
}

/// A native whose view is dropped lets go of its content with the view, not a
/// frame later: what the last frame keeps to draw a view again holds the view
/// weakly, so the platform view (a page and everything it holds open) goes as
/// soon as its owner lets it, whether or not the window draws again.
#[test]
fn a_native_dropped_with_its_view_goes_without_waiting_for_a_frame() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| {
        let host = host(window, cx);
        Gallery {
            picture: Some(cx.new(|_| Picture { host })),
        }
    });
    let platform = window
        .update(&mut cx, |gallery, _, cx| {
            let picture = gallery.picture.as_ref().unwrap().read(cx);
            std::rc::Rc::downgrade(picture.host.platform())
        })
        .unwrap();
    draw(&mut cx, window);
    window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
    draw(&mut cx, window);
    assert!(platform.upgrade().is_some(), "drawn, the native is held");

    window
        .update(&mut cx, |gallery, _, cx| {
            gallery.picture = None;
            cx.notify();
        })
        .unwrap();
    assert!(
        platform.upgrade().is_none(),
        "the last frame holds nothing of a dropped view's native"
    );
    draw(&mut cx, window);
    let placed = cx
        .update_window(window.into(), |_, window, _| {
            window.rendered_frame.scene.composition.placements.len()
        })
        .unwrap();
    assert_eq!(placed, 0, "and the next frame places nothing");
}

/// A web page in a dialog: a native hosted inside a deferred draw, over a
/// page native of the root view and the root view's own controls.
struct Dialog {
    page: NativeHost,
    dialog_page: NativeHost,
}

impl Render for Dialog {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(native_view(&self.page).absolute().size(px(300.)))
            .child(
                div()
                    .id("control")
                    .absolute()
                    .top(px(50.))
                    .left(px(50.))
                    .size(px(200.))
                    .bg(rgb(0x303030))
                    .on_mouse_down(crate::MouseButton::Left, |_, _, _| {}),
            )
            .child(deferred(
                anchored().position(point(px(40.), px(40.))).child(
                    div()
                        .relative()
                        .size(px(220.))
                        .bg(rgb(0x404040))
                        .occlude()
                        .child(native_view(&self.dialog_page).size(px(100.)))
                        .child(
                            div()
                                .id("dialog-control")
                                .absolute()
                                .top(px(0.))
                                .left(px(0.))
                                .size(px(20.))
                                .bg(rgb(0x505050))
                                .on_mouse_down(crate::MouseButton::Left, |_, _, _| {}),
                        ),
                ),
            ))
    }
}

/// gpui-fast #66 moves such a native's surface above its overlay surface;
/// here its placement is painted where the dialog paints it, so it is
/// above all the root view drew under the dialog, below what the dialog
/// draws after it, and takes the pointer over the root view's control.
#[test]
fn a_native_hosted_in_a_deferred_draw_stacks_with_its_dialog() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| Dialog {
        page: host(window, cx),
        dialog_page: host(window, cx),
    });
    draw(&mut cx, window);
    let (page, dialog_page) = window
        .update(&mut cx, |view, _, _| {
            (view.page.id(), view.dialog_page.id())
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, _| {
        let scene = &window.rendered_frame.scene;
        let hole = |id: NativeId| {
            let placement = scene
                .natives()
                .placements
                .iter()
                .find(|placement| placement.id == id)
                .unwrap();
            placement.order
        };
        let quad_order = |color: u32| {
            scene
                .quads
                .iter()
                .find(|quad| quad.background.solid == rgb(color).into())
                .map(|quad| quad.order)
                .unwrap()
        };
        assert!(hole(page) < quad_order(0x303030));
        assert!(quad_order(0x303030) < quad_order(0x404040));
        assert!(quad_order(0x404040) < hole(dialog_page));
        // A primitive of a hole's own order is drawn after the hole.
        assert!(hole(dialog_page) <= quad_order(0x505050));
        let drawn = drawing_order(scene);
        let at = |entry: (char, u32)| drawn.iter().position(|drawn| *drawn == entry).unwrap();
        assert!(
            at(('h', hole(dialog_page))) < at(('q', quad_order(0x505050))),
            "{drawn:?}"
        );
        assert!(
            at(('q', quad_order(0x404040))) < at(('h', hole(dialog_page))),
            "{drawn:?}"
        );

        let hit_map = window.native_hit_map().unwrap();
        assert_eq!(
            hit_map.native_at(point(px(100.), px(100.))),
            Some(dialog_page),
            "the dialog's page over the root view's control"
        );
        assert_eq!(
            hit_map.native_at(point(px(45.), px(45.))),
            None,
            "the dialog's own control over its page"
        );
        assert_eq!(
            hit_map.native_at(point(px(270.), px(10.))),
            Some(page),
            "the root view's page outside the dialog"
        );
    })
    .unwrap();
}

struct Described {
    plain: NativeHost,
    named: NativeHost,
}

impl Render for Described {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("root")
            .role(accesskit::Role::Group)
            .relative()
            .size_full()
            .child(
                native_view(&self.plain)
                    .id("plot")
                    .absolute()
                    .top(px(10.))
                    .left(px(20.))
                    .w(px(100.))
                    .h(px(50.)),
            )
            .child(
                native_view(&self.named)
                    .id("card")
                    .role(accesskit::Role::Document)
                    .aria_label("Page: chart")
                    .absolute()
                    .top(px(100.))
                    .left(px(20.))
                    .w(px(100.))
                    .h(px(50.)),
            )
    }
}

/// A native with an id is a node of the accessibility tree where it is placed,
/// so assistive technologies and tests find it among GPUI's own: a group named
/// for its host, with no role or label of its element's needed; an element's
/// own role and label win.
#[test]
fn a_native_is_a_node_of_the_accessibility_tree_where_it_shows() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| {
        let labelled = |label: &str| NativeHostOptions {
            label: Some(label.to_owned().into()),
            ..NativeHostOptions::default()
        };
        Described {
            plain: window
                .create_native_host(labelled("Page: plot"), cx)
                .expect("the test platform composes natives"),
            named: window
                .create_native_host(labelled("unused"), cx)
                .expect("the test platform composes natives"),
        }
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.set_a11y_active(true);
        window.draw_and_present(cx);
    })
    .unwrap();
    cx.update_window(window.into(), |_, window, _| {
        let scale = f64::from(window.scale_factor());
        let tree = window.a11y_tree().expect("a tree after an active frame");
        let node = |label: &str| {
            tree.nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(label))
                .unwrap_or_else(|| panic!("a node named {label}"))
        };
        let plain = node("Page: plot");
        assert_eq!(plain.role(), accesskit::Role::Group);
        let bounds = plain.bounds().expect("bounds");
        assert_eq!(
            (bounds.x0, bounds.y0, bounds.x1, bounds.y1),
            (20. * scale, 10. * scale, 120. * scale, 60. * scale),
            "where the native shows"
        );
        let named = node("Page: chart");
        assert_eq!(named.role(), accesskit::Role::Document);
        assert!(
            tree.nodes
                .iter()
                .all(|(_, node)| node.label() != Some("unused")),
            "the element's own label wins"
        );
    })
    .unwrap();
}
