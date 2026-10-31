//! A test window drawn by the Mac's own renderer and text system
//! (`gpui::fast::test_pixels`): what an app's visual test renders in its test
//! process, read back as the app's window would be.

use std::{borrow::Cow, ops::Range, sync::Arc};

use gpui::{
    AssetSource, Context, IntoElement, ParentElement as _, PlatformHeadlessRenderer, Render,
    SharedString, Styled as _, TestAppContext, TestDispatcher, VisualTestContext, Window,
    WindowBackgroundAppearance, div, px, rgb, rgba, size, svg,
};

use image::RgbaImage;

use crate::{metal_renderer::MetalHeadlessRenderer, text_system};

/// The renderer `gpui_platform::current_headless_renderer` makes on macOS.
fn current_headless_renderer() -> anyhow::Result<Option<Box<dyn PlatformHeadlessRenderer>>> {
    Ok(Some(Box::new(MetalHeadlessRenderer::new()?)))
}

/// One square icon, filled whole.
const ICON: &str = "icons/square.svg";
const SQUARE: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="black"/></svg>"#;

struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok((path == ICON).then_some(Cow::Borrowed(SQUARE)))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(vec![ICON.into()])
    }
}

/// A red block, a word and the icon on white, at fixed places, 120 × 40.
struct Card {
    ground: gpui::Rgba,
}

impl Render for Card {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(self.ground)
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .size(px(20.))
                    .bg(rgb(0xff0000)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(30.))
                    .top(px(4.))
                    .text_size(px(24.))
                    .text_color(rgb(0x000000))
                    .child("WM"),
            )
            .child(
                svg()
                    .path(ICON)
                    .absolute()
                    .left(px(96.))
                    .top(px(0.))
                    .size(px(16.))
                    .text_color(rgb(0x0000ff)),
            )
    }
}

/// A seeded test context whose windows draw through this platform's renderer
/// with its text system and [`Assets`], or `None` with no GPU to draw on.
fn drawing_context() -> Option<TestAppContext> {
    if !matches!(current_headless_renderer(), Ok(Some(_))) {
        eprintln!("skipped: no headless renderer on this machine");
        return None;
    }
    Some(TestAppContext::build_with_renderer(
        TestDispatcher::new(0),
        None,
        text_system(),
        Arc::new(Assets),
        current_headless_renderer,
    ))
}

/// A window of `Card` at 120 × 40 points and 1 device pixel to the point.
fn card(cx: &mut TestAppContext, ground: gpui::Rgba) -> &mut VisualTestContext {
    let (_card, cx) = cx.add_window_view(|_, _| Card { ground });
    cx.simulate_scale_factor_change(1.0);
    cx.simulate_resize(size(px(120.), px(40.)));
    cx.run_until_parked();
    cx
}

fn render(cx: &mut VisualTestContext) -> RgbaImage {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.render_to_image().expect("the window renders")
    })
}

/// Pixels of `image` in `xs`, `ys` (device pixels) whose colour is within 8 of
/// `rgb` on every channel.
fn count(image: &RgbaImage, xs: Range<u32>, ys: Range<u32>, rgb: [u8; 3]) -> usize {
    ys.flat_map(|y| xs.clone().map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let pixel = image.get_pixel(x, y).0;
            pixel.iter().zip(rgb).all(|(a, b)| a.abs_diff(b) <= 8)
        })
        .count()
}

/// The window draws what it paints, at its size: the block in its place, the
/// word in the text system's glyphs, the icon from the app's assets in its
/// colour, the rest the ground.
#[test]
fn a_test_window_draws_its_text_and_icons_in_real_pixels() {
    let Some(mut cx) = drawing_context() else {
        return;
    };
    let window = card(&mut cx, rgb(0xffffff));
    let image = render(window);
    assert_eq!(image.dimensions(), (120, 40));
    assert_eq!(
        count(&image, 0..20, 0..20, [255, 0, 0]),
        400,
        "the block, whole"
    );
    assert_eq!(
        image.get_pixel(25, 30).0,
        [255, 255, 255, 255],
        "the ground, opaque"
    );
    let ink = count(&image, 30..90, 0..40, [0, 0, 0]);
    assert!(ink > 60, "the word's glyphs are drawn: {ink} pixels of ink");
    assert_eq!(
        count(&image, 96..112, 0..16, [0, 0, 255]),
        256,
        "the icon, in its colour"
    );
    cx.quit();
}

/// A window whose background is not opaque clears to nothing, so a picture
/// laid over a backdrop shows the backdrop wherever the window paints nothing;
/// made opaque again, it clears to opaque.
#[test]
fn a_transparent_window_lets_through_what_it_does_not_paint() {
    let Some(mut cx) = drawing_context() else {
        return;
    };
    let window = card(&mut cx, rgba(0x00000000));
    let opaque = render(window);
    assert_eq!(
        opaque.get_pixel(25, 30).0[3],
        255,
        "opaque until it is told otherwise"
    );

    for background in [
        WindowBackgroundAppearance::Transparent,
        WindowBackgroundAppearance::Blurred,
    ] {
        window.update(|window, _| window.set_background_appearance(background));
        let clear = render(window);
        assert_eq!(
            clear.get_pixel(25, 30).0,
            [0, 0, 0, 0],
            "{background:?}: nothing painted"
        );
        assert_eq!(
            clear.get_pixel(5, 5).0,
            [255, 0, 0, 255],
            "{background:?}: the block"
        );
    }

    window.update(|window, _| window.set_background_appearance(WindowBackgroundAppearance::Opaque));
    assert_eq!(render(window).get_pixel(25, 30).0[3], 255, "opaque again");
    cx.quit();
}

/// A window on a 1x display renders at 2x on request, twice the pixels with
/// everything in its place, and goes on drawing at its own scale after.
#[test]
fn a_test_window_renders_at_another_scale() {
    let Some(mut cx) = drawing_context() else {
        return;
    };
    let window = card(&mut cx, rgb(0xffffff));
    let retina = window
        .update(|window, cx| window.render_to_image_at(2.0, cx))
        .expect("the window renders at 2x");
    assert_eq!(retina.dimensions(), (240, 80));
    assert_eq!(
        count(&retina, 0..40, 0..40, [255, 0, 0]),
        1600,
        "the block, at 2x"
    );
    assert_eq!(
        count(&retina, 192..224, 0..32, [0, 0, 255]),
        1024,
        "the icon, at 2x"
    );
    let own = render(window);
    assert_eq!(own.dimensions(), (120, 40), "back at its own scale");
    assert_eq!(count(&own, 0..20, 0..20, [255, 0, 0]), 400);
    cx.quit();
}

/// A context without a renderer has nothing to draw with, and says so rather
/// than giving a blank picture.
#[test]
fn a_window_without_a_renderer_says_it_cannot_render() {
    let mut cx = TestAppContext::build_with_renderer(
        TestDispatcher::new(0),
        None,
        text_system(),
        Arc::new(Assets),
        || Ok(None),
    );
    let window = card(&mut cx, rgb(0xffffff));
    let error = window
        .update(|window, cx| window.render_to_image_at(2.0, cx))
        .expect_err("nothing to render with");
    assert!(
        error.to_string().contains("no headless renderer"),
        "{error:#}"
    );
    let error = window
        .update(|window, _| window.render_to_image())
        .expect_err("nor at 1x");
    assert!(
        error.to_string().contains("render_to_image not available"),
        "{error:#}"
    );
    cx.quit();
}
