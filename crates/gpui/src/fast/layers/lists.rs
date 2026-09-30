//! Rendering only the rows of a `uniform_list` or `list` its layer lacks (M6).
//!
//! A virtual list renders only the rows it shows. Its layer holds more: the
//! rows its viewport shows and one viewport's worth of rows on each side
//! (the overscan), each painted into the layer's content scene at its place
//! in content space and kept there by row. On a frame that only scrolled
//! the list the rows the layer holds are neither rendered, laid out,
//! prepainted nor painted: only the rows the scroll brought into the
//! overscan are, and are added to the layer, whose tiles over them become
//! dirty; rows that left the overscan are dropped from it (spec §8). Any
//! other frame paints the rows afresh, as a div's layer is.
//!
//! The list elements call in here from a few hooks:
//!
//! - `uniform_list`: [`measure_item`], [`snap_item_offset`],
//!   [`begin_uniform_list`], [`render_rows`], [`row_indices`] and
//!   [`end_rows`] where it prepaints its rows; [`begin_paint_rows`],
//!   [`paint_row`] and [`end_paint_rows`] where it paints them.
//! - `list`: [`begin_list`], [`keeps_row`], [`snap_list_offset`],
//!   [`list_items_origin`], [`place_list_item`] and [`end_list`] where it lays
//!   out and prepaints its rows; [`begin_paint_list`], [`paint_row`] and
//!   [`end_paint_list`] where it paints them. A `list` has no element id: its
//!   layer is known by the id of the elements around it and its state.
//!
//! Between a list's prepaint and its paint, what the frame does with its
//! rows lives in its layer's [`LayerRows`]. While its rows prepaint and
//! paint, `WindowLayers::painting` is set, as for a div's layer, so that
//! nested views and scroll containers are painted into the layer.

use crate::{
    AnyElement, App, Bounds, ContentMask, GlobalElementId, PaintIndex, Pixels, Point,
    PrepaintStateIndex, Rgba, ScaledPixels, Scene, Size, TextStyle, Window,
    fast::{
        dependencies::{DependencyRecording, RenderDependencies},
        layers::{
            COMPILED, active, invalidate,
            paint::{self, Painting},
            policy::{self, Decision},
            record::LayerRecord,
            scene::translate_primitive,
            tiles::{dirty_tiles, tile_hashes},
        },
    },
    point, px,
    scene::PaintOperation,
    size,
};
use collections::FxHashMap;
use smallvec::SmallVec;
use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
    ops::Range,
    rc::Rc,
};

/// The rows of a list's layer, and what the frame being drawn does with
/// them.
#[derive(Default)]
pub(crate) struct LayerRows {
    /// The rows the layer holds, by index.
    pub(crate) painted: BTreeSet<usize>,
    /// Where each row the layer holds lies, in content space.
    pub(crate) row_origins: FxHashMap<usize, Point<Pixels>>,
    /// Whether the layer is a list's, which a scroll extends by the rows it
    /// uncovers instead of painting it again.
    pub(crate) list: bool,
    /// The content of each row the layer holds.
    rows: BTreeMap<usize, Row>,
    /// A uniform list's measured item, as last measured.
    measured: Option<Measured>,
    /// What the frame being drawn does with the list's rows, from its
    /// prepaint to its paint.
    frame: Option<RowsFrame>,
}

impl LayerRows {
    /// Ends the frame being drawn.
    pub(crate) fn finish_frame(&mut self) {
        self.frame = None;
    }

    fn clear(&mut self) {
        self.painted.clear();
        self.row_origins.clear();
        self.rows.clear();
    }
}

/// A row as the layer holds it.
struct Row {
    /// The row's slot, as wide as the viewport, in content space.
    slot: Bounds<ScaledPixels>,
    /// What painting the row drew, in content space.
    operations: Vec<PaintOperation>,
}

/// A uniform list's measured item, and what it was measured with besides
/// the item itself.
struct Measured {
    size: Size<Pixels>,
    rem_size: Pixels,
    text_style: TextStyle,
}

