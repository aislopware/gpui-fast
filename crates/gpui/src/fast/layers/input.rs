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
    App, Bounds, ContentMask, EntityId, GlobalElementId, Hitbox, Pixels, PlatformInput, Point,
    Window,
    fast::layers::{COMPILED, Layer, invalidate, policy::Decision},
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
        && input
            .ranges_frame
            .is_none_or(|painted| painted + 1 != frame)
    {
        return Decision::Repaint;
    }
    decision
}

/// Brings the content of every layer that `event` could reach up to date
/// before it is dispatched (spec §7, rule 3): a layer whose content was
/// shown scrolled since it was painted, and whose viewport holds the
/// pointer (any pointer event but a wheel's) or whose content holds the
/// focus (key events), is painted again at the current offset, in a frame
/// drawn now, so that the positions its closures and element states hold
/// are current when they see the event. Wheel events are dispatched as they
/// come: the content's wheel listeners go by its hitboxes, which are.
///
/// Called by [`Window::dispatch_event`] once it has taken in the event's
/// position.
pub(crate) fn before_dispatch(window: &mut Window, cx: &mut App, event: &PlatformInput) {
    if !COMPILED || window.fast_layers.layers.is_empty() {
        return;
    }
    let reaches: &dyn Fn(&Window, &Layer) -> bool = match event {
        PlatformInput::ScrollWheel(_) => return,
        PlatformInput::KeyDown(_)
        | PlatformInput::KeyUp(_)
        | PlatformInput::ModifiersChanged(_) => &focus_inside,
        _ => &|window, layer| layer.input.viewport.contains(&window.mouse_position()),
    };
    let mut owners = Vec::new();
    let mut unknown_owner = false;
    for layer in window.fast_layers.layers.values() {
        if layer.input.stale != Point::default() && reaches(window, layer) {
            match layer.input.owner {
                Some(owner) => owners.push(owner),
                None => unknown_owner = true,
            }
        }
    }
    let rebuilt = owners.len() + unknown_owner as usize;
    if rebuilt == 0 {
        return;
    }
    for layer in window.fast_layers.layers.values_mut() {
        if layer.input.stale != Point::default()
            && layer
                .input
                .owner
                .is_none_or(|owner| owners.contains(&owner))
        {
            layer.input.stale = Point::default();
        }
    }
    // The view holding the container is built again, as for any change of
    // the content, and the container paints its layer.
    for owner in owners {
        cx.notify(owner);
    }
    if unknown_owner {
        window.refresh();
    }
    window.draw(cx).clear(cx);
    if let Some(engine) = window.layout_engine.as_mut() {
        engine.retention.stats.layer_rebuilds_for_input += rebuilt as u64;
    }
}

/// Whether the focused element is inside the content of `layer`, as the
/// rendered frame holds it. When the rendered frame's records of the
/// content are not the layer's to tell, any focus is taken to be inside.
fn focus_inside(window: &Window, layer: &Layer) -> bool {
    let Some(focus) = window.focus else {
        return false;
    };
    let ranges_current = layer
        .input
        .ranges_frame
        .is_some_and(|frame| frame + 1 == window.fast_layers.frame);
    let Some(record) = layer.record.as_ref().filter(|_| ranges_current) else {
        return true;
    };
    let range = &record.prepaint_range;
    let nodes = &window.rendered_frame.dispatch_tree.nodes;
    let start = range.start.dispatch_tree_index.min(nodes.len());
    let end = range.end.dispatch_tree_index.clamp(start, nodes.len());
    nodes[start..end]
        .iter()
        .any(|node| node.focus_id == Some(focus))
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
