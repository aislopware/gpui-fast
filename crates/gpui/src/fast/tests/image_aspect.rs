//! An image's intrinsic aspect ratio sizes `img` only when a dimension is
//! auto. See [`crate::fast::image_aspect`].

use std::sync::Arc;

use crate::{
    AnyElement, ImageSource, IntoElement as _, ObjectFit, ParentElement as _, Pixels, RenderImage,
    Size, Styled as _, StyledImage as _, TestAppContext, div, img, point, px, size,
};

fn picture(width: u32, height: u32) -> Arc<RenderImage> {
    let pixels = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
    Arc::new(RenderImage::new([image::Frame::new(pixels)]))
}

/// Draws `build` and returns the size of the last image sprite, in
/// logical pixels.
fn sprite_size(cx: &mut TestAppContext, build: impl Fn() -> AnyElement) -> Size<Pixels> {
    let window = cx.add_empty_window();
    window.draw(point(px(0.), px(0.)), size(px(600.), px(600.)), |_, _| {
        build()
    });
    window.update(|window, _| {
        let sprite = window
            .rendered_frame
            .scene
            .polychrome_sprites
            .last()
            .expect("the image should paint a sprite");
        let scale = window.scale_factor();
        size(
            px(sprite.bounds.size.width.0 / scale),
            px(sprite.bounds.size.height.0 / scale),
        )
    })
}

#[crate::test]
fn explicit_dimensions_win_over_the_intrinsic_ratio(cx: &mut TestAppContext) {
    let full_width_fixed_height = sprite_size(cx, || {
        div()
            .w(px(100.))
            .child(
                img(ImageSource::Render(picture(100, 400)))
                    .w_full()
                    .h(px(60.))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element()
    });
    assert_eq!(full_width_fixed_height, size(px(100.), px(60.)));

    let full_size_in_a_stretched_flex_item = sprite_size(cx, || {
        div()
            .flex()
            .h(px(60.))
            .child(
                div().w(px(100.)).child(
                    img(ImageSource::Render(picture(100, 400)))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                ),
            )
            .into_any_element()
    });
    assert_eq!(full_size_in_a_stretched_flex_item, size(px(100.), px(60.)));
}

/// Over a grid of image shapes and boxes: both dimensions given, the
/// element takes them; one left auto, the ratio fills it in (to within a
/// pixel, as layout may round).
#[crate::test]
fn the_ratio_fills_in_only_an_auto_dimension(cx: &mut TestAppContext) {
    let shapes = [(100, 400), (400, 100), (64, 64), (300, 200)];
    let boxes = [(100., 60.), (60., 100.), (200., 200.), (37., 250.)];
    for (image_width, image_height) in shapes {
        let ratio = image_width as f32 / image_height as f32;
        for (width, height) in boxes {
            let both = sprite_size(cx, || {
                img(ImageSource::Render(picture(image_width, image_height)))
                    .w(px(width))
                    .h(px(height))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element()
            });
            assert_eq!(
                both,
                size(px(width), px(height)),
                "{image_width}x{image_height} image in a {width}x{height} box"
            );

            let auto_height = sprite_size(cx, || {
                img(ImageSource::Render(picture(image_width, image_height)))
                    .w(px(width))
                    .object_fit(ObjectFit::Fill)
                    .into_any_element()
            });
            assert!(
                (auto_height.width.0 - width).abs() < 1.
                    && (auto_height.height.0 - width / ratio).abs() < 1.,
                "{image_width}x{image_height} image {width} wide laid out at {auto_height:?}"
            );
        }
    }
}
