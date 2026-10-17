//! Everything gpui-fast adds to the platform crate lives under this module.
//!
//! `gpui_platform.rs` is kept as upstream GPUI (Zed's `crates/gpui_platform`)
//! has it, apart from the lines that call into this module.

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod clipboard;
pub(crate) mod text_system;
