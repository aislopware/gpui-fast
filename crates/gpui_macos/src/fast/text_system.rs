//! The Mac's text system without the platform.

use gpui::PlatformTextSystem;
use std::sync::Arc;

/// The Mac's text system on its own, without the platform around it.
///
/// `MacPlatform::new` has to run on the main thread, but Core Text and
/// font-kit do not: a test, which runs on a thread of its own, or a benchmark
/// that only shapes and rasterises text takes the text system from here.
pub fn text_system() -> Arc<dyn PlatformTextSystem> {
    Arc::new(gpui_apple::AppleTextSystem::new())
}
