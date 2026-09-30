//! Painting a scroll container's children into its layer: the scene swap,
//! the cull/clip split and inserting the tile quads (M3).
//!
//! A `div` that scrolls prepaints and paints its children through
//! [`begin_children`], [`prepaint_children`], [`end_children`] and
//! [`paint_children`]. They ask [`policy::decide`] what to do and do it:
//!
//! - `Bypass`: today's path.
//! - `Repaint`: the children are prepainted and painted as today, in window
//!   space at the current offset, but culled against the painted region (the
//!   viewport and its overscan) instead of the viewport, and painted into the
//!   layer's own scene, which is then stored in content space, diffed into
//!   tiles, and composited.
//! - `Composite`: the children are neither prepainted nor painted; the layer's
//!   tiles are composited at the new offset.

use crate::{
    App, Bounds, ContentMask, GlobalElementId, Overflow, Pixels, Point, PrepaintStateIndex, Rgba,
    ScaledPixels, Scene, Size, Style, TileCoord, Window, WindowBackgroundAppearance,
    fast::{
        dependencies::{DependencyRecording, RenderDependencies},
        layers::{
            COMPILED, Layer, active, background,
            policy::{self, Decision},
            record::LayerRecord,
            scene::{LayerKey, translate_primitive},
            tiles::{dirty_tiles, tile_hashes},
        },
    },
    point,
    scene::PaintOperation,
};
use collections::FxHashMap;
use std::{mem, ops::Range, rc::Rc};

/// The side of a tile, in device pixels.
pub(crate) const TILE_SIZE: u32 = 512;

/// The layer whose content is being prepainted or painted.
pub(crate) struct Painting {
    /// The scroll container.
    pub(crate) id: GlobalElementId,
    /// The container's clip rect, in window space.
    pub(crate) viewport: Bounds<Pixels>,
    /// The part of the content painted, in window space: the viewport and
    /// the overscan around it, within the content.
    pub(crate) painted_region: Bounds<Pixels>,
    /// While the content paints, the frame's scene, swapped out for the
    /// layer's; otherwise the layer's scene, in window space.
    pub(crate) scene: Scene,
    /// The offset the content is painted at, snapped.
    pub(crate) scroll_offset: Point<Pixels>,
    /// The layer's translation, `scroll_offset` in whole device pixels.
    pub(crate) translation: Point<ScaledPixels>,
    /// What prepainting the content added to the frame.
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    /// The recording of what prepainting the content reads, while it runs.
    pub(crate) recording: Option<DependencyRecording>,
    /// What prepainting the content read.
    pub(crate) dependencies: RenderDependencies,
}

/// What a container's prepaint decided, for its paint to carry out.
pub(crate) enum Prepainted {
    /// The content was prepainted into the layer, and is painted into it.
    Repaint(Painting),
    /// The content was not prepainted; the layer's tiles are composited.
    #[allow(dead_code, reason = "compositing reads them from the next task on")]
    Composite {
        viewport: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
    },
}

/// What a container decided for its children this frame, from
/// [`begin_children`] to [`end_children`].
#[derive(Clone, Copy)]
pub(crate) struct Children {
    decision: Decision,
    scroll_offset: Point<Pixels>,
}

/// Whether the content of a layer is being prepainted or painted: nested
/// retained views then record nothing and reuse nothing, the layer being
/// their retention (spec §6.3).
#[inline]
pub(crate) fn inside_layer(window: &Window) -> bool {
    window.fast_layers.painting.is_some()
}

