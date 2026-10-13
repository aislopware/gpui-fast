//! Native placements in the scene, the holes they cut in GPUI's drawable, and
//! the batches a renderer draws them in.

use std::ops::Range;

use crate::{
    Background, BorderStyle, Bounds, ContentMask, Corners, DrawOrder, Edges, FocusId, HitboxId,
    PaintOperation, PrimitiveBatch, Quad, ScaledPixels, Scene, black,
};

use super::NativeId;

/// Where a native shows in one frame: painted into the scene at its place in
/// painter's order, replayed with the primitives around it.
#[derive(Clone, Debug, PartialEq)]
pub struct NativePlacement {
    /// Its draw order among the scene's primitives, like theirs.
    pub order: DrawOrder,
    /// The native placed.
    pub id: NativeId,
    /// The hitbox the native takes the pointer through, if it does.
    pub hitbox: Option<HitboxId>,
    /// The focus handle standing for the native's keyboard focus, if any.
    pub focus: Option<FocusId>,
    /// The element's bounds, which the native's content fills.
    pub bounds: Bounds<ScaledPixels>,
    /// What of it shows.
    pub content_mask: ContentMask<ScaledPixels>,
    /// The corners GPUI rounds it with.
    pub corner_radii: Corners<ScaledPixels>,
    /// How opaque it is drawn, from the element opacity it was painted with.
    pub opacity: f32,
}

impl NativePlacement {
    /// The part of the placement that can show.
    pub fn clipped_bounds(&self) -> Bounds<ScaledPixels> {
        self.bounds.intersect(&self.content_mask.bounds)
    }

    /// The hole this placement cuts: the region GPUI's drawable is cleared in,
    /// by coverage × opacity, so the native under the drawable shows there.
    /// It is drawn with the quad shader and a blend that keeps
    /// `destination × (1 − source alpha)`.
    fn hole(&self) -> Quad {
        Quad {
            order: self.order,
            border_style: BorderStyle::Solid,
            bounds: self.bounds,
            content_mask: self.content_mask,
            background: Background::from(black().opacity(self.opacity)),
            border_color: black().opacity(0.),
            corner_radii: self.corner_radii,
            border_widths: Edges::default(),
        }
    }
}

/// The natives a scene places. Both lists are in draw order once the scene is
/// finished.
#[derive(Default)]
pub struct SceneComposition {
    /// Every placement painted this frame.
    pub placements: Vec<NativePlacement>,
    /// The hole each placement cuts, drawn by a renderer as quads that clear
    /// what is under them.
    pub holes: Vec<Quad>,
    /// Every backdrop blur painted this frame (`fast::backdrop`).
    pub blurs: Vec<crate::fast::backdrop::BackdropBlur>,
}

impl SceneComposition {
    pub(crate) fn sort(&mut self) {
        self.placements.sort_by_key(|placement| placement.order);
        self.holes.sort_by_key(|hole| hole.order);
        self.blurs.sort_by_key(|blur| blur.order);
    }
}

/// Moves the rest of the innermost open paint layer of `scene`, if any,
/// above what was just placed in it: a native or a backdrop blur, which the
/// layer's primitives painted after it are drawn over.
pub(crate) fn lift_open_layer(scene: &mut Scene) {
    if !scene.layer_stack.is_empty()
        && let Some(layer_bounds) = scene.open_layer_bounds()
    {
        let rest_of_layer = scene.primitive_bounds.insert(layer_bounds);
        if let Some(order) = scene.layer_stack.last_mut() {
            *order = rest_of_layer;
        }
    }
}

/// Forgets the natives `scene` placed, as [`Scene::clear`] forgets its primitives.
pub(crate) fn clear(scene: &mut Scene) {
    scene.composition.placements.clear();
    scene.composition.holes.clear();
    scene.composition.blurs.clear();
}

/// Places a native again as the last frame placed it, as [`Scene::replay`] does.
pub(crate) fn replay(scene: &mut Scene, placement: &NativePlacement) {
    scene.insert_native(placement.clone());
}

/// A batch a renderer draws: GPUI's primitives, or the holes natives cut.
#[derive(Debug)]
pub enum ComposedBatch {
    /// Primitives, as [`Scene::batches`] yields them.
    Primitives(PrimitiveBatch),
    /// Holes: `scene.natives().holes[range]`, drawn with a blend keeping
    /// `destination × (1 − source alpha)` in colour and alpha alike.
    Holes(Range<usize>),
    /// A backdrop blur: `scene.composition.blurs[index]`, drawn over what
    /// the batches before it drew (see `fast::backdrop`).
    Blur(usize),
}

