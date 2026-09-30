//! Tests of cursors with the application's own picture. See
//! [`crate::fast::cursor`].

use crate::{
    Context, CursorImage, CursorImageId, CursorStyle, DevicePixels, InteractiveElement as _,
    IntoElement, Modifiers, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, point, px, size,
};
use std::{cell::Cell, rc::Rc};

const TILE: CursorImageId = CursorImageId(42);

/// A remote-desktop tile of 100 × 100 at the window's top left whose pointer
/// is the far side's picture, counting its renders.
struct Tile {
    renders: Rc<Cell<usize>>,
}

impl Render for Tile {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full().child(
            div()
                .id("tile")
                .w(px(100.))
                .h(px(100.))
                .cursor(CursorStyle::Image(TILE)),
        )
    }
}

fn tile(cx: &mut TestAppContext) -> (Rc<Cell<usize>>, &mut VisualTestContext) {
    let renders = Rc::new(Cell::new(0));
    let (_, cx) = cx.add_window_view({
        let renders = renders.clone();
        |_, _| Tile { renders }
    });
    cx.update(|window, _| {
        window.activate_window();
        // Linux and Windows set the cursor only in the window the platform
        // reports hovered, which the test window never does.
        window.hovered.set(true);
    });
    cx.run_until_parked();
    (renders, cx)
}

fn cursor(cx: &VisualTestContext) -> CursorStyle {
    *cx.test_platform.active_cursor.lock()
}

fn picture(bgra: [u8; 4], hot: (i32, i32)) -> CursorImage {
    CursorImage::new(
        bgra.repeat(16 * 16),
        size(DevicePixels(16), DevicePixels(16)),
        point(DevicePixels(hot.0), DevicePixels(hot.1)),
        2.,
    )
    .unwrap()
}

#[crate::test]
fn the_pointer_takes_the_picture_as_it_enters_and_moving_over_it_draws_nothing(
    cx: &mut TestAppContext,
) {
    let (renders, cx) = tile(cx);
    cx.simulate_mouse_move(point(px(150.), px(150.)), None, Modifiers::default());
    assert_eq!(cursor(cx), CursorStyle::Arrow);
    let drawn = renders.get();

    cx.simulate_mouse_move(point(px(50.), px(50.)), None, Modifiers::default());
    assert_eq!(
        cursor(cx),
        CursorStyle::Image(TILE),
        "set as the move was handled"
    );
    for step in 0..20 {
        let at = px(10. + step as f32 * 4.);
        cx.simulate_mouse_move(point(at, at), None, Modifiers::default());
    }
    assert_eq!(cursor(cx), CursorStyle::Image(TILE));
    assert_eq!(
        renders.get(),
        drawn,
        "the pointer entered and moved over the tile without the tile being built"
    );

    cx.simulate_mouse_move(point(px(150.), px(50.)), None, Modifiers::default());
    assert_eq!(cursor(cx), CursorStyle::Arrow, "the arrow again outside");
}

#[crate::test]
fn an_id_is_pointed_at_pictures_and_forgotten_without_a_frame(cx: &mut TestAppContext) {
    let (renders, cx) = tile(cx);
    cx.simulate_mouse_move(point(px(50.), px(50.)), None, Modifiers::default());
    let drawn = renders.get();
    let (arrow, beam) = (picture([0, 0, 0, 255], (0, 0)), picture([255; 4], (7, 8)));

    cx.update(|_, cx| cx.set_cursor_image(TILE, Some(arrow.clone())));
    cx.update(|_, cx| cx.set_cursor_image(TILE, Some(beam.clone())));
    {
        let images = cx.test_platform.fast_cursor_images.borrow();
        assert_eq!(
            images.images.get(&TILE),
            Some(&beam),
            "the last picture holds"
        );
        assert_eq!(images.sets, 2);
    }
    cx.update(|_, cx| cx.set_cursor_image(TILE, None));
    assert!(
        !cx.test_platform
            .fast_cursor_images
            .borrow()
            .images
            .contains_key(&TILE),
        "forgotten"
    );
    cx.run_until_parked();
    assert_eq!(renders.get(), drawn, "a picture changes without a frame");
    assert_eq!(
        cursor(cx),
        CursorStyle::Image(TILE),
        "the style never moved"
    );
}

#[test]
fn a_picture_is_refused_unless_its_pixels_fill_it_and_the_hotspot_is_on_it() {
    let square = |bytes: usize, side: i32, hot: (i32, i32), scale: f32| {
        CursorImage::new(
            vec![0; bytes],
            size(DevicePixels(side), DevicePixels(side)),
            point(DevicePixels(hot.0), DevicePixels(hot.1)),
            scale,
        )
    };
    assert!(square(16, 2, (1, 1), 1.).is_ok());
    assert!(square(15, 2, (1, 1), 1.).is_err(), "a byte short");
    assert!(square(20, 2, (1, 1), 1.).is_err(), "a pixel over");
    assert!(square(0, 0, (0, 0), 1.).is_err(), "empty");
    assert!(
        square(16, 2, (2, 1), 1.).is_err(),
        "the hotspot off the right edge"
    );
    assert!(square(16, 2, (1, -1), 1.).is_err(), "the hotspot above");
    assert!(square(16, 2, (1, 1), 0.).is_err(), "no scale");
    assert!(square(16, 2, (1, 1), f32::NAN).is_err(), "not a number");
    let side = crate::CURSOR_IMAGE_MAX_SIDE + 1;
    assert!(square(side as usize * side as usize * 4, side, (0, 0), 1.).is_err());
}

#[test]
fn a_pictures_key_stands_for_its_pixels_hotspot_and_scale() {
    let a = picture([1, 2, 3, 255], (3, 4));
    assert_eq!(a.key(), picture([1, 2, 3, 255], (3, 4)).key());
    assert_ne!(a.key(), picture([1, 2, 4, 255], (3, 4)).key(), "a pixel");
    assert_ne!(
        a.key(),
        picture([1, 2, 3, 255], (4, 3)).key(),
        "the hotspot"
    );
    let at_1x = CursorImage::new(a.bgra().to_vec(), a.size(), a.hotspot(), 1.).unwrap();
    assert_ne!(a.key(), at_1x.key(), "the scale");
}

#[test]
fn a_picture_is_shown_at_its_size_in_points_with_a_hotspot_between_points() {
    let beam = picture([0; 4], (7, 8));
    assert_eq!(beam.size_in_points(), size(8., 8.), "16 pixels at 2×");
    assert_eq!(
        beam.hotspot_in_points(),
        point(3.5, 4.),
        "pixel 7 lies between points"
    );
}