/// Decides what the container `id`, prepainting its children at
/// `scroll_offset`, does with them, and sets up their prepaint for it.
/// `bounds` are the container's, `child_min` and `content_size` where its
/// children lie before scrolling.
#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_children(
    window: &mut Window,
    cx: &mut App,
    id: Option<&GlobalElementId>,
    bounds: Bounds<Pixels>,
    child_min: Point<Pixels>,
    content_size: Size<Pixels>,
    scroll_offset: Point<Pixels>,
    style: &Style,
) -> Children {
    let scroll_offset = snap_scroll_offset(window, scroll_offset);
    let bypass = Children {
        decision: Decision::Bypass,
        scroll_offset,
    };
    if !COMPILED {
        return bypass;
    }
    let Some(id) = id else {
        return bypass;
    };
    if !scrolls(style) || inside_layer(window) || !active(window, cx) {
        return bypass;
    }
    let viewport = window.content_mask().bounds;
    let mut decision = policy::decide(window, cx, id, bounds, content_size, scroll_offset);
    if decision == Decision::Composite
        && window
            .fast_layers
            .layers
            .get(id)
            .is_none_or(|layer| layer.record.is_none())
    {
        decision = Decision::Repaint;
    }
    match decision {
        Decision::Bypass => return bypass,
        Decision::Composite => {
            layer_mut(window, id).prepainted = Some(Prepainted::Composite {
                viewport,
                scroll_offset,
            });
        }
        Decision::Repaint => {
            let content_origin = if child_min.x == Pixels::MAX {
                bounds.origin
            } else {
                child_min
            };
            let content = Bounds {
                origin: content_origin + scroll_offset,
                size: content_size,
            };
            let painted_region = painted_region(viewport, content, style.overflow);
            let start = window.prepaint_index();
            window.fast_layers.painting = Some(Painting {
                id: id.clone(),
                viewport,
                painted_region,
                scene: Scene::default(),
                scroll_offset,
                translation: translation(window, scroll_offset),
                prepaint_range: start.clone()..start,
                recording: Some(cx.begin_recording_dependencies()),
                dependencies: RenderDependencies::default(),
            });
            // Culling works in the painted region, not in the viewport and
            // whatever clips it; the composite clips to those.
            window.content_mask_stack.push(ContentMask {
                bounds: painted_region,
            });
        }
    }
    Children {
        decision,
        scroll_offset,
    }
}

/// Prepaints a container's children as [`begin_children`] decided: `f` is
/// the children's prepaint, run at the container's scroll offset unless
/// the layer is composited.
pub(crate) fn prepaint_children(
    window: &mut Window,
    children: Children,
    f: impl FnOnce(&mut Window),
) {
    if children.decision != Decision::Composite {
        window.with_element_offset(children.scroll_offset, f);
    }
}

/// Ends what [`begin_children`] began, once the children are prepainted.
pub(crate) fn end_children(window: &mut Window, cx: &mut App, children: Children) {
    if children.decision != Decision::Repaint {
        return;
    }
    window.content_mask_stack.pop();
    let Some(mut painting) = window.fast_layers.painting.take() else {
        debug_assert!(false, "a layer's prepaint ended without beginning");
        return;
    };
    painting.prepaint_range.end = window.prepaint_index();
    if let Some(recording) = painting.recording.take() {
        painting.dependencies = cx.finish_recording_dependencies(recording).all;
    }
    let id = painting.id.clone();
    layer_mut(window, &id).prepainted = Some(Prepainted::Repaint(painting));
}

/// Paints a container's children as its prepaint decided: `f` is the
/// children's paint.
pub(crate) fn paint_children(
    window: &mut Window,
    cx: &mut App,
    id: Option<&GlobalElementId>,
    f: impl FnOnce(&mut Window, &mut App),
) {
    let prepainted = match id {
        Some(id) if !window.fast_layers.layers.is_empty() => window
            .fast_layers
            .layers
            .get_mut(id)
            .and_then(|layer| layer.prepainted.take()),
        _ => None,
    };
    match prepainted {
        None => f(window, cx),
        Some(Prepainted::Repaint(painting)) => match bake_background(window) {
            Some(background) => repaint(window, cx, painting, background, f),
            None => paint_unbaked(window, cx, painting, f),
        },
        Some(Prepainted::Composite { .. }) => {
            if let Some(id) = id {
                composite(window, id);
            }
        }
    }
}

/// The opaque colour a layer's tiles are cleared with: what the frame has
/// painted so far under the viewport, the current content mask, if it is
/// one solid quad covering it (spec §5.2).
fn bake_background(window: &Window) -> Option<Rgba> {
    let window_opaque =
        window.platform_window.background_appearance() == WindowBackgroundAppearance::Opaque;
    let viewport = window.snapped_content_mask().bounds;
    background::bake(&window.next_frame.scene, viewport, window_opaque)
}