/// The rows a list renders into its layer this frame, and those it keeps.
pub(crate) struct RowPlan {
    /// The rows to render, prepaint and paint into the layer, in order.
    pub(crate) render: Vec<Range<usize>>,
    /// The rows the layer holds that stay, which are not rendered.
    pub(crate) keep: Range<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// The rows are painted afresh.
    Repaint,
    /// The rows the layer holds stay; those it lacks are added.
    Extend,
}

/// What the frame being drawn does with a list's rows.
struct RowsFrame {
    mode: Mode,
    /// The rows the layer is to hold after the frame.
    needed: Range<usize>,
    /// The rows the layer holds that stay.
    keep: Range<usize>,
    /// Where each row painted into the layer this frame lies, in window
    /// space.
    slots: BTreeMap<usize, Bounds<Pixels>>,
    /// The list's clip rect, in window space.
    viewport: Bounds<Pixels>,
    /// The part of the content the frame paints, in window space: the
    /// viewport and the rows around it.
    painted_region: Bounds<Pixels>,
    /// The offset the rows are painted at, snapped.
    scroll_offset: Point<Pixels>,
    /// How far the layer's content is moved into window space this frame.
    translation: Point<ScaledPixels>,
    prepaint_start: PrepaintStateIndex,
    prepaint_range: Range<PrepaintStateIndex>,
    /// The recording of what rendering and prepainting the rows reads.
    recording: Option<DependencyRecording>,
    dependencies: RenderDependencies,
    /// A uniform list's rows in the order it paints them, and how many it
    /// painted.
    order: Vec<usize>,
    next: usize,
    paint: Option<PaintState>,
}

/// A list's rows being painted.
struct PaintState {
    /// The opaque colour under the viewport, if there is one; the rows are
    /// painted into the frame otherwise.
    background: Option<Rgba>,
    /// Whether rows are painted into the layer's scene, which the frame's is
    /// then swapped out for.
    swapped: bool,
    /// The frame's scene, swapped out for the layer's.
    scene: Scene,
    paint_start: PaintIndex,
    hovers_start: usize,
    recording: Option<DependencyRecording>,
    /// The row being painted.
    current: Option<usize>,
    /// What each row painted into the layer's scene.
    spans: Vec<(usize, Range<usize>)>,
}

/// What a `uniform_list` does with its rows this frame: nothing new, or
/// render the plan's rows into its layer.
pub(crate) struct Rows(Option<RowPlan>);

/// The rows to render into the layer of the list `id` showing the rows
/// `visible` of its `item_count`, with `overscan` rows around them, when
/// the rows the layer holds stay.
pub(crate) fn rows_to_render(
    window: &Window,
    id: &GlobalElementId,
    visible: Range<usize>,
    overscan: usize,
    item_count: usize,
) -> RowPlan {
    let needed = needed_rows(&visible, overscan, item_count);
    let held = window
        .fast_layers
        .layers
        .get(id)
        .map(|layer| &layer.rows.painted);
    let empty = BTreeSet::new();
    let held = held.unwrap_or(&empty);
    plan(held, needed)
}

/// The rows `visible` and `overscan` rows on each side, of `item_count`.
fn needed_rows(visible: &Range<usize>, overscan: usize, item_count: usize) -> Range<usize> {
    visible.start.saturating_sub(overscan).min(item_count)..(visible.end + overscan).min(item_count)
}

/// The rows of `needed` that `held` lacks, and those it holds.
fn plan(held: &BTreeSet<usize>, needed: Range<usize>) -> RowPlan {
    let mut render: Vec<Range<usize>> = Vec::new();
    let mut keep = needed.start..needed.start;
    for row in needed {
        if held.contains(&row) {
            if keep.is_empty() {
                keep = row..row + 1;
            } else {
                keep.end = row + 1;
            }
        } else {
            match render.last_mut() {
                Some(run) if run.end == row => run.end = row + 1,
                _ => render.push(row..row + 1),
            }
        }
    }
    RowPlan { render, keep }
}

