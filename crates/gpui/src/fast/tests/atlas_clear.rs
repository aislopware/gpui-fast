//! Tests of emptying the sprite atlases (`fast/atlas_clear.rs`): nothing a
//! frame draws after names a tile of the atlas before.

use crate::{
    AnyWindowHandle, AppContext as _, Context, Image, ImageFormat, IntoElement, ListAlignment,
    ListState, ParentElement as _, Render, ScrollDelta, ScrollEvent, Styled as _, TestAppContext,
    TileId, TouchPhase, Window, div, img, list, point, px, rgb,
};
use std::sync::Arc;

/// A PNG of `side` × `side` pixels of one colour.
fn png(side: u32, red: u8) -> Arc<Image> {
    let pixels = image::RgbaImage::from_pixel(side, side, image::Rgba([red, 0, 0, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    pixels
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("a PNG encodes");
    Arc::new(Image::from_bytes(ImageFormat::Png, bytes.into_inner()))
}

/// Shows each of its images, in a view nothing notifies, so a frame drawn
/// after its first draws it from that frame.
struct Images(Vec<Arc<Image>>);

impl Render for Images {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().children(
            self.0
                .iter()
                .map(|image| img(image.clone()).size(px(8.)).into_any_element()),
        )
    }
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

/// The tiles the window's last frame drew its images from, in order.
fn drawn_tiles(cx: &mut TestAppContext, window: AnyWindowHandle) -> Vec<TileId> {
    cx.update_window(window, |_, window, _| {
        let scene = &window.rendered_frame.scene;
        scene
            .polychrome_sprites
            .iter()
            .map(|sprite| sprite.tile.tile_id)
            .collect()
    })
    .unwrap()
}

/// Whether the window's atlas holds every image's tile.
fn all_in_atlas(cx: &mut TestAppContext, window: AnyWindowHandle, images: &[Arc<Image>]) -> bool {
    cx.update_window(window, |_, window, cx| {
        images.iter().all(|image| {
            let decoded = image
                .clone()
                .get_render_image(window, cx)
                .expect("the image has loaded");
            window.has_image_atlas_entry(&decoded)
        })
    })
    .unwrap()
}

/// After a clear the atlas holds nothing, and the frame after draws every
/// image from a tile inserted anew, though nothing told the view: a frame
/// drawn from the last one would name the tiles a cleared atlas no longer
/// has. A frame drawn without the clear names the same tiles.
#[crate::test]
fn the_frame_after_a_clear_uploads_what_it_draws(cx: &mut TestAppContext) {
    let images = vec![png(4, 10), png(4, 20), png(6, 30)];
    let shown = images.clone();
    let window: AnyWindowHandle = cx.add_window(move |_, _| Images(shown)).into();
    cx.run_until_parked();
    cx.update_window(window, |_, window, _| window.refresh())
        .unwrap();
    draw(cx, window);
    let before = drawn_tiles(cx, window);
    assert_eq!(before.len(), images.len(), "each image is drawn");
    assert!(all_in_atlas(cx, window, &images));

    // Drawn again from the last frame, the same tiles.
    draw(cx, window);
    assert_eq!(drawn_tiles(cx, window), before);

    cx.update(|cx| cx.clear_sprite_atlases());
    assert!(!all_in_atlas(cx, window, &images), "the atlas is empty");
    let dirty = cx
        .update_window(window, |_, window, _| window.invalidator.is_dirty())
        .unwrap();
    assert!(dirty, "a cleared window is drawn again");

    draw(cx, window);
    let after = drawn_tiles(cx, window);
    assert_eq!(after.len(), images.len());
    let newest_before = before.iter().map(|tile| tile.0).max().unwrap();
    assert!(
        after.iter().all(|tile| tile.0 > newest_before),
        "every tile drawn was inserted after the clear: {before:?} then {after:?}"
    );
    assert!(all_in_atlas(cx, window, &images));
}

/// A list that scrolls on a layer, its rows showing images.
struct Rows {
    state: ListState,
    image: Arc<Image>,
}

impl Render for Rows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let image = self.image.clone();
        div().size_full().bg(rgb(0xffffff)).child(
            list(self.state.clone(), move |_, _, _| {
                div()
                    .w(px(200.))
                    .h(px(20.))
                    .child(img(image.clone()).size(px(8.)))
                    .into_any_element()
            })
            .w(px(200.))
            .h(px(100.)),
        )
    }
}

/// Clearing drops a window's scroll layers, whose rows keep the scenes they
/// were painted with, tiles and all.
#[crate::test]
fn a_clear_drops_the_scroll_layers(cx: &mut TestAppContext) {
    if !crate::fast::layers::COMPILED {
        return;
    }
    let image = png(4, 40);
    let state = ListState::new(100, ListAlignment::Top, px(0.)).measure_all();
    let window: AnyWindowHandle = cx.add_window(move |_, _| Rows { state, image }).into();
    cx.run_until_parked();
    draw(cx, window);
    for _ in 0..4 {
        cx.update_window(window, |_, window, cx| {
            window.dispatch_event(
                crate::PlatformInput::Scroll(ScrollEvent {
                    position: point(px(20.), px(20.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-15.))),
                    touch_phase: TouchPhase::Moved,
                    ..Default::default()
                }),
                cx,
            );
        })
        .unwrap();
        draw(cx, window);
    }
    let layers = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| window.fast_layers.layers.len())
            .unwrap()
    };
    assert!(layers(cx) > 0, "the list scrolls on a layer");
    cx.update(|cx| cx.clear_sprite_atlases());
    assert_eq!(layers(cx), 0);
}
