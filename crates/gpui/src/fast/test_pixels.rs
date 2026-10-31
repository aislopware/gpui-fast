//! A test window that draws real pixels, as the app's window would: a test
//! context with the platform's text system, the app's assets and a headless
//! renderer, a window whose renderer keeps the background's transparency, and a
//! render of the window at another scale (`Window::render_to_image_at`).
//!
//! `#[gpui::test]`'s context has a no-op text system, no assets and no
//! renderer, so its windows lay text out with made-up advances and draw nothing.
//! A test that compares a window's picture with one the app drew needs all
//! three, and the seeded scheduler and the input simulation of a test window
//! beside them.

use std::sync::Arc;

use anyhow::Result;
use image::RgbaImage;

use crate::{
    AssetSource, BackgroundExecutor, DevicePixels, ForegroundExecutor, PlatformHeadlessRenderer,
    PlatformTextSystem, Scene, Size, SvgRenderer, TestAppContext, TestDispatcher, TestPlatform,
    TestWindow, WindowBackgroundAppearance,
};

impl TestAppContext {
    /// Creates a test context whose windows draw real pixels: text shaped and
    /// rasterised by `platform_text_system`, images and SVGs read from
    /// `asset_source`, and each window drawn by a renderer `renderer_factory`
    /// makes for it (`gpui_platform::current_headless_renderer`), which
    /// [`Window::render_to_image`](crate::Window::render_to_image) reads back.
    ///
    /// The scheduler is `dispatcher`'s, seeded and deterministic as under
    /// `#[gpui::test]`; a test that builds its context here also ends it as the
    /// macro does (`run_until_parked`, then `quit`).
    pub fn build_with_renderer(
        dispatcher: TestDispatcher,
        fn_name: Option<&'static str>,
        platform_text_system: Arc<dyn PlatformTextSystem>,
        asset_source: Arc<dyn AssetSource>,
        renderer_factory: impl Fn() -> Result<Option<Box<dyn PlatformHeadlessRenderer>>> + 'static,
    ) -> Self {
        let arc_dispatcher = Arc::new(dispatcher.clone());
        let background_executor = BackgroundExecutor::new(arc_dispatcher.clone());
        let foreground_executor = ForegroundExecutor::new(arc_dispatcher);
        let platform = TestPlatform::with_platform(
            background_executor.clone(),
            foreground_executor.clone(),
            platform_text_system,
            Some(Box::new(renderer_factory)),
        );
        let cx = Self::build_with_platform(
            dispatcher,
            fn_name,
            background_executor,
            foreground_executor,
            platform,
        );
        {
            let mut app = cx.app.borrow_mut();
            app.svg_renderer = SvgRenderer::new(asset_source.clone());
            app.asset_source = asset_source;
        }
        cx
    }
}

/// What the window's background is, as it was last set.
pub(crate) fn background(window: &TestWindow) -> WindowBackgroundAppearance {
    window.0.lock().fast_background
}

/// Sets the window's background, and has its renderer clear to transparent
/// unless the background is opaque, as the macOS window has its layer's.
pub(crate) fn set_background(window: &TestWindow, background: WindowBackgroundAppearance) {
    let mut state = window.0.lock();
    state.fast_background = background;
    if let Some(renderer) = state.renderer.as_mut() {
        renderer.fast_set_transparent(background != WindowBackgroundAppearance::Opaque);
    }
}

/// Renders `scene` offscreen at `size` device pixels, for
/// `Window::render_to_image_at`.
pub(crate) fn render_at(
    window: &TestWindow,
    scene: &Scene,
    size: Size<DevicePixels>,
) -> Result<RgbaImage> {
    match window.0.lock().renderer.as_mut() {
        Some(renderer) => renderer.render_scene_to_image(scene, size),
        None => anyhow::bail!("render_to_image_at not available: no headless renderer configured"),
    }
}
