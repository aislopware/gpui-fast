//! Image alpha masks (`Window::paint_image_fitted_masked`, carried in the
//! table of edge fades): the mask in device pixels, a mask that moves
//! reusing the image's atlas tile, and frames drawn again from last frame,
//! moved or not, masking as frames drawn from scratch do. See
//! [`crate::fast::image_mask`].

use std::sync::Arc;

use crate::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Corners, EdgeFadeRamps, Entity,
    ImageAlphaMask, IntoElement, LayoutStats, ParentElement as _, Pixels, Render, RenderImage,
    Styled as _, TestAppContext, Window, WindowHandle, canvas, div, point, px, size,
};

fn with_window<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| f(window, cx))
        .unwrap()
}

fn picture() -> Arc<RenderImage> {
    let pixels = image::RgbaImage::from_pixel(32, 24, image::Rgba([40, 120, 200, 255]));
    Arc::new(RenderImage::new([image::Frame::new(pixels)]))
}

/// A hero: an image covering a 300 × 200 box at the top left, fitted into
/// a box 40 pixels taller than it shows, with a composer's space cleared
/// out of it and its lower part faded, as zeron's new-thread background.
struct Hero {
    image: Arc<RenderImage>,
    composer: Bounds<Pixels>,
    feather: f32,
}

impl Render for Hero {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (image, composer, feather) = (self.image.clone(), self.composer, self.feather);
        div().size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let fitted = Bounds::new(
                        bounds.origin - point(px(0.), px(20.)),
                        bounds.size + size(px(0.), px(40.)),
                    );
                    let mask = ImageAlphaMask {
                        bounds: composer,
                        radius: px(120.),
                        feather: px(feather),
                        clearance: px(8.),
                        bottom_fade: Some((bounds.bottom(), bounds.size.height)),
                    };
                    window
                        .paint_image_fitted_masked(
                            bounds,
                            fitted,
                            Corners::default(),
                            image,
                            0,
                            false,
                            Some(mask),
                        )
                        .unwrap();
                },
            )
            .w(px(300.))
            .h(px(200.)),
        )
    }
}

fn composer() -> Bounds<Pixels> {
    Bounds::new(point(px(40.5), px(120.)), size(px(220.), px(60.)))
}

/// The ramps the window's only image sprite is drawn in, and its tile.
fn image_ramps(window: &Window) -> (Option<EdgeFadeRamps>, crate::AtlasTile) {
    let scene = &window.rendered_frame.scene;
    assert_eq!(scene.polychrome_sprites.len(), 1);
    let sprite = &scene.polychrome_sprites[0];
    (scene.edge_fade_ramps(sprite.pad), sprite.tile)
}

/// The mask reaches the shader in device pixels, its radius clamped to
/// half the excluded rectangle's shorter side; moving it changes only the
/// entry the sprite indexes, not its tile; a mask of no feather masks
/// nothing.
#[test]
fn a_mask_is_drawn_in_device_pixels_and_moves_without_the_image() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, _| Hero {
        image: picture(),
        composer: composer(),
        feather: 100.,
    });
    cx.test_window(window.into())
        .simulate_scale_factor_change(2.);
    let (ramps, tile) = with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        image_ramps(window)
    });
    let ramps = ramps.expect("the image is masked");
    assert!(ramps.masks() && !ramps.clips());
    assert!(ramps.rate.iter().all(|rate| *rate == 0.));
    assert_eq!(ramps.mask, [81., 240., 521., 360.]);
    assert_eq!(ramps.mask_shape, [60., 200., 16., 400.]);
    assert_eq!(ramps.mask_bottom, [400., 0., 0., 0.]);
    let at = |x: f32, y: f32| ramps.mask_at(point(crate::ScaledPixels(x), crate::ScaledPixels(y)));
    assert_eq!(at(300., 300.), 0., "the composer's space is cleared");
    assert!(at(300., 30.) > 0.9, "far above it, the image shows");

    window
        .update(&mut cx, |hero, _, cx| {
            hero.composer.origin.y -= px(30.);
            cx.notify();
        })
        .unwrap();
    let (moved, moved_tile) = with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        image_ramps(window)
    });
    let moved = moved.expect("still masked");
    assert_eq!(moved.mask, [81., 180., 521., 300.]);
    assert_eq!(moved_tile, tile, "the image's tile is reused");

    window
        .update(&mut cx, |hero, _, cx| {
            hero.feather = 0.;
            cx.notify();
        })
        .unwrap();
    let (none, _) = with_window(&mut cx, window.into(), |window, cx| {
        window.draw(cx).clear(cx);
        image_ramps(window)
    });
    assert_eq!(none, None, "no feather, no mask");
}

/// A hero moving down a window, its composer moving, with a nested view
/// notified alone beside it.
struct Sliding {
    spacer: f32,
    hero: Entity<Hero>,
    nested: Entity<Nested>,
}

struct Nested {
    count: usize,
}

impl Render for Nested {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(20.))
            .w(px(20. + self.count as f32))
            .bg(crate::hsla(0.4, 0.5, 0.5, 1.))
    }
}

impl Render for Sliding {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_none().h(px(self.spacer)))
            .child(self.nested.clone())
            .child(self.hero.clone())
    }
}

/// Drawn incrementally, the hero masks as a window drawn from scratch does,
/// while it moves and its mask moves.
#[test]
fn a_mask_drawn_again_masks_as_drawn_afresh() {
    let mut cx = TestAppContext::single();
    let image = picture();
    let new = |cx: &mut TestAppContext| {
        let image = image.clone();
        cx.add_window(move |_, cx| Sliding {
            spacer: 0.,
            hero: cx.new(|_| Hero {
                image,
                composer: composer(),
                feather: 80.,
            }),
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
    for frame in 0..24 {
        for window in windows {
            window
                .update(&mut cx, |sliding, _, cx| match frame % 3 {
                    0 => {
                        sliding.spacer += 0.5;
                        cx.notify();
                    }
                    1 => sliding.hero.update(cx, |hero, cx| {
                        hero.composer.origin.y += px(3.);
                        cx.notify();
                    }),
                    _ => sliding.nested.update(cx, |nested, cx| {
                        nested.count += 1;
                        cx.notify();
                    }),
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
            actual.iter().any(|line| line.contains("mask_shape: [60.0")),
            "frame {frame} masks"
        );
        stats.views_reused += frame_stats.views_reused;
    }
    assert!(stats.views_reused > 0, "{stats:?}");
}