/// The size of a uniform list's measured item: `measure`d, unless the
/// frame only scrolls the list's layer, whose content, the measured item
/// included, is then as it was (spec §8).
pub(crate) fn measure_item(
    window: &mut Window,
    cx: &mut App,
    id: Option<&GlobalElementId>,
    measure: impl FnOnce(&mut Window, &mut App) -> Size<Pixels>,
) -> Size<Pixels> {
    if !COMPILED || window.fast_layers.layers.is_empty() {
        return measure(window, cx);
    }
    let Some(id) = id else {
        return measure(window, cx);
    };
    if let Some(size) = kept_item_size(window, cx, id) {
        return size;
    }
    let size = measure(window, cx);
    let rem_size = window.rem_size();
    let text_style = window.text_style();
    if let Some(layer) = window.fast_layers.layers.get_mut(id) {
        layer.rows.measured = Some(Measured {
            size,
            rem_size,
            text_style,
        });
    }
    size
}

/// The measured item's size as the layer of the list `id` keeps it, if the
/// frame only scrolls the layer.
fn kept_item_size(window: &Window, cx: &App, id: &GlobalElementId) -> Option<Size<Pixels>> {
    if paint::inside_layer(window) || !active(window, cx) {
        return None;
    }
    let layer = window.fast_layers.layers.get(id)?;
    let record = layer.record.as_ref()?;
    let measured = layer.rows.measured.as_ref()?;
    if !layer.rows.list
        || measured.rem_size != window.rem_size()
        || measured.text_style != window.text_style()
    {
        return None;
    }
    #[cfg(any(test, feature = "test-support"))]
    if let Some(decision) = window.fast_layers.forced_decision {
        return (decision == Decision::Composite).then_some(measured.size);
    }
    invalidate::scroll_only(window, cx, id, record).then_some(measured.size)
}

/// `scroll_offset`, a uniform list's offset about to place its rows, moved
/// to whole device pixels where layers are compiled, so that rows painted
/// into a layer and rows drawn without one land on the same pixels. See
/// [`paint::snap_scroll_offset`].
pub(crate) fn snap_item_offset(window: &Window, scroll_offset: Point<Pixels>) -> Point<Pixels> {
    paint::snap_scroll_offset(window, scroll_offset)
}

/// Decides what the uniform list `id` does with its rows this frame, and
/// sets up rendering and prepainting them. The list's rows are
/// `item_height` tall, `item_count` of them from the top of
/// `padded_bounds`, scrolled by `scroll_offset`; it shows the rows
/// `visible`. A list flipped vertically keeps today's path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_uniform_list(
    window: &mut Window,
    cx: &mut App,
    id: Option<&GlobalElementId>,
    padded_bounds: Bounds<Pixels>,
    scroll_offset: Point<Pixels>,
    item_height: Pixels,
    item_count: usize,
    visible: &Range<usize>,
    y_flipped: bool,
) -> Rows {
    if !COMPILED || y_flipped || item_height <= Pixels::ZERO {
        return Rows(None);
    }
    let Some(id) = id else {
        return Rows(None);
    };
    if paint::inside_layer(window) || !active(window, cx) {
        return Rows(None);
    }
    let viewport = window.content_mask().bounds;
    let content_size = size(padded_bounds.size.width, item_height * item_count);
    let decision = policy::decide(window, cx, id, padded_bounds, content_size, scroll_offset);
    if decision == Decision::Bypass {
        return Rows(None);
    }
    let layer = paint::layer_mut(window, id);
    let extends = decision == Decision::Composite
        && layer.rows.list
        && layer
            .record
            .as_ref()
            .is_some_and(|record| record.scroll_offset.x == scroll_offset.x);
    let mode = if extends { Mode::Extend } else { Mode::Repaint };

    let overscan = (viewport.size.height / item_height).ceil().max(1.) as usize;
    let needed = needed_rows(visible, overscan, item_count);
    let plan = match mode {
        Mode::Extend => rows_to_render(window, id, visible.clone(), overscan, item_count),
        Mode::Repaint => RowPlan {
            render: vec![needed.clone()],
            keep: needed.start..needed.start,
        },
    };
    let slot = |row: usize| Bounds {
        origin: point(
            viewport.origin.x,
            padded_bounds.origin.y + scroll_offset.y + item_height * row,
        ),
        size: size(viewport.size.width, item_height),
    };
    let slots: BTreeMap<usize, Bounds<Pixels>> = plan
        .render
        .iter()
        .flat_map(|run| run.clone())
        .map(|row| (row, slot(row)))
        .collect();
    let mut painted_region = viewport;
    if !needed.is_empty() {
        painted_region = painted_region
            .union(&slot(needed.start))
            .union(&slot(needed.end - 1));
    }
    let translation = paint::translation(window, scroll_offset);
    let order = slots.keys().copied().collect();
    let frame = RowsFrame {
        mode,
        needed,
        keep: plan.keep.clone(),
        slots,
        viewport,
        painted_region,
        scroll_offset,
        translation,
        prepaint_start: window.prepaint_index(),
        prepaint_range: window.prepaint_index()..window.prepaint_index(),
        recording: Some(cx.begin_recording_dependencies()),
        dependencies: RenderDependencies::default(),
        order,
        next: 0,
        paint: None,
    };
    window.fast_layers.painting = Some(marker(id, &frame));
    paint::layer_mut(window, id).rows.frame = Some(frame);
    Rows(Some(plan))
}

