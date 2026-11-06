//! Tests of freeing an image's atlas tiles with its asset
//! (`fast/image_evict.rs`).

use crate::{
    AppContext as _, Context, Image, ImageFormat, ImageSource, IntoElement, ParentElement as _,
    Render, RenderImage, Styled as _, TestAppContext, Window, WindowHandle, div, img, px,
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

/// Shows each of its images.
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

/// A window showing `images`, drawn once they have loaded.
fn shown(cx: &mut TestAppContext, images: Vec<Arc<Image>>) -> WindowHandle<Images> {
    let window = cx.add_window(move |_, _| Images(images));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    window
}

/// The decoded image of `image`, loaded already.
fn decoded(
    cx: &mut TestAppContext,
    window: WindowHandle<Images>,
    image: &Arc<Image>,
) -> Arc<RenderImage> {
    window
        .update(cx, |_, window, cx| {
            image.clone().get_render_image(window, cx)
        })
        .unwrap()
        .expect("the image has loaded")
}

fn in_atlas(cx: &mut TestAppContext, window: WindowHandle<Images>, image: &RenderImage) -> bool {
    cx.update_window(window.into(), |_, window, _| {
        window.has_image_atlas_entry(image)
    })
    .unwrap()
}

/// Removing an image's asset leaves its tiles in the atlas, where nothing
/// can reach them; evicting it frees them, from outside the window's update
/// and from inside it, and `peek_asset` reads a finished load without
/// starting one.
#[crate::test]
fn evicting_an_image_frees_its_atlas_tiles(cx: &mut TestAppContext) {
    let (removed, evicted, evicted_inside) = (png(4, 10), png(4, 20), png(4, 30));
    let window = shown(
        cx,
        vec![removed.clone(), evicted.clone(), evicted_inside.clone()],
    );
    let decoded_images: Vec<Arc<RenderImage>> = [&removed, &evicted, &evicted_inside]
        .into_iter()
        .map(|image| decoded(cx, window, image))
        .collect();
    for image in &decoded_images {
        assert!(in_atlas(cx, window, image), "each image is in the atlas");
    }

    // The leak: the cache forgets the image, the atlas keeps it.
    cx.update(|cx| ImageSource::Image(removed.clone()).remove_asset(cx));
    assert!(!cx.update(|cx| removed.is_asset_cached(cx)));
    assert!(in_atlas(cx, window, &decoded_images[0]));

    // `peek_asset` sees the finished load, and starts none.
    let source = ImageSource::Image(evicted.clone());
    cx.update(|cx| {
        let peeked = cx
            .peek_asset::<crate::AssetLogger<crate::ImageDecoder>>(&evicted)
            .and_then(Result::ok)
            .expect("a finished load");
        assert!(Arc::ptr_eq(&peeked, &decoded_images[1]));
        assert!(
            cx.peek_asset::<crate::ImgResourceLoader>(&crate::Resource::Embedded(
                "never-loaded.png".into()
            ))
            .is_none()
        );
        assert!(
            !cx.has_asset::<crate::ImgResourceLoader>(&crate::Resource::Embedded(
                "never-loaded.png".into()
            ))
        );
    });

    cx.update(|cx| source.evict(None, cx));
    assert!(!cx.update(|cx| evicted.is_asset_cached(cx)));
    assert!(!in_atlas(cx, window, &decoded_images[1]));

    cx.update_window(window.into(), |_, window, cx| {
        ImageSource::Image(evicted_inside.clone()).evict(Some(window), cx)
    })
    .unwrap();
    assert!(!cx.update(|cx| evicted_inside.is_asset_cached(cx)));
    assert!(!in_atlas(cx, window, &decoded_images[2]));
    // The one only removed is still there.
    assert!(in_atlas(cx, window, &decoded_images[0]));
}
