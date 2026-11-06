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
    /// Every layer mask's cuts painted this frame (`fast::layer_mask`).
    pub masks: Vec<crate::fast::layer_mask::LayerMaskCut>,
    /// The begins of the masks still open, innermost last.
    pub(crate) open_masks: Vec<crate::fast::layer_mask::LayerMaskCut>,
    /// How many masks were begun this frame: the next one's id.
    pub(crate) mask_ids: u32,
}

impl SceneComposition {
    pub(crate) fn sort(&mut self) {
        self.placements.sort_by_key(|placement| placement.order);
        self.holes.sort_by_key(|hole| hole.order);
        self.blurs.sort_by_key(|blur| blur.order);
        crate::fast::layer_mask::sort(&mut self.masks, &mut self.open_masks);
    }
}

/// Moves the rest of the innermost open paint layer of `scene`, if any,
/// above what was just placed in it: a native, a backdrop blur or a layer
/// mask's cut, which the layer's primitives painted after it are drawn over.
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
    scene.composition.masks.clear();
    scene.composition.open_masks.clear();
    scene.composition.mask_ids = 0;
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
    /// Backdrop blurs: `scene.backdrop_blurs()[range]`, of one draw order,
    /// drawn over what the batches before them drew, after one copy of it
    /// (see `fast::backdrop`).
    Blurs(Range<usize>),
    /// Layer masks' cuts: `scene.layer_masks()[range]`, of one draw order,
    /// whose regions are apart: each begin copies what the batches before
    /// it drew in its region and clears it, each end draws the group back
    /// over that copy through its mask (see `fast::layer_mask`).
    Masks(Range<usize>),
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
                | PaintOperation::Backdrop(_)
                | PaintOperation::LayerMask(_) => {}
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
            next_mask: 0,
        }
    }

    /// Whether any primitive, hole, blur or mask is drawn at an order above
    /// `order`.
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
            || above(self.composition.masks.last().map(|cut| cut.order))
    }
}

struct ComposedBatches<'a, I> {
    scene: &'a Scene,
    batches: I,
    /// What is left of a primitive batch split around a hole or a blur.
    pending: Option<PrimitiveBatch>,
    next_hole: usize,
    next_blur: usize,
    next_mask: usize,
}

/// What a primitive batch is split around. At one draw order, among cuts
/// drawn after as many quads of it, a mask's cut goes first, then the blurs,
/// then the holes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Cut {
    Mask,
    Blur,
    Hole,
}

impl<I: Iterator<Item = PrimitiveBatch>> ComposedBatches<'_, I> {
    /// The order of the next cut, and what it is.
    fn next_cut(&self) -> Option<(DrawOrder, Cut)> {
        let composition = &self.scene.composition;
        // Each with how many quads of its order are drawn before it.
        let mask = (composition.masks.get(self.next_mask)).map(|cut| (cut.order, cut.quads_before));
        let blur =
            (composition.blurs.get(self.next_blur)).map(|blur| (blur.order, blur.quads_before));
        let hole = composition
            .holes
            .get(self.next_hole)
            .map(|hole| (hole.order, 0));
        [(mask, Cut::Mask), (blur, Cut::Blur), (hole, Cut::Hole)]
            .into_iter()
            .filter_map(|(at, cut)| Some((at?, cut)))
            .min()
            .map(|((order, _), cut)| (order, cut))
    }

    /// The cut batch `cut` names, from the next of its kind.
    fn cut(&mut self, cut: Cut, first_order: Option<DrawOrder>, shadows: bool) -> ComposedBatch {
        match cut {
            Cut::Mask => self.masks(),
            Cut::Blur => self.blurs(),
            Cut::Hole => self.holes(first_order, move |order, hole| {
                order < hole || (shadows && order == hole)
            }),
        }
    }

    /// The masks' cuts from the next of its draw order.
    fn masks(&mut self) -> ComposedBatch {
        let masks = &self.scene.composition.masks;
        let start = self.next_mask;
        let (order, quads_before) = (masks[start].order, masks[start].quads_before);
        let end = start
            + masks[start..]
                .iter()
                .take_while(|cut| cut.order == order && cut.quads_before == quads_before)
                .count();
        self.next_mask = end;
        ComposedBatch::Masks(start..end)
    }

    /// The blurs from the next of its draw order drawn after as many of that
    /// order's quads.
    fn blurs(&mut self) -> ComposedBatch {
        let blurs = &self.scene.composition.blurs;
        let start = self.next_blur;
        let (order, quads_before) = (blurs[start].order, blurs[start].quads_before);
        let end = start
            + blurs[start..]
                .iter()
                .take_while(|blur| blur.order == order && blur.quads_before == quads_before)
                .count();
        self.next_blur = end;
        ComposedBatch::Blurs(start..end)
    }

    /// The holes from the next, up to the next blur or mask and those
    /// `goes_first` says are not drawn after `first_order`.
    fn holes(
        &mut self,
        first_order: Option<DrawOrder>,
        goes_first: impl Fn(DrawOrder, DrawOrder) -> bool,
    ) -> ComposedBatch {
        let composition = &self.scene.composition;
        let holes = &composition.holes;
        // A hole goes before a blur or a mask's cut of its order only when quads of that
        // order are drawn before the cut, as the hole goes before them.
        let blur =
            (composition.blurs.get(self.next_blur)).map(|blur| (blur.order, blur.quads_before));
        let mask = (composition.masks.get(self.next_mask)).map(|cut| (cut.order, cut.quads_before));
        let start = self.next_hole;
        let end = start
            + holes[start..]
                .iter()
                .take_while(|hole| {
                    blur.is_none_or(|blur| (hole.order, 0) < blur)
                        && mask.is_none_or(|mask| (hole.order, 0) < mask)
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
            // What is left after every primitive: the remaining cuts, in
            // order.
            let (_, cut) = self.next_cut()?;
            return Some(self.cut(cut, None, false));
        };
        let Some((cut_order, cut)) = self.next_cut() else {
            return Some(ComposedBatch::Primitives(batch));
        };
        let order_at = |index| batch_order(self.scene, &batch, index);
        let len = batch_len(&batch);
        // A hole is drawn as its native would be as a quad: after the shadows of its own
        // order, which draw beyond their bounds, and before everything else of its order.
        // A blur or a mask's cut is drawn as monocode drew its marker quad: after the shadows
        // of its order and the quads of its order painted before it, and before everything
        // else of its order.
        let shadows = matches!(batch, PrimitiveBatch::Shadows(_));
        let goes_first =
            move |order: DrawOrder, cut: DrawOrder| order < cut || (shadows && order == cut);
        // The quads of the cut's order drawn before it end at this index of the scene's quads.
        let composition = &self.scene.composition;
        let quads_end = match (&batch, cut) {
            (PrimitiveBatch::Quads(range), Cut::Blur | Cut::Mask) => {
                let before = match cut {
                    Cut::Blur => composition.blurs[self.next_blur].quads_before,
                    _ => composition.masks[self.next_mask].quads_before,
                };
                let first = self
                    .scene
                    .quads
                    .partition_point(|quad| quad.order < cut_order);
                Some((range.start, first + before as usize))
            }
            _ => None,
        };
        let below = partition_point(len, |index| {
            goes_first(order_at(index), cut_order)
                || quads_end
                    .is_some_and(|(start, end)| order_at(index) == cut_order && start + index < end)
        });
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
        Some(self.cut(cut, Some(first_order), shadows))
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