/// What marks the rows of the list `id` as painting into its layer, for
/// nested views and scroll containers to tell.
fn marker(id: &GlobalElementId, frame: &RowsFrame) -> Painting {
    Painting {
        id: id.clone(),
        viewport: frame.viewport,
        painted_region: frame.painted_region,
        scene: Scene::default(),
        scroll_offset: frame.scroll_offset,
        translation: frame.translation,
        prepaint_range: frame.prepaint_start.clone()..frame.prepaint_start.clone(),
        recording: None,
        dependencies: RenderDependencies::default(),
    }
}

/// Renders the rows a uniform list renders this frame with `render`: those
/// it shows, `visible`, on today's path, and the rows its layer lacks
/// otherwise, in order.
pub(crate) fn render_rows(
    rows: &Rows,
    visible: Range<usize>,
    mut render: impl FnMut(Range<usize>) -> SmallVec<[AnyElement; 64]>,
) -> SmallVec<[AnyElement; 64]> {
    let Some(plan) = &rows.0 else {
        return render(visible);
    };
    let mut items = SmallVec::new();
    for run in &plan.render {
        items.extend(render(run.clone()));
    }
    items
}

/// The indices of the rows [`render_rows`] rendered, in order.
pub(crate) fn row_indices(rows: &Rows, visible: Range<usize>) -> RowIndices {
    match &rows.0 {
        None => RowIndices::Visible(visible),
        Some(plan) => RowIndices::Planned(
            plan.render
                .iter()
                .flat_map(|run| run.clone())
                .collect::<Vec<_>>()
                .into_iter(),
        ),
    }
}

/// See [`row_indices`].
pub(crate) enum RowIndices {
    Visible(Range<usize>),
    Planned(std::vec::IntoIter<usize>),
}

impl Iterator for RowIndices {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        match self {
            RowIndices::Visible(range) => range.next(),
            RowIndices::Planned(rows) => rows.next(),
        }
    }
}

/// Ends what [`begin_uniform_list`] began, once the rows are prepainted.
pub(crate) fn end_rows(window: &mut Window, cx: &mut App, rows: Rows) {
    if rows.0.is_none() {
        return;
    }
    let Some(painting) = window.fast_layers.painting.take() else {
        debug_assert!(false, "a list's rows ended without beginning");
        return;
    };
    finish_prepaint(window, cx, &painting.id);
}

/// Ends the prepaint of the rows of the list `id`: what it added to the
/// frame and read is the frame's.
fn finish_prepaint(window: &mut Window, cx: &mut App, id: &GlobalElementId) {
    let end = window.prepaint_index();
    let Some(frame) = frame_mut(window, id) else {
        return;
    };
    frame.prepaint_range = frame.prepaint_start.clone()..end;
    let recording = frame.recording.take();
    if let Some(recording) = recording {
        let dependencies = without_owner(window, cx.finish_recording_dependencies(recording).all);
        if let Some(frame) = frame_mut(window, id) {
            frame.dependencies = dependencies;
        }
    }
}