/// Paints content prepainted for a layer straight into the frame, as
/// without one, because the layer's tiles could not be cleared with what is
/// under them. The layer keeps no content, so that it is painted afresh.
fn paint_unbaked(
    window: &mut Window,
    cx: &mut App,
    painting: Painting,
    f: impl FnOnce(&mut Window, &mut App),
) {
    let id = painting.id.clone();
    // Nested views were prepainted inside the layer, and are painted so.
    window.fast_layers.painting = Some(painting);
    f(window, cx);
    window.fast_layers.painting = None;
    layer_mut(window, &id).record = None;
}

/// Carries out a `Composite` decision for the container `id`: the layer's
/// content stands, cleared with the background now under it.
fn composite(window: &mut Window, id: &GlobalElementId) {
    let background = bake_background(window);
    let layer = layer_mut(window, id);
    let Some(record) = layer.record.as_mut() else {
        return;
    };
    match background {
        Some(background) if background == record.background => {}
        Some(background) => {
            // Every tile is cleared with another colour: a new generation of
            // the same content, all of it dirty.
            record.background = background;
            record.generation += 1;
            record.dirty_tiles = all_tiles(&record.tile_hashes);
        }
        None => layer.record = None,
    }
}

/// Every tile of `hashes`, sorted.
fn all_tiles(hashes: &FxHashMap<TileCoord, u64>) -> Vec<TileCoord> {
    let mut all: Vec<_> = hashes.keys().copied().collect();
    all.sort();
    all
}

/// Paints the content into the layer's scene and records it.
fn repaint(
    window: &mut Window,
    cx: &mut App,
    mut painting: Painting,
    background: Rgba,
    f: impl FnOnce(&mut Window, &mut App),
) {
    window.content_mask_stack.push(ContentMask {
        bounds: painting.painted_region,
    });
    mem::swap(&mut window.next_frame.scene, &mut painting.scene);
    let paint_start = window.paint_index();
    window.take_hover_reads();
    let hovers_start = window.retained_state.hover_dependencies.len();
    let recording = cx.begin_recording_dependencies();
    window.fast_layers.painting = Some(painting);

    f(window, cx);

    let mut painting = window
        .fast_layers
        .painting
        .take()
        .expect("the layer being painted");
    let paint_dependencies = cx.finish_recording_dependencies(recording).all;
    window.take_hover_reads();
    let hovers: Rc<[_]> = window.retained_state.hover_dependencies[hovers_start..].into();
    let paint_end = window.paint_index();
    mem::swap(&mut window.next_frame.scene, &mut painting.scene);
    window.content_mask_stack.pop();

    let scale_factor = window.scale_factor();
    let translation = painting.translation;
    let to_content = point(
        ScaledPixels(-translation.x.0),
        ScaledPixels(-translation.y.0),
    );
    let content = translated_scene(&painting.scene, to_content);
    let region = painting.painted_region.scale(scale_factor);
    let region = Bounds {
        origin: region.origin + to_content,
        size: region.size,
    };
    let hashes = tile_hashes(&content, TILE_SIZE, region);

    let layer = layer_mut(window, &painting.id);
    let (generation, dirty) = match &layer.record {
        Some(old) if old.background == background => {
            (old.generation + 1, dirty_tiles(&old.tile_hashes, &hashes))
        }
        old => (
            old.as_ref().map_or(1, |old| old.generation + 1),
            all_tiles(&hashes),
        ),
    };
    layer.record = Some(LayerRecord {
        content: Rc::new(content),
        generation,
        painted_region: painting.painted_region,
        viewport: painting.viewport,
        scroll_offset: painting.scroll_offset,
        translation,
        prepaint_range: painting.prepaint_range,
        paint_range: paint_start..paint_end,
        tile_hashes: hashes,
        dirty_tiles: dirty,
        background,
        hovers,
        dependencies: painting.dependencies.union(&paint_dependencies),
    });
    window
        .layout_engine
        .as_mut()
        .unwrap()
        .retention
        .stats
        .layer_frames_repainted += 1;
}