impl Scene {
    /// Places a native: it takes the next draw order over what it overlaps,
    /// and cuts its hole there. Inside a paint layer, whose primitives share
    /// one order, the rest of the layer is moved above the native.
    pub fn insert_native(&mut self, mut placement: NativePlacement) {
        let clipped_bounds = placement.clipped_bounds();
        if clipped_bounds.is_empty() || placement.opacity <= 0. {
            return;
        }
        placement.order = self.primitive_bounds.insert(clipped_bounds);
        lift_open_layer(self);
        self.composition.holes.push(placement.hole());
        self.composition.placements.push(placement.clone());
        crate::fast::scene::push(self, PaintOperation::Native(Box::new(placement)));
    }

    /// The bounds of the innermost paint layer still open.
    fn open_layer_bounds(&self) -> Option<Bounds<ScaledPixels>> {
        let mut depth = 0usize;
        for operation in self.paint_operations.iter().rev() {
            match operation {
                PaintOperation::EndLayer => depth += 1,
                PaintOperation::StartLayer(bounds) => {
                    if depth == 0 {
                        return Some(*bounds);
                    }
                    depth -= 1;
                }
                PaintOperation::Primitive(_)
                | PaintOperation::Native(_)
                | PaintOperation::Backdrop(_) => {}
            }
        }
        None
    }

    /// The batches to draw this finished scene in: [`Scene::batches`] with the
    /// holes natives cut interleaved at their draw orders. A primitive batch
    /// spanning a hole's order is split around it.
    pub fn composed_batches(&self) -> impl Iterator<Item = ComposedBatch> + '_ {
        ComposedBatches {
            scene: self,
            batches: self.batches(),
            pending: None,
            next_hole: 0,
            next_blur: 0,
        }
    }

    /// Whether any primitive or hole is drawn at an order above `order`.
    pub(crate) fn draws_above(&self, order: DrawOrder) -> bool {
        let above = |last: Option<DrawOrder>| last.is_some_and(|last| last > order);
        above(self.shadows.last().map(|primitive| primitive.order))
            || above(self.quads.last().map(|primitive| primitive.order))
            || above(self.paths.last().map(|primitive| primitive.order))
            || above(self.underlines.last().map(|primitive| primitive.order))
            || above(
                self.monochrome_sprites
                    .last()
                    .map(|primitive| primitive.order),
            )
            || above(
                self.subpixel_sprites
                    .last()
                    .map(|primitive| primitive.order),
            )
            || above(
                self.polychrome_sprites
                    .last()
                    .map(|primitive| primitive.order),
            )
            || above(self.surfaces.last().map(|primitive| primitive.order))
            || above(self.composition.holes.last().map(|hole| hole.order))
            || above(self.composition.blurs.last().map(|blur| blur.order))
    }
}

struct ComposedBatches<'a, I> {
    scene: &'a Scene,
    batches: I,
    /// What is left of a primitive batch split around a hole or a blur.
    pending: Option<PrimitiveBatch>,
    next_hole: usize,
    next_blur: usize,
}

impl<I: Iterator<Item = PrimitiveBatch>> ComposedBatches<'_, I> {
    /// The order of the next hole, or of the next blur when it comes first,
    /// and whether it is the blur.
    fn next_cut(&self) -> Option<(DrawOrder, bool)> {
        let hole = self
            .scene
            .composition
            .holes
            .get(self.next_hole)
            .map(|hole| hole.order);
        let blur = self
            .scene
            .composition
            .blurs
            .get(self.next_blur)
            .map(|blur| blur.order);
        match (hole, blur) {
            (Some(hole), Some(blur)) if blur < hole => Some((blur, true)),
            (Some(hole), _) => Some((hole, false)),
            (None, Some(blur)) => Some((blur, true)),
            (None, None) => None,
        }
    }

    /// The holes from the next, up to the next blur and those `goes_first`
    /// says are not drawn after `first_order`.
    fn holes(
        &mut self,
        first_order: Option<DrawOrder>,
        goes_first: impl Fn(DrawOrder, DrawOrder) -> bool,
    ) -> ComposedBatch {
        let holes = &self.scene.composition.holes;
        let blur = self
            .scene
            .composition
            .blurs
            .get(self.next_blur)
            .map(|blur| blur.order);
        let start = self.next_hole;
        let end = start
            + holes[start..]
                .iter()
                .take_while(|hole| {
                    blur.is_none_or(|blur| hole.order < blur)
                        && first_order.is_none_or(|first| !goes_first(first, hole.order))
                })
                .count();
        self.next_hole = end;
        ComposedBatch::Holes(start..end)
    }
}