/// `dependencies`, read by a list's rows, without the view holding the
/// list: a list renders its rows as that view, whose own changes, and
/// whether it was notified for anything but a scroll, are judged apart (see
/// [`invalidate::scroll_only`]).
fn without_owner(window: &Window, dependencies: RenderDependencies) -> RenderDependencies {
    invalidate::without_entity(&dependencies, invalidate::owner_view(window))
        .unwrap_or(dependencies)
}

fn frame_mut<'a>(window: &'a mut Window, id: &GlobalElementId) -> Option<&'a mut RowsFrame> {
    window
        .fast_layers
        .layers
        .get_mut(id)
        .and_then(|layer| layer.rows.frame.as_mut())
}

/// Starts painting the rows of the list `id` as its prepaint decided.
pub(crate) fn begin_paint_rows(window: &mut Window, cx: &mut App, id: Option<&GlobalElementId>) {
    let Some(id) = id else {
        return;
    };
    if window.fast_layers.layers.is_empty() {
        return;
    }
    let Some(frame) = frame_mut(window, id) else {
        return;
    };
    if frame.paint.is_some() {
        return;
    }
    let painted_region = frame.painted_region;
    let has_rows = !frame.slots.is_empty();
    let background = if has_rows {
        paint::bake_background(window)
    } else {
        None
    };
    let mut state = PaintState {
        background,
        swapped: background.is_some(),
        scene: Scene::default(),
        paint_start: window.paint_index(),
        hovers_start: 0,
        recording: None,
        current: None,
        spans: Vec::new(),
    };
    if state.swapped {
        window.content_mask_stack.push(ContentMask {
            bounds: painted_region,
        });
        mem::swap(&mut window.next_frame.scene, &mut state.scene);
        state.paint_start = window.paint_index();
        window.take_hover_reads();
        state.hovers_start = window.retained_state.hover_dependencies.len();
        state.recording = Some(cx.begin_recording_dependencies());
    }
    let frame = frame_mut(window, id).unwrap();
    let marker = marker(id, frame);
    frame.paint = Some(state);
    window.fast_layers.painting = Some(marker);
}

/// Paints a row of a list with `f`: into the list's layer if its prepaint
/// rendered the row for it, not at all if the layer holds the row, into the
/// frame otherwise. `ix` is the row, or, for a uniform list, the next row it
/// rendered.
pub(crate) fn paint_row(
    window: &mut Window,
    cx: &mut App,
    ix: Option<usize>,
    f: impl FnOnce(&mut Window, &mut App),
) {
    let Some(id) = window.fast_layers.painting.as_ref().map(|p| p.id.clone()) else {
        return f(window, cx);
    };
    let Some(frame) = frame_mut(window, &id) else {
        return f(window, cx);
    };
    let Some(paint) = frame.paint.as_ref() else {
        return f(window, cx);
    };
    if paint.current.is_some() {
        // A row of a list nested in a row being painted.
        return f(window, cx);
    }
    let swapped = paint.swapped;
    let row = match ix {
        Some(row) => row,
        None => {
            let Some(row) = frame.order.get(frame.next).copied() else {
                return f(window, cx);
            };
            frame.next += 1;
            row
        }
    };
    if frame.mode == Mode::Extend && frame.keep.contains(&row) {
        // The layer holds the row as it is.
        return;
    }
    if !swapped {
        return f(window, cx);
    }
    if !frame.slots.contains_key(&row) {
        // Into the frame, around the layer's scene and painted region.
        let region = window.content_mask_stack.pop();
        swap_scenes(window, &id);
        f(window, cx);
        swap_scenes(window, &id);
        window.content_mask_stack.extend(region);
        return;
    }
    let start = window.next_frame.scene.paint_operations.len();
    set_current_row(window, &id, Some(row));
    f(window, cx);
    let end = window.next_frame.scene.paint_operations.len();
    set_current_row(window, &id, None);
    if let Some(paint) = frame_mut(window, &id).and_then(|frame| frame.paint.as_mut()) {
        paint.spans.push((row, start..end));
    }
}