/// Ends the layers' part of the frame being drawn, before it becomes the
/// rendered frame.
pub(crate) fn finish_frame(window: &mut Window) {
    debug_assert!(window.fast_layers.painting.is_none());
    for layer in window.fast_layers.layers.values_mut() {
        layer.prepainted = None;
    }
}

/// The layer of the container `id`, made if it has none.
fn layer_mut<'a>(window: &'a mut Window, id: &GlobalElementId) -> &'a mut Layer {
    let layers = &mut window.fast_layers;
    if !layers.layers.contains_key(id) {
        let key = LayerKey(layers.next_key);
        layers.next_key += 1;
        layers.layers.insert(
            id.clone(),
            Layer {
                key,
                record: None,
                policy: Default::default(),
                input: Default::default(),
                rows: Default::default(),
                last_composited_frame: 0,
                prepainted: None,
            },
        );
    }
    layers.layers.get_mut(id).unwrap()
}

/// Whether `style` scrolls on some axis.
fn scrolls(style: &Style) -> bool {
    style.overflow.x == Overflow::Scroll || style.overflow.y == Overflow::Scroll
}

/// The part of the content a layer paints: the viewport, and one viewport's
/// extent on each side along the scrolled axes as far as the content goes.
pub(crate) fn painted_region(
    viewport: Bounds<Pixels>,
    content: Bounds<Pixels>,
    overflow: Point<Overflow>,
) -> Bounds<Pixels> {
    fn extend(
        min: Pixels,
        extent: Pixels,
        content_min: Pixels,
        content_extent: Pixels,
    ) -> (Pixels, Pixels) {
        let max = min + extent;
        let low = (min - extent).max(content_min).min(min);
        let high = (max + extent).min(content_min + content_extent).max(max);
        (low, high - low)
    }
    let mut region = viewport;
    if overflow.x == Overflow::Scroll {
        (region.origin.x, region.size.width) = extend(
            viewport.origin.x,
            viewport.size.width,
            content.origin.x,
            content.size.width,
        );
    }
    if overflow.y == Overflow::Scroll {
        (region.origin.y, region.size.height) = extend(
            viewport.origin.y,
            viewport.size.height,
            content.origin.y,
            content.size.height,
        );
    }
    region
}

/// A layer's translation at `scroll_offset`: the offset in whole device
/// pixels.
pub(crate) fn translation(window: &Window, scroll_offset: Point<Pixels>) -> Point<ScaledPixels> {
    let scale_factor = window.scale_factor();
    point(
        ScaledPixels((scroll_offset.x.0 * scale_factor).round()),
        ScaledPixels((scroll_offset.y.0 * scale_factor).round()),
    )
}

/// `scene` moved by `delta`, finished.
fn translated_scene(scene: &Scene, delta: Point<ScaledPixels>) -> Scene {
    let mut translated = Scene::default();
    for operation in &scene.paint_operations {
        match operation {
            PaintOperation::Primitive(primitive) => {
                translated.insert_primitive(translate_primitive(primitive, delta))
            }
            PaintOperation::StartLayer(bounds) => translated.push_layer(Bounds {
                origin: bounds.origin + delta,
                size: bounds.size,
            }),
            PaintOperation::EndLayer => translated.pop_layer(),
        }
    }
    translated.finish();
    translated
}

/// `offset`, a scroll offset about to be applied, moved to whole device
/// pixels where layers are compiled, with or without a layer, so that a
/// layer's content and content drawn without one land on the same pixels
/// (spec §5.3).
pub(crate) fn snap_scroll_offset(window: &Window, offset: Point<Pixels>) -> Point<Pixels> {
    snap_offset(offset, window.scale_factor(), COMPILED)
}

/// [`snap_scroll_offset`] at `scale_factor`, snapping only when `compiled`,
/// rounding as [`Window::pixel_snap`] does.
pub(crate) fn snap_offset(
    offset: Point<Pixels>,
    scale_factor: f32,
    compiled: bool,
) -> Point<Pixels> {
    if !compiled {
        return offset;
    }
    offset.map(|value| {
        crate::px(crate::util::round_half_toward_zero(value.0 * scale_factor) / scale_factor)
    })
}
