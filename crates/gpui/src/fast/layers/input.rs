//! Input under scroll layers: hitbox translation and clipping, rebuilding
//! before input, tooltips and getter translation (M5).
//!
//! Application code sees window coordinates only (spec §7). On frames that
//! composite a layer its content is not prepainted or painted again, so the
//! hitboxes it inserted are carried moved by the scroll (see
//! [`crate::fast::layers::reuse`]), and positions its closures and element
//! states captured lag behind by as much. The content is painted again
//! before it sees input that could observe them.

use crate::{
    Bounds, ContentMask, EntityId, GlobalElementId, Hitbox, Pixels, Point, Window,
    fast::layers::{invalidate, policy::Decision},
};

/// What a layer keeps to route input into its content.
#[derive(Default)]
pub(crate) struct LayerInput {
    /// The content's hitboxes as painted: in window space at the offset the
    /// content was painted at, their masks clipped by the content's own
    /// clips but not by the viewport.
    pub(crate) hitboxes: Vec<Hitbox>,
    /// The frame the layer's record's prepaint and paint ranges index the
    /// records of: they can be carried into the frame after it only.
    pub(crate) ranges_frame: Option<u64>,
    /// How far the content shown last frame had scrolled since it was
    /// painted: how far behind the positions its closures and element
    /// states hold are.
    pub(crate) stale: Point<Pixels>,
    /// The container's clip rect in window space, last frame.
    pub(crate) viewport: Bounds<Pixels>,
    /// The view holding the container, last frame.
    pub(crate) owner: Option<EntityId>,
}

impl LayerInput {
    /// `hitboxes`, moved by `delta` and clipped to `viewport`.
    pub(crate) fn hitboxes_at(
        hitboxes: &[Hitbox],
        delta: Point<Pixels>,
        viewport: Bounds<Pixels>,
    ) -> impl Iterator<Item = Hitbox> {
        hitboxes.iter().map(move |hitbox| Hitbox {
            id: hitbox.id,
            bounds: Bounds {
                origin: hitbox.bounds.origin + delta,
                size: hitbox.bounds.size,
            },
            content_mask: ContentMask {
                bounds: Bounds {
                    origin: hitbox.content_mask.bounds.origin + delta,
                    size: hitbox.content_mask.bounds.size,
                }
                .intersect(&viewport),
            },
            behavior: hitbox.behavior,
        })
    }
}

/// What the container `id` does with its children, given the policy's
/// `decision`: a layer is composited only if what its content added to the
/// last frame can be carried into this one, which a frame drawn without
/// the container prepainting its children in between (a reused view, a
/// refresh) prevents. It is painted again otherwise.
pub(crate) fn decide(window: &mut Window, id: &GlobalElementId, decision: Decision) -> Decision {
    let frame = window.fast_layers.frame;
    let owner = invalidate::owner_view(window);
    let Some(layer) = window.fast_layers.layers.get_mut(id) else {
        return decision;
    };
    let input = &mut layer.input;
    input.owner = owner;
    if decision == Decision::Composite
        && input.ranges_frame.is_none_or(|painted| painted + 1 != frame)
    {
        return Decision::Repaint;
    }
    decision
}

/// Notes that the content of the layer of the container `id` was just
/// painted into the frame being drawn, over its record's prepaint range.
pub(crate) fn painted(window: &mut Window, id: &GlobalElementId) {
    let frame = window.fast_layers.frame;
    let Some(layer) = window.fast_layers.layers.get_mut(id) else {
        return;
    };
    let Some(record) = layer.record.as_ref() else {
        return;
    };
    let range = &record.prepaint_range;
    let input = &mut layer.input;
    input.hitboxes.clear();
    input.hitboxes.extend_from_slice(
        &window.next_frame.hitboxes[range.start.hitboxes_index..range.end.hitboxes_index],
    );
    input.ranges_frame = Some(frame);
    input.stale = Point::default();
    input.viewport = record.viewport;
}