fn swap_scenes(window: &mut Window, id: &GlobalElementId) {
    let Some(layer) = window.fast_layers.layers.get_mut(id) else {
        return;
    };
    if let Some(paint) = layer
        .rows
        .frame
        .as_mut()
        .and_then(|frame| frame.paint.as_mut())
    {
        mem::swap(&mut window.next_frame.scene, &mut paint.scene);
    }
}

fn set_current_row(window: &mut Window, id: &GlobalElementId, row: Option<usize>) {
    if let Some(paint) = frame_mut(window, id).and_then(|frame| frame.paint.as_mut()) {
        paint.current = row;
    }
}

/// Ends painting the rows of the list `id`: the rows painted into its layer
/// are recorded and the layer is composited.
pub(crate) fn end_paint_rows(window: &mut Window, cx: &mut App, id: Option<&GlobalElementId>) {
    let Some(id) = id else {
        return;
    };
    if window.fast_layers.layers.is_empty() {
        return;
    }
    let Some(layer) = window.fast_layers.layers.get_mut(id) else {
        return;
    };
    let Some(mut frame) = layer.rows.frame.take() else {
        return;
    };
    let Some(mut paint) = frame.paint.take() else {
        return;
    };
    window.fast_layers.painting = None;
    if frame.slots.is_empty() {
        match frame.mode {
            // Nothing new: the layer's content stands.
            Mode::Extend => paint::composite_at(window, id, frame.translation),
            // No row to paint: the layer holds nothing.
            Mode::Repaint => {
                if let Some(layer) = window.fast_layers.layers.get_mut(id) {
                    layer.record = None;
                }
            }
        }
        if window
            .fast_layers
            .layers
            .get(id)
            .is_none_or(|layer| layer.record.is_none())
        {
            clear_rows(window, id);
        }
        return;
    }
    let Some(background) = paint.background else {
        // The rows were painted into the frame: so is what the layer held.
        let layer = window.fast_layers.layers.get_mut(id).unwrap();
        if let Some(record) = layer.record.take()
            && frame.mode == Mode::Extend
        {
            paint::draw_into_frame(window, &record.content, frame.translation);
        }
        clear_rows(window, id);
        return;
    };

    let paint_dependencies = paint
        .recording
        .take()
        .map(|recording| without_owner(window, cx.finish_recording_dependencies(recording).all))
        .unwrap_or_default();
    window.take_hover_reads();
    let new_hovers = window.retained_state.hover_dependencies[paint.hovers_start..].to_vec();
    let paint_end = window.paint_index();
    mem::swap(&mut window.next_frame.scene, &mut paint.scene);
    window.content_mask_stack.pop();
    let painted = paint.scene;

    let scale_factor = window.scale_factor();
    let translation = frame.translation;
    let to_content = point(
        ScaledPixels(-translation.x.0),
        ScaledPixels(-translation.y.0),
    );
    let views = invalidate::content_views(window, &frame.prepaint_range);
    let viewport = window.snapped_content_mask().bounds;

    let layer = window.fast_layers.layers.get_mut(id).unwrap();
    let rows = &mut layer.rows;
    match frame.mode {
        Mode::Repaint => rows.clear(),
        Mode::Extend => {
            let needed = frame.needed.clone();
            rows.rows.retain(|row, _| needed.contains(row));
            rows.painted.retain(|row| needed.contains(row));
            rows.row_origins.retain(|row, _| needed.contains(row));
        }
    }
    let mut operations: Vec<Option<PaintOperation>> =
        painted.paint_operations.into_iter().map(Some).collect();
    for (row, span) in paint.spans {
        let Some(slot) = frame.slots.get(&row) else {
            continue;
        };
        let slot = slot.scale(scale_factor);
        let slot = Bounds {
            origin: slot.origin + to_content,
            size: slot.size,
        };
        let row_operations = operations[span]
            .iter_mut()
            .filter_map(Option::take)
            .map(|operation| match operation {
                PaintOperation::Primitive(primitive) => {
                    PaintOperation::Primitive(translate_primitive(&primitive, to_content))
                }
                PaintOperation::StartLayer(bounds) => PaintOperation::StartLayer(Bounds {
                    origin: bounds.origin + to_content,
                    size: bounds.size,
                }),
                PaintOperation::EndLayer => PaintOperation::EndLayer,
            })
            .collect();
        rows.painted.insert(row);
        rows.row_origins.insert(
            row,
            point(
                px(slot.origin.x.0 / scale_factor),
                px(slot.origin.y.0 / scale_factor),
            ),
        );
        rows.rows.insert(
            row,
            Row {
                slot,
                operations: row_operations,
            },
        );
    }
    rows.list = true;

    // The content: the rows in order.
    let mut content = Scene::default();
    let mut region = Bounds {
        origin: viewport.origin + to_content,
        size: viewport.size,
    };
    for row in rows.rows.values() {
        region = region.union(&row.slot);
        for operation in &row.operations {
            match operation {
                PaintOperation::Primitive(primitive) => content.insert_primitive(primitive.clone()),
                PaintOperation::StartLayer(bounds) => content.push_layer(*bounds),
                PaintOperation::EndLayer => content.pop_layer(),
            }
        }
    }
    content.finish();
    let has_paths = !content.paths.is_empty();
    let hashes = tile_hashes(&content, paint::TILE_SIZE, region);

    let old = layer.record.take();
    let (generation, dirty) = match &old {
        Some(old) if old.background == background => {
            (old.generation + 1, dirty_tiles(&old.tile_hashes, &hashes))
        }
        old => (
            old.as_ref().map_or(1, |old| old.generation + 1),
            paint::all_tiles(&hashes),
        ),
    };
    let paint_range = paint.paint_start..paint_end;
    let (dependencies, hovers, views) = match (&old, frame.mode) {
        (Some(old), Mode::Extend) => {
            let mut hovers = old.hovers.to_vec();
            hovers.extend(new_hovers);
            let mut all_views = old.views.to_vec();
            all_views.extend(views.iter().copied());
            (
                old.dependencies
                    .union(&frame.dependencies)
                    .union(&paint_dependencies),
                hovers.into(),
                all_views.into(),
            )
        }
        _ => (
            frame.dependencies.union(&paint_dependencies),
            new_hovers.into(),
            views,
        ),
    };
    let painted_region = rows.rows.values().fold(frame.viewport, |region, row| {
        let slot = Bounds {
            origin: row.slot.origin - to_content,
            size: row.slot.size,
        };
        region.union(&Bounds {
            origin: point(
                px(slot.origin.x.0 / scale_factor),
                px(slot.origin.y.0 / scale_factor),
            ),
            size: crate::size(
                px(slot.size.width.0 / scale_factor),
                px(slot.size.height.0 / scale_factor),
            ),
        })
    });
    let dirtied = dirty.len();
    layer.record = Some(LayerRecord {
        content: Rc::new(content),
        generation,
        painted_region,
        viewport: frame.viewport,
        scroll_offset: frame.scroll_offset,
        translation,
        prepaint_range: frame.prepaint_range,
        paint_range,
        tile_hashes: hashes,
        dirty_tiles: dirty,
        background,
        hovers,
        dependencies,
        views,
        has_paths,
    });
    if frame.mode == Mode::Repaint && !has_paths {
        window
            .layout_engine
            .as_mut()
            .unwrap()
            .retention
            .stats
            .layer_frames_repainted += 1;
    }
    paint::insert_layer(window, id, translation, dirtied);
}

/// Forgets the rows of the layer of the list `id`, whose content is gone.
fn clear_rows(window: &mut Window, id: &GlobalElementId) {
    if let Some(layer) = window.fast_layers.layers.get_mut(id) {
        layer.rows.clear();
    }
}
