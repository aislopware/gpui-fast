//! Backdrop blurs over natives in the presented frame
//! ([`NativeFrame::blurs`]): a blur over a native's hole is handed to the
//! platform, which blurs the native under GPUI's layer there; one over GPUI's
//! content alone is not; a frame that changes no blur, drawn afresh or from
//! the last frame, changes nothing on the platform. See
//! [`crate::fast::composition::NativeFrameBlur`].

use crate::{
    AnyElement, AppContext as _, BackdropBlur, Bounds, ContentMask, Context, Corners, Entity,
    IntoElement, ParentElement as _, Render, ScaledPixels, Scene, Styled as _, TestAppContext,
    Window, WindowHandle, canvas,
    composition::{
        NativeFrame, NativeFrameBlur, NativeHost, NativeHostOptions, NativeId, NativePlacement,
        native_view,
    },
    div, hsla, point,
    prelude::FluentBuilder as _,
    px, size,
};

fn scaled(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x), ScaledPixels(y)),
        size(ScaledPixels(w), ScaledPixels(h)),
    )
}

fn everywhere() -> ContentMask<ScaledPixels> {
    ContentMask {
        bounds: scaled(0., 0., 1e4, 1e4),
    }
}

fn native(id: u64, bounds: Bounds<ScaledPixels>) -> NativePlacement {
    NativePlacement {
        order: 0,
        id: NativeId(id),
        hitbox: None,
        focus: None,
        bounds,
        content_mask: everywhere(),
        corner_radii: Corners::default(),
        opacity: 1.,
    }
}

fn blur(bounds: Bounds<ScaledPixels>, corner: f32, radius: f32) -> BackdropBlur {
    BackdropBlur {
        order: 0,
        fade: 0,
        bounds,
        content_mask: everywhere(),
        corner_radii: Corners::all(ScaledPixels(corner)),
        blur_radius: ScaledPixels(radius),
        opacity: 1.,
        quads_before: 0,
    }
}

/// A blur is handed to the platform when it lies over a native drawn under
/// it, in logical pixels, stacked above the natives drawn under it; its
/// corners are clamped as a quad's and square where its content mask cut
/// them off. A blur over GPUI's content alone, or under every native it
/// overlaps, is not handed over.
#[test]
fn a_blur_over_a_native_is_placed_in_the_presented_frame() {
    let mut scene = Scene::default();
    let background = crate::Quad {
        bounds: scaled(0., 0., 1000., 1000.),
        content_mask: everywhere(),
        background: hsla(0., 0., 0.1, 1.).into(),
        ..crate::Quad::default()
    };
    scene.insert_primitive(background);
    // Native A, a frosted menu over it, native B over the menu, a frosted
    // popover over B, a badge over GPUI alone, and a blur clipped by its
    // content mask over A.
    scene.insert_native(native(0, scaled(0., 0., 400., 300.)));
    scene.insert_backdrop_blur(blur(scaled(100., 100., 100., 60.), 12., 24.));
    scene.insert_native(native(1, scaled(150., 120., 200., 100.)));
    scene.insert_backdrop_blur(blur(scaled(300., 200., 80., 60.), 12., 16.));
    scene.insert_backdrop_blur(blur(scaled(600., 600., 40., 40.), 8., 8.));
    scene.insert_backdrop_blur(BackdropBlur {
        content_mask: ContentMask {
            bounds: scaled(0., 0., 160., 1000.),
        },
        ..blur(scaled(100., 240., 100., 40.), 30., 2.)
    });
    scene.finish();

    let frame = NativeFrame::new(&scene, 2., size(px(500.), px(500.)));
    assert_eq!(frame.blurs.len(), 3, "{:#?}", frame.blurs);
    let at = |x: f32, y: f32| {
        frame
            .blurs
            .iter()
            .find(|blur| blur.bounds.origin == point(px(x), px(y)))
            .unwrap_or_else(|| panic!("a blur at {x}, {y}: {:#?}", frame.blurs))
    };
    assert_eq!(
        *at(50., 50.),
        NativeFrameBlur {
            bounds: Bounds::new(point(px(50.), px(50.)), size(px(50.), px(30.))),
            corner_radii: Corners::all(px(6.)),
            blur_radius: px(12.),
            opacity: 1.,
            natives_below: 1,
        },
        "the menu goes over A and under B"
    );
    assert_eq!(at(150., 100.).natives_below, 2, "the popover goes over B");
    assert_eq!(at(150., 100.).blur_radius, px(8.));
    let clipped = at(50., 120.);
    assert_eq!(
        clipped.bounds.size,
        size(px(30.), px(20.)),
        "what the content mask leaves"
    );
    assert_eq!(
        clipped.corner_radii,
        Corners {
            top_left: px(10.),
            top_right: px(0.),
            bottom_right: px(0.),
            bottom_left: px(10.),
        },
        "clamped to half the region's height, square where the mask cut"
    );
    assert!(
        frame
            .blurs
            .iter()
            .all(|blur| blur.bounds.origin.x < px(300.)),
        "the badge over GPUI alone is not handed over"
    );

    // A blur under the only native it overlaps is not handed over either.
    let mut scene = Scene::default();
    scene.insert_backdrop_blur(blur(scaled(10., 10., 100., 100.), 0., 8.));
    scene.insert_native(native(0, scaled(0., 0., 200., 200.)));
    scene.finish();
    let frame = NativeFrame::new(&scene, 2., size(px(500.), px(500.)));
    assert!(frame.blurs.is_empty(), "{:#?}", frame.blurs);
    assert!(
        frame.changes_since(&frame.clone()).is_empty(),
        "a frame is no change from itself"
    );
}

