//! Which scroll containers get a layer: eligibility, promotion, demotion
//! and drop (M4).
//!
//! A scroll container is looked at every frame it prepaints its children.
//! It gets a layer once it has scrolled on two frames in a row (a container
//! that never scrolls never pays for one); from then on the layer is
//! composited on frames that only scrolled it and painted again on the
//! others.

#![allow(
    dead_code,
    reason = "the paint stream's hook around a scroll container's children calls decide; remove once it is merged"
)]

use crate::fast::layers::record::LayerRecord;
use crate::fast::layers::{Layer, input, invalidate, lists, scene::LayerKey};
use crate::{App, Bounds, ContentMask, GlobalElementId, Pixels, Point, Size, TextStyle, Window};

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

/// How many frames in a row a container must scroll on to get a layer.
const PROMOTE_AFTER_SCROLLED_FRAMES: u8 = 2;
/// Layer keys stay below this, as tile texture ids require.
const LAYER_KEY_LIMIT: u32 = 0x0100_0000;

/// A scroll container's standing with its layer.
#[derive(Default)]
pub(crate) struct LayerPolicy {
    /// How many frames in a row, up to the last one looked at, the container
    /// scrolled on.
    pub(crate) scrolled_streak: u8,
    /// The last frame the container scrolled on.
    last_scrolled_frame: Option<u64>,
    /// The last frame the container was looked at.
    last_seen_frame: u64,
    /// Where the content in the layer was painted and what it inherited.
    painted_in: Option<LayerContext>,
    /// What was decided the last time the container was looked at.
    last_decision: Option<Decision>,
}

/// What a container's content is painted with besides what it reads: where
/// the container is and what it inherits. A layer painted in one context is
/// not composited in another.
#[derive(Clone, PartialEq)]
struct LayerContext {
    bounds: Bounds<Pixels>,
    content_size: Size<Pixels>,
    content_mask: ContentMask<Pixels>,
    text_style: TextStyle,
    opacity: f32,
    rem_size: Pixels,
    scale_factor: f32,
}

impl LayerContext {
    fn current(window: &Window, bounds: Bounds<Pixels>, content_size: Size<Pixels>) -> Self {
        LayerContext {
            bounds,
            content_size,
            content_mask: window.content_mask(),
            text_style: window.text_style(),
            opacity: window.element_opacity,
            rem_size: window.rem_size(),
            scale_factor: window.scale_factor(),
        }
    }
}

/// Decides what the scroll container `id`, prepainted at `bounds` with
/// children spanning `content_size` and scrolled by `scroll_offset`, does
/// with its children this frame, and keeps its standing up to date.
///
/// Called where the container prepaints its children, before it does.
pub(crate) fn decide(
    window: &mut Window,
    cx: &mut App,
    id: &GlobalElementId,
    bounds: Bounds<Pixels>,
    content_size: Size<Pixels>,
    scroll_offset: Point<Pixels>,
) -> Decision {
    // A scroll container inside a layer is painted into it (spec §6.6).
    if !super::active(window, cx) || window.fast_layers.painting.is_some() {
        return Decision::Bypass;
    }
    let frame = window.fast_layers.frame;
    let scrolled = invalidate::scrolled(window, id);
    if !window.fast_layers.layers.contains_key(id) {
        if !scrolled {
            return Decision::Bypass;
        }
        let key = LayerKey(window.fast_layers.next_key);
        window.fast_layers.next_key = (window.fast_layers.next_key + 1) % LAYER_KEY_LIMIT;
        window
            .fast_layers
            .layers
            .insert(id.clone(), new_layer(key, frame));
    }
    let context = LayerContext::current(window, bounds, content_size);
    let layer = &window.fast_layers.layers[id];
    let policy = &layer.policy;

    let streak = if !scrolled {
        0
    } else if policy
        .last_scrolled_frame
        .is_some_and(|last| last + 1 == frame)
    {
        policy.scrolled_streak.saturating_add(1)
    } else {
        1
    };

    let decision = match &layer.record {
        Some(record)
            if policy.painted_in.as_ref() == Some(&context)
                && invalidate::scroll_only(window, cx, id, record)
                && covers(record, bounds, content_size, scroll_offset) =>
        {
            Decision::Composite
        }
        Some(_) => Decision::Repaint,
        None if streak >= PROMOTE_AFTER_SCROLLED_FRAMES => Decision::Repaint,
        None => Decision::Bypass,
    };

    let layer = window.fast_layers.layers.get_mut(id).unwrap();
    let policy = &mut layer.policy;
    policy.scrolled_streak = streak;
    if scrolled {
        policy.last_scrolled_frame = Some(frame);
    }
    policy.last_seen_frame = frame;
    policy.last_decision = Some(decision);
    match decision {
        Decision::Repaint => {
            policy.painted_in = Some(context);
            layer.last_composited_frame = frame;
        }
        Decision::Composite => layer.last_composited_frame = frame,
        Decision::Bypass => {}
    }
    decision
}

/// Whether the part of the content `record` painted still covers the
/// viewport at `bounds`, scrolled by `scroll_offset`, with a margin of a
/// quarter of the overscan — a quarter of the viewport's extent — around
/// it, as far as the content reaches (spec §5.4).
fn covers(
    record: &LayerRecord,
    bounds: Bounds<Pixels>,
    content_size: Size<Pixels>,
    scroll_offset: Point<Pixels>,
) -> bool {
    let translation = scroll_offset - record.scroll_offset;
    let painted = Bounds {
        origin: record.painted_region.origin + translation,
        size: record.painted_region.size,
    };
    let margin = Point {
        x: record.viewport.size.width / 4.,
        y: record.viewport.size.height / 4.,
    };
    let viewport = record.viewport;
    let wanted = Bounds::from_corners(viewport.origin - margin, viewport.bottom_right() + margin);
    let content = Bounds {
        origin: bounds.origin + scroll_offset,
        size: content_size,
    };
    let wanted = wanted.intersect(&content);
    if wanted.size.width <= Pixels::ZERO || wanted.size.height <= Pixels::ZERO {
        return true;
    }
    wanted.origin.x >= painted.origin.x
        && wanted.origin.y >= painted.origin.y
        && wanted.right() <= painted.right()
        && wanted.bottom() <= painted.bottom()
}

fn new_layer(key: LayerKey, frame: u64) -> Layer {
    Layer {
        key,
        record: None,
        policy: LayerPolicy {
            last_seen_frame: frame,
            ..LayerPolicy::default()
        },
        input: input::LayerInput::default(),
        rows: lists::LayerRows::default(),
        last_composited_frame: frame,
    }
}

/// What the scroll container `id` decided in the last frame drawn, if it
/// was looked at in it.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn last_decision(window: &Window, id: &GlobalElementId) -> Option<Decision> {
    let policy = &window.fast_layers.layers.get(id)?.policy;
    (policy.last_seen_frame + 1 == window.fast_layers.frame)
        .then_some(policy.last_decision)
        .flatten()
}