impl<I: Iterator<Item = PrimitiveBatch>> Iterator for ComposedBatches<'_, I> {
    type Item = ComposedBatch;

    fn next(&mut self) -> Option<ComposedBatch> {
        let Some(batch) = self.pending.take().or_else(|| self.batches.next()) else {
            // What is left after every primitive: the remaining holes and
            // blurs, in order.
            let (_, blur) = self.next_cut()?;
            if blur {
                self.next_blur += 1;
                return Some(ComposedBatch::Blur(self.next_blur - 1));
            }
            return Some(self.holes(None, |_, _| false));
        };
        let Some((cut_order, blur)) = self.next_cut() else {
            return Some(ComposedBatch::Primitives(batch));
        };
        let order_at = |index| batch_order(self.scene, &batch, index);
        let len = batch_len(&batch);
        // A hole is drawn as its native would be as a quad: after the shadows of its own
        // order, which draw beyond their bounds, and before everything else of its order.
        // A blur is drawn after everything ordered below it and before everything else.
        let shadows = matches!(batch, PrimitiveBatch::Shadows(_));
        let goes_first = move |order: DrawOrder, cut: DrawOrder| {
            order < cut || (!blur && shadows && order == cut)
        };
        let below = partition_point(len, |index| goes_first(order_at(index), cut_order));
        if below == len {
            return Some(ComposedBatch::Primitives(batch));
        }
        let first_order = order_at(0);
        if below > 0 {
            let (first, rest) = split_batch(batch, below);
            self.pending = Some(rest);
            return Some(ComposedBatch::Primitives(first));
        }
        self.pending = Some(batch);
        if blur {
            self.next_blur += 1;
            return Some(ComposedBatch::Blur(self.next_blur - 1));
        }
        Some(self.holes(Some(first_order), goes_first))
    }
}

/// The first index in `0..len` for which `pred` is false, where `pred` is
/// true for a prefix.
fn partition_point(len: usize, pred: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, len);
    while low < high {
        let middle = low + (high - low) / 2;
        if pred(middle) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

fn batch_len(batch: &PrimitiveBatch) -> usize {
    match batch {
        PrimitiveBatch::Shadows(range)
        | PrimitiveBatch::Quads(range)
        | PrimitiveBatch::Paths(range)
        | PrimitiveBatch::Underlines(range)
        | PrimitiveBatch::MonochromeSprites { range, .. }
        | PrimitiveBatch::SubpixelSprites { range, .. }
        | PrimitiveBatch::PolychromeSprites { range, .. }
        | PrimitiveBatch::Surfaces(range) => range.len(),
    }
}

/// The draw order of a batch's `index`th primitive. A batch's primitives are
/// in draw order.
fn batch_order(scene: &Scene, batch: &PrimitiveBatch, index: usize) -> DrawOrder {
    match batch {
        PrimitiveBatch::Shadows(range) => scene.shadows[range.start + index].order,
        PrimitiveBatch::Quads(range) => scene.quads[range.start + index].order,
        PrimitiveBatch::Paths(range) => scene.paths[range.start + index].order,
        PrimitiveBatch::Underlines(range) => scene.underlines[range.start + index].order,
        PrimitiveBatch::MonochromeSprites { range, .. } => {
            scene.monochrome_sprites[range.start + index].order
        }
        PrimitiveBatch::SubpixelSprites { range, .. } => {
            scene.subpixel_sprites[range.start + index].order
        }
        PrimitiveBatch::PolychromeSprites { range, .. } => {
            scene.polychrome_sprites[range.start + index].order
        }
        PrimitiveBatch::Surfaces(range) => scene.surfaces[range.start + index].order,
    }
}

/// Splits a batch after its first `count` primitives.
fn split_batch(batch: PrimitiveBatch, count: usize) -> (PrimitiveBatch, PrimitiveBatch) {
    fn split(range: Range<usize>, count: usize) -> (Range<usize>, Range<usize>) {
        let middle = range.start + count;
        (range.start..middle, middle..range.end)
    }
    match batch {
        PrimitiveBatch::Shadows(range) => {
            let (a, b) = split(range, count);
            (PrimitiveBatch::Shadows(a), PrimitiveBatch::Shadows(b))
        }
        PrimitiveBatch::Quads(range) => {
            let (a, b) = split(range, count);
            (PrimitiveBatch::Quads(a), PrimitiveBatch::Quads(b))
        }
        PrimitiveBatch::Paths(range) => {
            let (a, b) = split(range, count);
            (PrimitiveBatch::Paths(a), PrimitiveBatch::Paths(b))
        }
        PrimitiveBatch::Underlines(range) => {
            let (a, b) = split(range, count);
            (PrimitiveBatch::Underlines(a), PrimitiveBatch::Underlines(b))
        }
        PrimitiveBatch::MonochromeSprites { texture_id, range } => {
            let (a, b) = split(range, count);
            (
                PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    range: a,
                },
                PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    range: b,
                },
            )
        }
        PrimitiveBatch::SubpixelSprites { texture_id, range } => {
            let (a, b) = split(range, count);
            (
                PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    range: a,
                },
                PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    range: b,
                },
            )
        }
        PrimitiveBatch::PolychromeSprites { texture_id, range } => {
            let (a, b) = split(range, count);
            (
                PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    range: a,
                },
                PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    range: b,
                },
            )
        }
        PrimitiveBatch::Surfaces(range) => {
            let (a, b) = split(range, count);
            (PrimitiveBatch::Surfaces(a), PrimitiveBatch::Surfaces(b))
        }
    }
}
