//! Tests of a test window that draws through a headless renderer
//! (`fast::test_pixels`): the background it is given reaches the renderer, and
//! a render at another scale goes to the renderer at that size. The pixels
//! themselves are `gpui_platform`'s tests, on the platform's own renderer.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use anyhow::Result;
use image::RgbaImage;

use crate::{
    AnyWindowHandle, Bounds, DevicePixels, HeadlessAtlas, PlatformAtlas, PlatformHeadlessRenderer,
    Scene, Size, TestAppContext, TestDispatcher, WindowBackgroundAppearance, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};

/// What the renderer was asked: each transparency it was set to, and each
/// size it rendered a picture at.
#[derive(Default)]
struct Asked {
    transparent: Vec<bool>,
    pictures: Vec<Size<DevicePixels>>,
}

/// A renderer that draws nothing and remembers what it was asked; its picture
/// is the clear colour alone, opaque black or transparent.
struct Recording {
    asked: Rc<RefCell<Asked>>,
    transparent: bool,
    atlas: Arc<HeadlessAtlas>,
}

impl PlatformHeadlessRenderer for Recording {
    fn render_scene_to_image(&mut self, _: &Scene, size: Size<DevicePixels>) -> Result<RgbaImage> {
        self.asked.borrow_mut().pictures.push(size);
        let alpha = if self.transparent { 0 } else { 255 };
        Ok(RgbaImage::from_pixel(
            u32::try_from(size.width.0)?,
            u32::try_from(size.height.0)?,
            image::Rgba([0, 0, 0, alpha]),
        ))
    }

    fn render_scene(&mut self, _: &Scene, _: Size<DevicePixels>) -> Result<()> {
        Ok(())
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }

    fn fast_set_transparent(&mut self, transparent: bool) {
        self.asked.borrow_mut().transparent.push(transparent);
        self.transparent = transparent;
    }
}

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut crate::Window, _: &mut crate::Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn recording_context(asked: &Rc<RefCell<Asked>>) -> TestAppContext {
    let asked = asked.clone();
    TestAppContext::build_with_renderer(
        TestDispatcher::new(0),
        None,
        Arc::new(crate::NoopTextSystem),
        Arc::new(()),
        move || {
            Ok(Some(Box::new(Recording {
                asked: asked.clone(),
                transparent: false,
                atlas: Arc::new(HeadlessAtlas::default()),
            })))
        },
    )
}

fn open(cx: &mut TestAppContext, background: WindowBackgroundAppearance) -> AnyWindowHandle {
    let window = cx
        .update(|cx| {
            let bounds = Bounds::centered(None, size(px(50.), px(30.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_background: background,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Blank),
            )
        })
        .unwrap();
    cx.run_until_parked();
    window.into()
}

/// A window opened on a blurred background has its renderer clear to
/// transparent from its first frame, and says it is blurred; set opaque, the
/// renderer clears opaque again.
#[test]
fn a_window_s_background_reaches_its_renderer() {
    let asked = Rc::new(RefCell::new(Asked::default()));
    let mut cx = recording_context(&asked);
    let window = open(&mut cx, WindowBackgroundAppearance::Blurred);
    assert_eq!(asked.borrow().transparent, [true]);
    let test_window = cx.test_window(window);
    assert_eq!(
        crate::PlatformWindow::background_appearance(&test_window),
        WindowBackgroundAppearance::Blurred
    );

    let picture = cx
        .update(|cx| window.update(cx, |_, window, _| window.render_to_image()))
        .unwrap()
        .unwrap();
    assert_eq!(picture.get_pixel(0, 0).0[3], 0, "the clear is transparent");

    cx.update(|cx| {
        window.update(cx, |_, window, _| {
            window.set_background_appearance(WindowBackgroundAppearance::Opaque)
        })
    })
    .unwrap();
    assert_eq!(asked.borrow().transparent, [true, false]);
    assert_eq!(
        crate::PlatformWindow::background_appearance(&test_window),
        WindowBackgroundAppearance::Opaque
    );
    cx.quit();
}

/// A render at another scale asks the renderer for the window's size in points
/// at that scale, rounded up, and the window's own render stays at its own.
#[test]
fn a_render_at_a_scale_is_asked_at_that_scale() {
    let asked = Rc::new(RefCell::new(Asked::default()));
    let mut cx = recording_context(&asked);
    let window = open(&mut cx, WindowBackgroundAppearance::Opaque);
    let scale = cx
        .update(|cx| window.update(cx, |_, window, _| window.scale_factor()))
        .unwrap();
    let at = cx
        .update(|cx| window.update(cx, |_, window, cx| window.render_to_image_at(1.5, cx)))
        .unwrap()
        .unwrap();
    assert_eq!(at.dimensions(), (75, 45));
    let own = cx
        .update(|cx| window.update(cx, |_, window, _| window.render_to_image()))
        .unwrap()
        .unwrap();
    let own_size = (50. * scale).ceil() as u32;
    assert_eq!(own.width(), own_size, "back at its own scale, {scale}");
    let device = |w, h| size(DevicePixels(w), DevicePixels(h));
    assert_eq!(
        asked.borrow().pictures,
        [
            device(75, 45),
            device(own_size as i32, (30. * scale).ceil() as i32)
        ]
    );
    cx.quit();
}
