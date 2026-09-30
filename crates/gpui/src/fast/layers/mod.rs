//! Scroll layers: a scroll container's content painted once into cached
//! tiles and composited at the scroll offset on frames where only the
//! offset changed. See docs/superpowers/specs/2026-09-30-scroll-layers-design.md.
//!
//! `scene` is the contract with renderers; each other module belongs to one
//! work stream of the plan (docs/superpowers/plans/2026-09-30-scroll-layers.md).

pub(crate) mod background;
pub(crate) mod input;
pub(crate) mod invalidate;
pub(crate) mod lists;
pub(crate) mod paint;
pub(crate) mod policy;
pub(crate) mod record;
pub(crate) mod reuse;
pub mod scene;
pub(crate) mod tiles;

use crate::{App, GlobalElementId, Window};
use collections::FxHashMap;

/// Whether scroll layers are compiled in (spec §5.1: Linux/wgpu only in v1).
/// Elsewhere every layer entry point is a no-op and today's path runs.
pub(crate) const COMPILED: bool = cfg!(target_os = "linux");

/// A window's scroll layers, by the scroll container's global id.
#[allow(
    dead_code,
    reason = "the skeleton the work streams fill in; remove once they use it"
)]
pub(crate) struct WindowLayers {
    pub(crate) layers: FxHashMap<GlobalElementId, Layer>,
    /// The key the next layer gets, so keys stay unique in the window.
    pub(crate) next_key: u32,
    /// Off in tests that compare against today's path, and where
    /// `GPUI_SCROLL_LAYERS=0`; on by default where compiled.
    pub(crate) enabled: bool,
    /// The scrolls and offset reads of the frame being drawn.
    pub(crate) scrolls: invalidate::ScrollLog,
    /// The layer being painted, if any.
    pub(crate) painting: Option<paint::Painting>,
}

impl Default for WindowLayers {
    fn default() -> Self {
        Self {
            layers: FxHashMap::default(),
            next_key: 0,
            enabled: COMPILED
                && std::env::var("GPUI_SCROLL_LAYERS").map_or(true, |value| value != "0"),
            scrolls: invalidate::ScrollLog::default(),
            painting: None,
        }
    }
}

/// The cached content of one scroll container and what it takes to
/// composite it and route input into it.
#[allow(
    dead_code,
    reason = "the skeleton the work streams fill in; remove once they use it"
)]
pub(crate) struct Layer {
    pub(crate) key: scene::LayerKey,
    /// The content as last painted, once it has been.
    pub(crate) record: Option<record::LayerRecord>,
    pub(crate) policy: policy::LayerPolicy,
    pub(crate) input: input::LayerInput,
    pub(crate) rows: lists::LayerRows,
    /// The last frame the layer's tiles were composited in.
    pub(crate) last_composited_frame: u64,
}

/// Whether layers may be used in `window` this frame (spec §6.5, first bullet).
#[allow(
    dead_code,
    reason = "the skeleton the work streams fill in; remove once they use it"
)]
pub(crate) fn active(window: &Window, cx: &App) -> bool {
    COMPILED
        && window.fast_layers.enabled
        && window.retained_state.view_retention
        && !window.refreshing
        && !cx.has_active_drag()
        && !window.a11y.is_active()
        && !window.is_inspector_picking(cx)
}

impl Window {
    /// Turns scroll layers on or off for this window, dropping every layer,
    /// and redraws it: tests compare the two paths with it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_scroll_layers(&mut self, enabled: bool) {
        self.fast_layers.enabled = enabled;
        self.fast_layers.layers.clear();
        self.refresh();
    }
}
