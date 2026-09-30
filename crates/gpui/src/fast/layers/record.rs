//! A layer's recorded content: its content-space scene, painted region,
//! translation at paint, prepaint and paint ranges and dependencies (M3).

use std::{ops::Range, rc::Rc};

use crate::fast::dependencies::RenderDependencies;
use crate::{Bounds, HitboxId, PaintIndex, Pixels, Point, PrepaintStateIndex};

// The fields the invalidation stream (M4) decides with, named as the paint
// stream (M3), which owns this file, names them; it adds the rest.
#[allow(
    dead_code,
    reason = "policy::decide reads them, which the paint stream's hook calls"
)]
#[derive(Default)]
pub(crate) struct LayerRecord {
    /// The part of the content painted, in window space at paint: the
    /// viewport and the overscan around it.
    pub(crate) painted_region: Bounds<Pixels>,
    /// The container's clip rect in window space at paint.
    pub(crate) viewport: Bounds<Pixels>,
    /// The scroll offset the content was painted at, snapped.
    pub(crate) scroll_offset: Point<Pixels>,
    /// What prepainting the content added to the frame.
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    /// What painting the content added to the frame.
    pub(crate) paint_range: Range<PaintIndex>,
    /// The hovers the content was painted by.
    pub(crate) hovers: Rc<[(HitboxId, bool)]>,
    /// What prepainting and painting the content read.
    pub(crate) dependencies: RenderDependencies,
    /// Whether the content painted a path, which a layer never composites
    /// (spec §5.6).
    pub(crate) has_paths: bool,
}
