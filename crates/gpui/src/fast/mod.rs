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

pub(crate) mod a11y;
pub(crate) mod backdrop;
pub mod check_retained;
pub mod composition;
pub mod cursor;
pub(crate) mod dependencies;
pub(crate) mod dispatch;
pub(crate) mod edge_fade;
pub(crate) mod element;
pub(crate) mod fling;
pub(crate) mod focus;
pub(crate) mod focus_tooltip;
pub(crate) mod frame_time;
pub mod gesture;
pub(crate) mod glyph_blur;
pub(crate) mod glyphs;
pub(crate) mod headless_atlas;
pub(crate) mod image_evict;
pub(crate) mod image_mask;
pub(crate) mod inline_boxes;
pub(crate) mod interactivity;
pub(crate) mod keyed;
pub(crate) mod keypad;
pub(crate) mod layers;
pub(crate) mod layout;
pub(crate) mod layout_bounds;
pub(crate) mod layout_key;
// longbridge #63's CJK punctuation predicates: only its wrap tests are kept,
// run against `line_breaks` (UAX #14), which already keeps the punctuation in place.
pub(crate) mod draw_timing;
#[cfg(test)]
pub(crate) mod line_break;
pub(crate) mod line_breaks;
pub(crate) mod list_tail;
pub(crate) mod live_region;
pub(crate) mod mask;
pub(crate) mod misc;
pub(crate) mod number_shaping;
pub(crate) mod path_cache;
pub(crate) mod present;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod render_at;
pub(crate) mod retained;
pub(crate) mod rounded_clip;
pub(crate) mod scene;
pub(crate) mod scroll;
pub(crate) mod settle;
pub(crate) mod shift;
pub(crate) mod splice;
pub(crate) mod stats;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod subscriptions;
pub(crate) mod svg_text;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod test_window;
pub(crate) mod text;
pub mod text_smoothing;
pub(crate) mod text_style;
pub(crate) mod touch_hover;
pub(crate) mod transition;
pub(crate) mod value_transition;

#[cfg(test)]
mod tests;
