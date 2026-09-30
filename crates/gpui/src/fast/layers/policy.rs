//! Which scroll containers get a layer: eligibility, promotion, demotion
//! and drop (M4).

use crate::{App, Bounds, GlobalElementId, Pixels, Point, Size, Window};

#[derive(Default)]
pub(crate) struct LayerPolicy {}

/// What a scroll container does with its content this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Today's path: the content is prepainted and painted into the frame.
    Bypass,
    /// The content is painted into the container's layer, whose tiles are
    /// composited.
    Repaint,
    /// The content is left as the layer holds it; its tiles are composited
    /// at the new offset.
    Composite,
}

/// What the scroll container `id` does with its content this frame.
///
/// Until the policy stream (M4) decides it, every container takes today's
/// path, unless a test forces a decision.
pub(crate) fn decide(
    window: &Window,
    _cx: &App,
    _id: &GlobalElementId,
    _bounds: Bounds<Pixels>,
    _content_size: Size<Pixels>,
    _scroll_offset: Point<Pixels>,
) -> Decision {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(decision) = window.fast_layers.forced_decision {
        return decision;
    }
    let _ = window;
    Decision::Bypass
}
