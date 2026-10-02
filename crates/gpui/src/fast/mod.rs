//! Everything gpui-fast adds to GPUI lives under this module.
//!
//! The rest of this crate is kept as close to upstream GPUI (Zed's
//! `crates/gpui`) as possible, so that pulling in upstream changes stays a
//! matter of merging rather than of rewriting. Upstream files only *call into*
//! this module: a field holding this module's state, a line forwarding a method
//! to it, a hook at the point something happens. The logic itself — data
//! structures, algorithms, bookkeeping, tests — is written here, one file per
//! topic. See `docs/upstream-sync.md` for the rules and how they are checked.
//!
//! Nothing here is glob-imported or glob-re-exported. Code, upstream's or
//! ours, names what it uses by its place in this module —
//! `crate::fast::retained::RetainedSubtrees` — so it is always plain where it
//! comes from. The few items that are public API are exported from the crate
//! root one by one, in `gpui.rs`.

pub mod composition;
pub mod cursor;
pub(crate) mod dependencies;
pub(crate) mod dispatch;
pub(crate) mod element;
pub(crate) mod focus;
pub mod gesture;
pub(crate) mod global_id;
pub(crate) mod glyphs;
pub(crate) mod headless_atlas;
pub(crate) mod interactivity;
pub(crate) mod keyed;
pub(crate) mod keypad;
pub(crate) mod layers;
pub(crate) mod layout;
pub(crate) mod layout_bounds;
pub(crate) mod layout_key;
pub(crate) mod line_breaks;
pub(crate) mod number_shaping;
pub(crate) mod path_cache;
pub(crate) mod retained;
pub(crate) mod scene;
pub(crate) mod scroll;
pub(crate) mod shift;
pub(crate) mod splice;
pub(crate) mod stats;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod subscriptions;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod test_window;
pub(crate) mod text;
pub mod text_smoothing;
pub(crate) mod text_style;

#[cfg(test)]
mod tests;