/// What is under it blurred by `radius`, with a translucent fill over that.
fn frosted(radius: f32) -> AnyElement {
    div()
        .relative()
        .size_full()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    window.paint_backdrop_blur(bounds, Corners::all(px(6.)), px(radius))
                },
            )
            .absolute()
            .size_full(),
        )
        .child(div().size_full().rounded(px(6.)).bg(hsla(0., 0., 1., 0.3)))
        .into_any_element()
}

/// A frosted menu, 80 × 50; clipped, only its left 50 show.
struct Menu {
    clip: bool,
}

impl Render for Menu {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(if self.clip { 50. } else { 80. }))
            .h(px(50.))
            .overflow_hidden()
            .child(div().flex_none().w(px(80.)).h(px(50.)).child(frosted(12.)))
    }
}

/// A native page at 10, 10, 200 × 150; the menu over it at `menu_left`, 40;
/// and a badge frosted over GPUI's content alone, beside the page.
struct Page {
    host: NativeHost,
    menu: Entity<Menu>,
    menu_left: f32,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(hsla(0., 0., 0.1, 1.))
            .child(
                native_view(&self.host)
                    .absolute()
                    .left(px(10.))
                    .top(px(10.))
                    .w(px(200.))
                    .h(px(150.)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(300.))
                    .top(px(10.))
                    .size(px(60.))
                    .child(frosted(4.)),
            )
            .when(self.menu_left >= 0., |this| {
                this.child(
                    div()
                        .absolute()
                        .left(px(self.menu_left))
                        .top(px(40.))
                        .child(self.menu.clone()),
                )
            })
    }
}

fn draw<V: 'static>(cx: &mut TestAppContext, window: WindowHandle<V>) {
    cx.update_window(window.into(), |_, window, cx| window.draw_and_present(cx))
        .unwrap();
}

/// The blurs over natives the test platform shows, how many presents changed
/// them, and how many presents changed anything.
fn platform<V: 'static>(
    cx: &mut TestAppContext,
    window: WindowHandle<V>,
) -> (Vec<NativeFrameBlur>, usize, usize) {
    cx.update_window(window.into(), |_, window, _| {
        let mut window = window.platform_window.as_test().unwrap().0.lock();
        let composition = &mut window.composition;
        (
            composition.blurs.clone(),
            composition.blur_updates,
            composition.transactional_presents,
        )
    })
    .unwrap()
}

/// In a window: the menu's blur over the page reaches the platform; a frame
/// drawn again with the menu replayed from the last frame changes nothing;
/// moving it, clipping it and taking it off the page each change the blurs
/// once.
#[test]
fn a_blur_over_a_native_reaches_the_platform_and_a_replayed_one_costs_nothing() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| Page {
        host: window
            .create_native_host(NativeHostOptions::default(), cx)
            .expect("the test platform composes natives"),
        menu: cx.new(|_| Menu { clip: false }),
        menu_left: 150.,
    });
    draw(&mut cx, window);
    let menu = NativeFrameBlur {
        bounds: Bounds::new(point(px(150.), px(40.)), size(px(80.), px(50.))),
        corner_radii: Corners::all(px(6.)),
        blur_radius: px(12.),
        opacity: 1.,
        natives_below: 1,
    };
    let (blurs, updates, transactional) = platform(&mut cx, window);
    assert_eq!(blurs, vec![menu.clone()], "the badge is not handed over");
    assert!(updates >= 1);
    let presented = cx
        .update_window(window.into(), |_, window, _| {
            window.presented_natives().unwrap().blurs.clone()
        })
        .unwrap();
    assert_eq!(presented, blurs);

    // The page drawn again, the menu's view replayed from the last frame.
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
    assert_eq!(reused, retaining, "the menu was drawn from the last frame");
    assert_eq!(
        platform(&mut cx, window),
        (vec![menu.clone()], updates, transactional),
        "a frame changing no blur changes nothing on the platform"
    );

    window
        .update(&mut cx, |page, _, cx| {
            page.menu_left = 160.;
            cx.notify();
        })
        .unwrap();
    draw(&mut cx, window);
    let moved = NativeFrameBlur {
        bounds: Bounds::new(point(px(160.), px(40.)), size(px(80.), px(50.))),
        ..menu
    };
    assert_eq!(
        platform(&mut cx, window),
        (vec![moved], updates + 1, transactional + 1),
        "moving the menu moves its blur in one present"
    );

    window
        .update(&mut cx, |page, _, cx| {
            page.menu.update(cx, |menu, cx| {
                menu.clip = true;
                cx.notify();
            })
        })
        .unwrap();
    draw(&mut cx, window);
    let (blurs, clipped_updates, _) = platform(&mut cx, window);
    assert_eq!(clipped_updates, updates + 2);
    assert_eq!(
        blurs,
        vec![NativeFrameBlur {
            bounds: Bounds::new(point(px(160.), px(40.)), size(px(50.), px(50.))),
            corner_radii: Corners {
                top_left: px(6.),
                top_right: px(0.),
                bottom_right: px(0.),
                bottom_left: px(6.),
            },
            ..menu
        }],
        "a clipped menu's blur is square where it was cut"
    );

    window
        .update(&mut cx, |page, _, cx| {
            page.menu_left = 250.;
            cx.notify();
        })
        .unwrap();
    draw(&mut cx, window);
    let (blurs, off_updates, _) = platform(&mut cx, window);
    assert!(blurs.is_empty(), "off the page: {blurs:#?}");
    assert_eq!(off_updates, updates + 3);

    window
        .update(&mut cx, |page, _, cx| {
            page.menu_left = -1.;
            cx.notify();
        })
        .unwrap();
    draw(&mut cx, window);
    assert_eq!(
        platform(&mut cx, window).1,
        updates + 3,
        "no blur over a native, before and after: no change"
    );
}
