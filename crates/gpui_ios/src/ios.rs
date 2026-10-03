//! UIKit-backed implementation details for the iOS GPUI platform.

mod a11y;
pub(crate) mod cg_types;
pub mod composition;
mod dispatcher;
mod display;
mod events;

pub mod ffi;
pub(crate) mod menus;
mod platform;
mod text_input;
mod text_system;
mod util;
mod window;

pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub use platform::*;
pub(crate) use text_system::*;
pub use window::set_status_bar_style;
pub(crate) use window::*;

/// Returns the native platform implementation for iOS.
pub fn current_platform(_headless: bool) -> std::rc::Rc<dyn gpui::Platform> {
    std::rc::Rc::new(IosPlatform::new())
}

/// The iOS text system on its own, without the platform around it: Core Text
/// may be used from any thread, where the platform is made on the main one.
pub fn text_system() -> std::sync::Arc<dyn gpui::PlatformTextSystem> {
    std::sync::Arc::new(IosTextSystem::new())
}
