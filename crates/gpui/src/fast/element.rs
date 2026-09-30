//! Elements drawn again from what they drew on the last frame.
//!
//! A view that changed renders again and builds every element it holds anew,
//! though most of them are usually built just as they were: a quote board
//! whose price changed in one row builds every other row as it did. Those
//! elements are still built, but once built, each is compared with the
//! element built at its place last frame; one built the same way, in every
//! element nested in it too, is drawn again from what it drew then, as a
//! view whose dependencies did not change is (see [`crate::fast::retained`]):
//! its layout nodes are kept rather than requested, and its hitboxes,
//! dispatch nodes and primitives are copied from last frame's.
//!
//! Only elements whose output is fully decided by what they were built with
//! and where they are drawn take part: a `div` with a style and children but
//! no listener, focus, scroll, hover or other interactive state, and plain
//! text. What they were built with is compared exactly, a `div`'s style
//! refinement with the one kept from last frame, text with its text; where
//! they are drawn — bounds, content mask, opacity, text style and rem size —
//! is compared too. Nothing they draw can depend on anything else, so they
//! read nothing a view would have to depend on.
//!
//! An element is found again by its layout key: its path from the root of
//! the element tree, each step an `ElementId` or a position among siblings,
//! as its layout node is (see [`crate::fast::layout_key`]). An element that
//! differs is built as upstream builds it, but the elements nested in it are
//! each compared on their own, so a row whose price changed builds the row
//! and the price, and draws the other cells of the row from last frame.
//!
//! Each element drawn this way leaves a record. The records of an element
//! nested in no other that does — a row, say — and of those nested in it
//! are frozen, once it is painted, into one [`Subtree`] shared from frame to
//! frame, their ranges relative to its own. Drawing the row again from last
//! frame takes that subtree over as it is, however many elements it holds;
//! only elements drawn again inside one built this frame copy their records.

use crate::fast::layout_key::{KeyPosition, key_position, pop_layout_key, push_layout_key};
use crate::fast::shift::{
    Masks, Noted, Shift, ShiftedOperation, clear_of_zero, known_inside, replay_shifted,
    shift_operations, still_beyond,
};
use crate::text_system::LineLayoutIndex;
use crate::window::{PaintIndex, PrepaintStateIndex};
use crate::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Div, Drawable, Element, ElementId,
    HitboxBehavior, Interactivity, LayoutId, Overflow, Pixels, Point, ScaledPixels, Scene,
    SharedString, Size, Stateful, StyleRefinement, Svg, Text, TextStyle, Transformation, Window,
    px,
};
use collections::FxHashMap;
use std::{
    any::{Any, TypeId},
    cell::OnceCell,
    mem,
    ops::Range,
    rc::Rc,
};

impl Window {
    /// Sets whether an element built this frame as it was built on the last
    /// one is drawn again from what it drew then, which is the default. It
    /// takes view retention to be on as well (see
    /// [`Window::set_view_retention`]): with that off, every element is drawn
    /// from scratch each frame, as upstream GPUI draws it.
    ///
    /// The `GPUI_ELEMENT_RETENTION=0` environment variable turns it off for
    /// every window.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_element_retention(&mut self, enabled: bool) {
        if self.retained_state.element_retention != enabled {
            self.retained_state.element_retention = enabled;
            self.refresh();
        }
    }
}

/// The records of the elements drawn in one frame, for the next one to draw
/// them again from. Kept in the frame's
/// [`crate::fast::retained::RetainedSubtrees`].
#[derive(Default)]
pub(crate) struct ElementRecords {
    /// A record per element nested in no other element with one, in the
    /// order they began prepainting.
    roots: Vec<Root>,
    by_key: FxHashMap<u64, u32>,
    /// The records of the root being prepainted, its own first, each
    /// followed by those nested in it, with ranges in this frame.
    building: Vec<ElementRecord>,
    /// The root whose records `building` holds.
    building_root: u32,
    /// The records in `building` whose prepaint is under way, innermost
    /// last, each with `reused` as it was when it began.
    open: Vec<(u32, u32)>,
    /// How many times elements were drawn again inside ones being built,
    /// for a record to tell whether anything nested in it was.
    reused: u32,
    /// How the elements drawn again moved this frame did, for their paint.
    shifts: Vec<Shifted>,
    /// What they paint, moved, each one's in a range of it.
    shifted: Vec<ShiftedOperation>,
}

/// An element drawn again moved, for its paint.
struct Shifted {
    by: Point<Pixels>,
    /// Where what it paints, moved, is in [`ElementRecords::shifted`].
    operations: Range<u32>,
}

/// An element nested in no other element with a record, and where its
/// subtree of records was drawn.
struct Root {
    key: u64,
    records: RootRecords,
    /// For a root left without records, where it was drawn. See
    /// [`Placement`].
    placement: Option<Placement>,
    prepaint_start: PrepaintStateIndex,
    /// Where its paint started, or, while [`Paint::Pending`], where it
    /// started in the frame it is copied from.
    paint_start: PaintIndex,
    paint: Paint,
    /// Whether it is the only root under its key.
    usable: bool,
    /// Whether its records were built this frame, rather than taken over
    /// from an earlier one, so that the rests of those nested in it are
    /// this frame's.
    fresh: bool,
    rest: Rest,
    /// For how many frames more its elements note what they paint, for
    /// them to be drawn again moved (see [`crate::fast::shift::Noting`]):
    /// [`IN_MOTION`] once it or an element nested in it moved, one fewer each
    /// frame it holds still.
    motion: u8,
}

/// For how many frames a root that moved keeps noting what it paints.
const IN_MOTION: u8 = 30;

/// Where a root left without records was drawn, and whether that is where
/// it was drawn the frame before.
///
/// A root starts out drawn as upstream draws it, with everything nested in
/// it, and is recorded only once it is drawn twice in a row where it was:
/// rows under a scroll, or below rows inserted above them, are drawn
/// somewhere else every frame and could never be drawn again from the last
/// one, so they are not compared, kept and built again every frame for
/// nothing. A recorded root found to have moved goes back to that.
///
/// One moved by a whole number of device pixels can be drawn again moved
/// (see [`crate::fast::shift`]), and counts as drawn where it was the second
/// frame in a row it moves so: a row under a scroll is recorded and drawn
/// again moved from then on, while rows moved once, below a line inserted
/// above them, are not recorded for a move they will not make again.
#[derive(Clone, Copy)]
struct Placement {
    bounds: Bounds<Pixels>,
    still: bool,
    /// Whether it moved there by a whole number of device pixels.
    moved: bool,
}

/// Where a root left without records was drawn last frame, and whether it
/// had moved there by a whole number of device pixels. See [`Placement`].
#[derive(Clone, Copy)]
struct Placed {
    bounds: Bounds<Pixels>,
    moved: bool,
}

/// Whether an element nested in no other with a record is recorded this
/// frame. See [`Placement`].
enum Probation {
    /// It was recorded last frame, or stood still, where it was drawn then,
    /// and does not rest.
    Record(Bounds<Pixels>, Rest),
    /// It moved, or was not drawn, last frame, or it rests: drawn as
    /// upstream draws it, its placement noted, where it was placed last
    /// frame if it was, with the rest it leaves.
    Skip(Option<Placed>, Rest),
}

/// The records of a root and those nested in it.
enum RootRecords {
    /// Being prepainted, in `building`.
    Building,
    /// Prepainted, with prepaint ranges relative to the root's, and paint
    /// ranges in this frame as they are painted.
    Pending(Box<[ElementRecord]>),
    /// Painted.
    Frozen(Rc<Subtree>),
    /// Never painted, or carried along with something drawn again from last
    /// frame without having been painted there, or not recorded while it
    /// skips being compared.
    Lost,
}

/// The records of an element and of the elements nested in it, its own
/// first, each followed by those nested in it, with prepaint and paint
/// ranges relative to its own. Shared by every frame that draws it again.
struct Subtree {
    records: Box<[ElementRecord]>,
    /// The records by key, for an element whose place among its siblings
    /// changed, worked out the first time one is looked for.
    by_key: OnceCell<FxHashMap<u64, u32>>,
}

impl Subtree {
    fn find(&self, key: u64) -> Option<u32> {
        self.by_key
            .get_or_init(|| {
                self.records
                    .iter()
                    .enumerate()
                    .map(|(index, record)| (record.key, index as u32))
                    .collect()
            })
            .get(&key)
            .copied()
    }
}

#[derive(Clone)]
struct ElementRecord {
    key: u64,
    snapshot: Snapshot,
    /// How many of the records following this one are nested inside it.
    nested: u32,
    /// Whether every child of its element left a record, in order, among
    /// the nested ones: none went unprepainted under a `display: none`.
    complete: bool,
    /// Whether it is complete, everything nested in it recorded, and its
    /// layout claimed the nodes of these records and no others, so that it
    /// can be drawn again from them.
    reusable: bool,
    paint: Paint,
    /// For [`Snapshot::Moving`], whether it was drawn where it was the frame
    /// before.
    still: bool,
    /// For a record nested in another; a root's is its [`Root`]'s.
    rest: Rest,
    /// What painting it noted besides what it painted: where it placed
    /// glyphs, and what it left out for lying outside a mask, which a move
    /// could bring inside it.
    noted: Noted,
    layout_id: LayoutId,
    /// While it is being built, how many layout nodes it claimed.
    claimed: u32,
    context: ElementContext,
    /// Relative to its root's prepaint.
    prepaint_range: Range<PrepaintAt>,
    /// Relative to its root's paint, or, while [`Paint::Pending`], to the
    /// paint of the root of last frame it is drawn again from.
    paint_range: Range<PaintAt>,
}

#[derive(Clone, Copy, PartialEq)]
enum Paint {
    /// Not painted, so its paint range means nothing.
    Unpainted,
    /// Painted into its paint range.
    Painted,
    /// Drawn again from the frame before and holding the paint range it had
    /// there, until what it is drawn again with is painted.
    Pending,
}

/// Where an element was drawn, and what it inherited there.
#[derive(Clone)]
struct ElementContext {
    bounds: Bounds<Pixels>,
    content_mask: ContentMask<Pixels>,
    opacity: f32,
    text_style: Rc<TextStyle>,
    rem_size: Pixels,
}

impl ElementContext {
    fn matches(&self, other: &Self) -> bool {
        self.bounds == other.bounds
            && self.content_mask == other.content_mask
            && self.opacity == other.opacity
            && self.rem_size == other.rem_size
            && same_text_style(&self.text_style, &other.text_style)
    }
}

fn same_text_style(a: &Rc<TextStyle>, b: &Rc<TextStyle>) -> bool {
    Rc::ptr_eq(a, b) || **a == **b
}

/// What an element was built with, besides its children, which have
/// records of their own.
#[derive(Clone)]
enum Snapshot {
    Div {
        id: Option<ElementId>,
        /// Taken from the element once it is painted, when not lent by the
        /// record of last frame it was built as.
        style: Option<Rc<StyleRefinement>>,
        children: u32,
    },
    Text {
        id: Option<ElementId>,
        text: SharedString,
    },
    Svg {
        id: Option<ElementId>,
        /// As for [`Snapshot::Div`].
        style: Option<Rc<StyleRefinement>>,
        /// Shared, for svgs are few and a record is copied often.
        source: Rc<SvgSource>,
    },
    /// Not recorded, with nothing nested in it, because it was drawn
    /// somewhere else than the frame before; it is recorded again once it
    /// stands still. See [`Placement`], which does this for roots.
    Moving,
    /// Not recorded, with nothing nested in it, while it rests. See
    /// [`Rest`].
    Resting,
}

/// How long an element has come to nothing, and rested for it.
///
/// An element built differently every frame, and holding nothing drawn
/// again either, a price ticking in its cell, costs a comparison and a
/// record every frame and saves nothing. Once it has come to nothing on
/// [`REST_AFTER`] frames in a row, it rests: it is drawn as upstream draws
/// it, with everything nested in it, neither compared nor recorded, for a
/// frame, then two, doubling up to [`LONGEST_REST`] frames as it keeps
/// coming to nothing. After a rest it is recorded again, and compared on
/// the frame after; drawn again then, or holding anything that is, it
/// starts over.
#[derive(Clone, Copy, Default)]
struct Rest {
    /// The frames in a row it was compared with its record of the frame
    /// before, and it and everything nested in it built anew.
    misses: u8,
    /// The frames it rested since, and for a record after a rest, the
    /// length of that rest, which it does not rest again before it is
    /// compared.
    rested: u8,
}

const REST_AFTER: u8 = 2;
const LONGEST_REST: u8 = 16;

impl Rest {
    /// The rest that follows this one, when it rests this frame.
    fn next(self) -> Option<Rest> {
        if self.misses < REST_AFTER {
            return None;
        }
        let length = (1u8 << (self.misses - REST_AFTER).min(7)).min(LONGEST_REST);
        (self.rested < length).then_some(Rest {
            misses: self.misses,
            rested: self.rested + 1,
        })
    }

    /// The rest of a record built this frame, compared or not with this
    /// one's record, should it come to nothing.
    fn built(self, compared: bool) -> Rest {
        if compared {
            Rest {
                misses: self.misses.saturating_add(1),
                rested: 0,
            }
        } else {
            self
        }
    }
}

/// What an `svg` draws besides its style: the asset at `path`, or the bytes
/// `data_path` names, which is a hash of them, and the cache key the atlas
/// keeps their rendering under.
#[derive(Clone, PartialEq)]
struct SvgSource {
    path: Option<SharedString>,
    data_path: Option<SharedString>,
    transformation: Option<Transformation>,
}

impl SvgSource {
    fn of(svg: &Svg) -> Self {
        SvgSource {
            path: svg.path.clone(),
            data_path: svg.data_path.clone(),
            transformation: svg.transformation,
        }
    }

    fn matches(&self, svg: &Svg) -> bool {
        self.path == svg.path
            && self.data_path == svg.data_path
            && self.transformation == svg.transformation
    }
}

/// A record of last frame: the record `index` of the subtree of its root
/// `root`.
#[derive(Clone, Copy, PartialEq)]
struct PrevRef {
    root: u32,
    index: u32,
}

/// A place in each of a frame's prepaint lists, as a [`PrepaintStateIndex`]
/// is, as an offset from a root's: in `u32`s, and without the font
/// generation, which the root's holds. A record keeps two, and records are
/// copied as often as elements are drawn again.
#[derive(Clone, Copy, Default)]
struct PrepaintAt {
    hitboxes: u32,
    tooltips: u32,
    deferred_draws: u32,
    dispatch_tree: u32,
    accessed_element_states: u32,
    lines: LinesAt,
}

/// As [`PrepaintAt`], a place in each of a frame's paint lists.
#[derive(Clone, Copy, Default)]
struct PaintAt {
    scene: u32,
    window_control_hitboxes: u32,
    #[cfg(any(test, feature = "test-support"))]
    debug_bounds: u32,
    mouse_listeners: u32,
    input_handlers: u32,
    cursor_styles: u32,
    accessed_element_states: u32,
    tab_handle: u32,
    lines: LinesAt,
}

/// As [`PrepaintAt`], a place in each of a frame's line layout lists.
#[derive(Clone, Copy, Default)]
struct LinesAt {
    lines: u32,
    wrapped_lines: u32,
    lines_by_hash: u32,
    wrapped_lines_by_hash: u32,
}

/// The offset of `to` from `from`, which it is not before.
fn offset(from: usize, to: usize) -> u32 {
    (to - from) as u32
}

impl PrepaintAt {
    /// Where `to` is from `from`.
    fn between(from: &PrepaintStateIndex, to: &PrepaintStateIndex) -> Self {
        // Destructured, so that a field upstream adds can't be missed here.
        let PrepaintStateIndex {
            hitboxes_index,
            tooltips_index,
            deferred_draws_index,
            dispatch_tree_index,
            accessed_element_states_index,
            line_layout_index,
        } = to;
        PrepaintAt {
            hitboxes: offset(from.hitboxes_index, *hitboxes_index),
            tooltips: offset(from.tooltips_index, *tooltips_index),
            deferred_draws: offset(from.deferred_draws_index, *deferred_draws_index),
            dispatch_tree: offset(from.dispatch_tree_index, *dispatch_tree_index),
            accessed_element_states: offset(
                from.accessed_element_states_index,
                *accessed_element_states_index,
            ),
            lines: LinesAt::between(&from.line_layout_index, line_layout_index),
        }
    }

    /// The place this far from `base`.
    fn at(self, base: &PrepaintStateIndex) -> PrepaintStateIndex {
        PrepaintStateIndex {
            hitboxes_index: base.hitboxes_index + self.hitboxes as usize,
            tooltips_index: base.tooltips_index + self.tooltips as usize,
            deferred_draws_index: base.deferred_draws_index + self.deferred_draws as usize,
            dispatch_tree_index: base.dispatch_tree_index + self.dispatch_tree as usize,
            accessed_element_states_index: base.accessed_element_states_index
                + self.accessed_element_states as usize,
            line_layout_index: self.lines.at(&base.line_layout_index),
        }
    }

    fn plus(self, other: Self) -> Self {
        PrepaintAt {
            hitboxes: self.hitboxes + other.hitboxes,
            tooltips: self.tooltips + other.tooltips,
            deferred_draws: self.deferred_draws + other.deferred_draws,
            dispatch_tree: self.dispatch_tree + other.dispatch_tree,
            accessed_element_states: self.accessed_element_states + other.accessed_element_states,
            lines: self.lines.plus(other.lines),
        }
    }

    fn minus(self, other: Self) -> Self {
        PrepaintAt {
            hitboxes: self.hitboxes - other.hitboxes,
            tooltips: self.tooltips - other.tooltips,
            deferred_draws: self.deferred_draws - other.deferred_draws,
            dispatch_tree: self.dispatch_tree - other.dispatch_tree,
            accessed_element_states: self.accessed_element_states - other.accessed_element_states,
            lines: self.lines.minus(other.lines),
        }
    }
}

impl PaintAt {
    /// Where `to` is from `from`.
    fn between(from: &PaintIndex, to: &PaintIndex) -> Self {
        // Destructured, so that a field upstream adds can't be missed here.
        let PaintIndex {
            scene_index,
            fast_window_control_hitboxes_index,
            #[cfg(any(test, feature = "test-support"))]
            debug_bounds_index,
            mouse_listeners_index,
            input_handlers_index,
            cursor_styles_index,
            accessed_element_states_index,
            tab_handle_index,
            line_layout_index,
        } = to;
        PaintAt {
            scene: offset(from.scene_index, *scene_index),
            window_control_hitboxes: offset(
                from.fast_window_control_hitboxes_index,
                *fast_window_control_hitboxes_index,
            ),
            #[cfg(any(test, feature = "test-support"))]
            debug_bounds: offset(from.debug_bounds_index, *debug_bounds_index),
            mouse_listeners: offset(from.mouse_listeners_index, *mouse_listeners_index),
            input_handlers: offset(from.input_handlers_index, *input_handlers_index),
            cursor_styles: offset(from.cursor_styles_index, *cursor_styles_index),
            accessed_element_states: offset(
                from.accessed_element_states_index,
                *accessed_element_states_index,
            ),
            tab_handle: offset(from.tab_handle_index, *tab_handle_index),
            lines: LinesAt::between(&from.line_layout_index, line_layout_index),
        }
    }

    /// The place this far from `base`.
    fn at(self, base: &PaintIndex) -> PaintIndex {
        PaintIndex {
            scene_index: base.scene_index + self.scene as usize,
            fast_window_control_hitboxes_index: base.fast_window_control_hitboxes_index
                + self.window_control_hitboxes as usize,
            #[cfg(any(test, feature = "test-support"))]
            debug_bounds_index: base.debug_bounds_index + self.debug_bounds as usize,
            mouse_listeners_index: base.mouse_listeners_index + self.mouse_listeners as usize,
            input_handlers_index: base.input_handlers_index + self.input_handlers as usize,
            cursor_styles_index: base.cursor_styles_index + self.cursor_styles as usize,
            accessed_element_states_index: base.accessed_element_states_index
                + self.accessed_element_states as usize,
            tab_handle_index: base.tab_handle_index + self.tab_handle as usize,
            line_layout_index: self.lines.at(&base.line_layout_index),
        }
    }

    fn plus(self, other: Self) -> Self {
        PaintAt {
            scene: self.scene + other.scene,
            window_control_hitboxes: self.window_control_hitboxes + other.window_control_hitboxes,
            #[cfg(any(test, feature = "test-support"))]
            debug_bounds: self.debug_bounds + other.debug_bounds,
            mouse_listeners: self.mouse_listeners + other.mouse_listeners,
            input_handlers: self.input_handlers + other.input_handlers,
            cursor_styles: self.cursor_styles + other.cursor_styles,
            accessed_element_states: self.accessed_element_states + other.accessed_element_states,
            tab_handle: self.tab_handle + other.tab_handle,
            lines: self.lines.plus(other.lines),
        }
    }

    fn minus(self, other: Self) -> Self {
        PaintAt {
            scene: self.scene - other.scene,
            window_control_hitboxes: self.window_control_hitboxes - other.window_control_hitboxes,
            #[cfg(any(test, feature = "test-support"))]
            debug_bounds: self.debug_bounds - other.debug_bounds,
            mouse_listeners: self.mouse_listeners - other.mouse_listeners,
            input_handlers: self.input_handlers - other.input_handlers,
            cursor_styles: self.cursor_styles - other.cursor_styles,
            accessed_element_states: self.accessed_element_states - other.accessed_element_states,
            tab_handle: self.tab_handle - other.tab_handle,
            lines: self.lines.minus(other.lines),
        }
    }
}

impl LinesAt {
    fn between(from: &LineLayoutIndex, to: &LineLayoutIndex) -> Self {
        // Destructured, so that a field upstream adds can't be missed here;
        // the font generation is the base's.
        let LineLayoutIndex {
            font_generation: _,
            lines_index,
            wrapped_lines_index,
            lines_by_hash_index,
            wrapped_lines_by_hash_index,
        } = to;
        LinesAt {
            lines: offset(from.lines_index, *lines_index),
            wrapped_lines: offset(from.wrapped_lines_index, *wrapped_lines_index),
            lines_by_hash: offset(from.lines_by_hash_index, *lines_by_hash_index),
            wrapped_lines_by_hash: offset(
                from.wrapped_lines_by_hash_index,
                *wrapped_lines_by_hash_index,
            ),
        }
    }

    fn at(self, base: &LineLayoutIndex) -> LineLayoutIndex {
        LineLayoutIndex {
            font_generation: base.font_generation,
            lines_index: base.lines_index + self.lines as usize,
            wrapped_lines_index: base.wrapped_lines_index + self.wrapped_lines as usize,
            lines_by_hash_index: base.lines_by_hash_index + self.lines_by_hash as usize,
            wrapped_lines_by_hash_index: base.wrapped_lines_by_hash_index
                + self.wrapped_lines_by_hash as usize,
        }
    }

    fn plus(self, other: Self) -> Self {
        LinesAt {
            lines: self.lines + other.lines,
            wrapped_lines: self.wrapped_lines + other.wrapped_lines,
            lines_by_hash: self.lines_by_hash + other.lines_by_hash,
            wrapped_lines_by_hash: self.wrapped_lines_by_hash + other.wrapped_lines_by_hash,
        }
    }

    fn minus(self, other: Self) -> Self {
        LinesAt {
            lines: self.lines - other.lines,
            wrapped_lines: self.wrapped_lines - other.wrapped_lines,
            lines_by_hash: self.lines_by_hash - other.lines_by_hash,
            wrapped_lines_by_hash: self.wrapped_lines_by_hash - other.wrapped_lines_by_hash,
        }
    }
}

impl ElementRecords {
    /// How many roots this frame holds so far, for what is drawn from here
    /// on to know where its roots start.
    pub(crate) fn len(&self) -> u32 {
        self.roots.len() as u32
    }

    pub(crate) fn clear(&mut self) {
        self.roots.clear();
        self.by_key.clear();
        self.building.clear();
        self.open.clear();
        self.reused = 0;
        self.shifts.clear();
        self.shifted.clear();
    }

    /// Keeps how an element `moved` for its paint, returning where.
    fn push_shift(&mut self, moved: Option<Moved>) -> u32 {
        match moved {
            Some(moved) => {
                self.shifts.push(Shifted {
                    by: moved.shift.by,
                    operations: moved.operations,
                });
                self.shifts.len() as u32 - 1
            }
            None => NO_SHIFT,
        }
    }

    /// The painted subtree of the root `root`, if it can be drawn again from.
    fn subtree(&self, root: u32) -> Option<&Rc<Subtree>> {
        let root = &self.roots[root as usize];
        match &root.records {
            RootRecords::Frozen(subtree) if root.paint == Paint::Painted && root.usable => {
                Some(subtree)
            }
            _ => None,
        }
    }

    /// The records of the root `root`, pending their paint.
    fn pending(&mut self, root: u32) -> &mut [ElementRecord] {
        match &mut self.roots[root as usize].records {
            RootRecords::Pending(records) => records,
            _ => panic!("a root is painted once, after it is prepainted"),
        }
    }

    fn record(&self, previous: PrevRef) -> &ElementRecord {
        &self
            .subtree(previous.root)
            .expect("a record found is painted")
            .records[previous.index as usize]
    }

    /// Where `previous`'s prepaint and paint went in this frame.
    fn ranges(&self, previous: PrevRef) -> (Range<PrepaintStateIndex>, Range<PaintIndex>) {
        let root = &self.roots[previous.root as usize];
        let record = self.record(previous);
        (
            record.prepaint_range.start.at(&root.prepaint_start)
                ..record.prepaint_range.end.at(&root.prepaint_start),
            record.paint_range.start.at(&root.paint_start)
                ..record.paint_range.end.at(&root.paint_start),
        )
    }

    /// The record the element with layout key `key` left, if it can be drawn
    /// again from it: the one the element around it pointed it to if that
    /// has its key, or else one under its key where that one was, or else a
    /// root under its key.
    fn find(&self, candidate: Option<PrevRef>, key: u64) -> Option<PrevRef> {
        if let Some(candidate) = candidate
            && let Some(subtree) = self.subtree(candidate.root)
        {
            if subtree.records[candidate.index as usize].key == key {
                return Some(candidate);
            }
            if let Some(index) = subtree.find(key) {
                return Some(PrevRef {
                    root: candidate.root,
                    index,
                });
            }
        }
        let root = *self.by_key.get(&key)?;
        self.subtree(root)?;
        Some(PrevRef { root, index: 0 })
    }

    /// Whether the element under `key`, should it be a root, is recorded.
    fn probation(&self, key: u64) -> Probation {
        let Some(&root) = self.by_key.get(&key) else {
            return Probation::Skip(None, Rest::default());
        };
        let root = &self.roots[root as usize];
        let (bounds, moved) = match (&root.records, root.placement) {
            _ if !root.usable => return Probation::Skip(None, Rest::default()),
            (RootRecords::Frozen(subtree), _) if root.paint == Paint::Painted => {
                (subtree.records[0].context.bounds, false)
            }
            (
                _,
                Some(Placement {
                    still: true,
                    bounds,
                    moved,
                }),
            ) => (bounds, moved),
            (_, placement) => {
                return Probation::Skip(
                    placement.map(|placement| Placed {
                        bounds: placement.bounds,
                        moved: placement.moved,
                    }),
                    Rest::default(),
                );
            }
        };
        match root.rest.next() {
            Some(rest) => Probation::Skip(Some(Placed { bounds, moved }), rest),
            None => Probation::Record(bounds, root.rest),
        }
    }

    fn push_root(&mut self, root: Root) -> u32 {
        let index = self.roots.len() as u32;
        // Two roots under one key can only both be drawn; neither can be
        // found to be drawn again.
        let mut usable = true;
        match self.by_key.entry(root.key) {
            collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(index);
            }
            collections::hash_map::Entry::Occupied(entry) => {
                self.roots[*entry.get() as usize].usable = false;
                usable = false;
            }
        }
        self.roots.push(Root { usable, ..root });
        index
    }

    /// Sets the records of the root being built aside, now that it is
    /// prepainted.
    fn finish_prepaint(&mut self) {
        let root = &mut self.roots[self.building_root as usize];
        root.rest = self.building[0].rest;
        root.records = RootRecords::Pending(self.building.drain(..).collect());
    }

    /// Freezes the records of the root `root`, now painted, into its
    /// subtree.
    fn freeze(&mut self, root: u32) {
        let root = &mut self.roots[root as usize];
        let RootRecords::Pending(mut records) = mem::replace(&mut root.records, RootRecords::Lost)
        else {
            panic!("a root is painted once, after it is prepainted");
        };
        for record in records.iter_mut() {
            if record.paint != Paint::Painted {
                record.paint = Paint::Unpainted;
            }
        }
        root.records = RootRecords::Frozen(Rc::new(Subtree {
            records,
            by_key: OnceCell::new(),
        }));
        root.paint = Paint::Painted;
    }
}

/// Carries last frame's roots `range` into this frame, for what they were
/// drawn in is drawn again from last frame, with the prepaint that started
/// at `from` copied to `to`. Their paint is placed by
/// [`place_carried_paint`]. Returns where they landed.
pub(crate) fn carry_records(
    source: &ElementRecords,
    target: &mut ElementRecords,
    range: Range<u32>,
    from: &PrepaintStateIndex,
    to: &PrepaintStateIndex,
) -> Range<u32> {
    let start = target.len();
    for root in &source.roots[range.start as usize..range.end as usize] {
        let (records, paint) = match (&root.records, root.paint) {
            (RootRecords::Frozen(subtree), Paint::Painted) => {
                (RootRecords::Frozen(subtree.clone()), Paint::Pending)
            }
            _ => (RootRecords::Lost, Paint::Unpainted),
        };
        target.push_root(Root {
            key: root.key,
            records,
            placement: root.placement,
            prepaint_start: root.prepaint_start.shifted(from, to),
            paint_start: root.paint_start.clone(),
            paint,
            usable: true,
            fresh: false,
            rest: Rest::default(),
            motion: root.motion.saturating_sub(1),
        });
    }
    start..target.len()
}

/// Places the paint of the roots `range` [`carry_records`] carried, now
/// that the paint that started at `from` last frame was copied to `to`.
pub(crate) fn place_carried_paint(
    records: &mut ElementRecords,
    range: Range<u32>,
    from: &PaintIndex,
    to: &PaintIndex,
) {
    for root in &mut records.roots[range.start as usize..range.end as usize] {
        if root.paint == Paint::Pending {
            root.paint_start = root.paint_start.shifted(from, to);
            root.paint = Paint::Painted;
        }
    }
}

/// What a [`Drawable`] keeps to be drawn from last frame. See
/// [`crate::fast::element`].
#[derive(Default)]
pub(crate) struct DrawableRetention {
    /// Whether the element, and everything nested in it, is of a kind that
    /// can be drawn again from last frame, once worked out.
    eligible: Option<bool>,
    /// The record of last frame at the element's place among its siblings,
    /// as the element around it found it.
    candidate: Option<PrevRef>,
    /// The record of last frame the element was last compared with, and
    /// whether it, and everything nested in it, was built the same way.
    matched: Option<(PrevRef, bool)>,
    /// The style of the record it was compared with, when it was built with
    /// the same.
    lent_style: Option<Rc<StyleRefinement>>,
    phase: Phase,
}

#[derive(Default)]
enum Phase {
    /// Its layout not requested yet.
    #[default]
    Start,
    /// Drawn as upstream draws it, without a record.
    Plain,
    /// A root of the kind that can be drawn again, but with an element of
    /// another kind nested in it: drawn as upstream draws it, its place
    /// kept for the next frame. See [`Placement`].
    Ineligible {
        key: u64,
        layout_id: LayoutId,
        previous: Option<Placed>,
    },
    /// Drawn as upstream draws it, with everything nested in it, while it
    /// is not recorded. See [`Placement`].
    Skipped {
        key: u64,
        layout_id: LayoutId,
        previous: Option<Placed>,
        rest: Rest,
    },
    /// Built and laid out; it leaves a record once prepainted.
    Built(BuiltLayout),
    /// Laid out at the nodes a record of last frame kept, without being
    /// built.
    Kept(KeptLayout),
    /// Prepainted into the record `index` of the root `root`.
    Recorded { root: u32, index: u32 },
    /// Drawn from last frame as far as prepaint, as the root at this index,
    /// moved as the shift at `shift` says, if it moved.
    ReusedRoot { root: u32, shift: u32 },
    /// Drawn from last frame as far as prepaint, into the records starting
    /// at `index` of the root `root`, from the root `source` of last frame,
    /// moved as the shift at `shift` says, if it moved.
    ReusedNested {
        root: u32,
        index: u32,
        source: u32,
        shift: u32,
    },
}

struct BuiltLayout {
    key: u64,
    snapshot: Snapshot,
    layout_id: LayoutId,
    claimed: u32,
    /// Its rest, should it come to nothing.
    rest: Rest,
    /// Where its record of last frame was drawn, for a moving one to tell
    /// whether it stood still.
    previous_bounds: Option<Bounds<Pixels>>,
}

struct KeptLayout {
    previous: PrevRef,
    key: u64,
    layout_id: LayoutId,
    /// Where it was begun, to request its layout after all.
    position: KeyPosition,
    /// Its rest, should it not be drawn again after all: kept, but not
    /// drawable again where it moved to, it comes to nothing like an
    /// element built anew, and rests for it.
    rest: Rest,
}

/// An element as an element nested in another sees it, for the one around
/// it to compare it with last frame's.
pub(crate) struct ElementParts<'a> {
    element: &'a mut dyn Any,
    state: &'a mut DrawableRetention,
    id: fn(&dyn Any) -> Option<ElementId>,
}

impl<'a> ElementParts<'a> {
    pub(crate) fn new<E: Element>(element: &'a mut E, state: &'a mut DrawableRetention) -> Self {
        ElementParts {
            element,
            state,
            id: |element| element.downcast_ref::<E>().and_then(Element::id),
        }
    }

    /// The element's [`Element::id`].
    pub(crate) fn element_id(&self) -> Option<ElementId> {
        (self.id)(self.element)
    }
}

/// The type of [`Div`]'s listener for its children's bounds, named here so
/// that its field fits a line.
pub(crate) type PrepaintListener =
    Option<Box<dyn Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static>>;

/// The type of [`Div`]'s function ordering its children's prepaint, named
/// here so that its field fits a line.
pub(crate) type PrepaintOrderFn =
    Option<Box<dyn Fn(&mut Window, &mut App) -> smallvec::SmallVec<[usize; 8]>>>;

/// The element types that can be drawn from last frame. A `Drawable` of any
/// other type goes straight to upstream's drawing, decided at compile time.
#[inline(always)]
fn retainable_type<E: 'static>() -> bool {
    let id = TypeId::of::<E>();
    id == TypeId::of::<Div>()
        || id == TypeId::of::<Stateful<Div>>()
        || id == TypeId::of::<SharedString>()
        || id == TypeId::of::<&'static str>()
        || id == TypeId::of::<Text>()
        || id == TypeId::of::<Svg>()
}

/// Whether anything can be drawn from last frame in `window` this frame.
fn active(window: &Window, cx: &App) -> bool {
    #[cfg(any(feature = "inspector", debug_assertions))]
    if window.inspector_enabled() {
        return false;
    }
    let state = &window.retained_state;
    state.element_retention
        && state.view_retention
        && !window.refreshing
        && !cx.has_active_drag()
        && !window.a11y.is_active()
}

/// What an element is, as far as being drawn again goes.
enum Own<'a> {
    Div(&'a mut Div),
    Svg(&'a mut Svg),
    Text {
        id: Option<&'a ElementId>,
        text: TextRef<'a>,
    },
}

enum TextRef<'a> {
    Shared(&'a SharedString),
    Static(&'static str),
}

impl TextRef<'_> {
    fn as_str(&self) -> &str {
        match self {
            TextRef::Shared(text) => text,
            TextRef::Static(text) => text,
        }
    }

    fn to_shared(&self) -> SharedString {
        match self {
            TextRef::Shared(text) => (*text).clone(),
            TextRef::Static(text) => SharedString::new_static(text),
        }
    }
}

fn own(element: &mut dyn Any) -> Option<Own<'_>> {
    if element.is::<Div>() {
        return element.downcast_mut::<Div>().map(Own::Div);
    }
    if element.is::<Stateful<Div>>() {
        return element
            .downcast_mut::<Stateful<Div>>()
            .map(|stateful| Own::Div(&mut stateful.element));
    }
    if element.is::<SharedString>() {
        let text = element.downcast_ref::<SharedString>()?;
        return Some(Own::Text {
            id: None,
            text: TextRef::Shared(text),
        });
    }
    if element.is::<&'static str>() {
        let text = *element.downcast_ref::<&'static str>()?;
        return Some(Own::Text {
            id: None,
            text: TextRef::Static(text),
        });
    }
    if element.is::<Text>() {
        let text = element.downcast_ref::<Text>()?;
        return Some(Own::Text {
            id: text.id.as_ref(),
            text: TextRef::Shared(&text.text),
        });
    }
    element.downcast_mut::<Svg>().map(Own::Svg)
}

impl Own<'_> {
    /// Whether the element itself, leaving aside what is nested in it, is of
    /// a kind that can be drawn again from last frame.
    fn plain(&self) -> bool {
        match self {
            Own::Div(div) => plain_div(div),
            // An svg from a file draws nothing until the file is loaded,
            // which notifies the view it is drawn in without changing it.
            Own::Svg(svg) => svg.external_path.is_none() && plain_interactivity(&svg.interactivity),
            Own::Text { .. } => true,
        }
    }

    /// The style an element that has one was built with.
    fn style(&mut self) -> Option<&mut StyleRefinement> {
        match self {
            Own::Div(div) => Some(&mut div.interactivity.base_style),
            Own::Svg(svg) => Some(&mut svg.interactivity.base_style),
            Own::Text { .. } => None,
        }
    }

    /// What it was built with, for a record, but for a `div`'s style, which
    /// `lent_style` is, when a record of last frame lends it.
    fn snapshot(&self, lent_style: Option<Rc<StyleRefinement>>) -> Snapshot {
        match self {
            Own::Div(div) => Snapshot::Div {
                id: div.interactivity.element_id.clone(),
                style: lent_style,
                children: div.children.len() as u32,
            },
            Own::Svg(svg) => Snapshot::Svg {
                id: svg.interactivity.element_id.clone(),
                style: lent_style,
                source: Rc::new(SvgSource::of(svg)),
            },
            Own::Text { id, text } => Snapshot::Text {
                id: id.cloned(),
                text: text.to_shared(),
            },
        }
    }
}

/// Whether a `div` draws nothing but its style and its children: no
/// listener, and no interactive state that changes what it draws.
fn plain_div(div: &Div) -> bool {
    let Div {
        interactivity,
        children: _,
        prepaint_listener,
        image_cache,
        prepaint_order_fn,
    } = div;
    prepaint_listener.is_none()
        && image_cache.is_none()
        && prepaint_order_fn.is_none()
        && plain_interactivity(interactivity)
}

fn plain_interactivity(interactivity: &Interactivity) -> bool {
    // Destructured, so that a field upstream adds can't be missed here.
    let Interactivity {
        element_id: _,
        active: _,
        hovered: _,
        tooltip_id: _,
        content_size: _,
        key_context,
        focusable,
        tracked_focus_handle,
        tracked_scroll_handle,
        scroll_anchor,
        scroll_offset,
        ongoing_scroll,
        group,
        base_style,
        focus_style,
        in_focus_style,
        focus_visible_style,
        hover_style,
        group_hover_style,
        active_style,
        group_active_style,
        drag_over_styles,
        group_drag_over_styles,
        mouse_down_listeners,
        mouse_up_listeners,
        mouse_pressure_listeners,
        mouse_move_listeners,
        mouse_exit_listeners,
        file_drop_exit_listeners,
        scroll_wheel_listeners,
        pinch_listeners,
        fast_gesture_listeners,
        key_down_listeners,
        key_up_listeners,
        modifiers_changed_listeners,
        action_listeners,
        drop_listeners,
        can_drop_predicate,
        click_listeners,
        aux_click_listeners,
        drag_listener,
        hover_listener,
        hover_listener_mode: _,
        tooltip_builder,
        tooltip_show_delay: _,
        window_control,
        hitbox_behavior,
        tab_index,
        tab_group,
        tab_stop: _,
        // What accessibility reads is drawn only while it is active, when
        // nothing is drawn again from last frame.
        a11y_action_listeners: _,
        a11y_synthetic_children: _,
        report_active_descendant_focus: _,
        override_role: _,
        aria: _,
        #[cfg(any(feature = "inspector", debug_assertions))]
            source_location: _,
        #[cfg(any(test, feature = "test-support"))]
            debug_selector: _,
    } = interactivity;
    key_context.is_none()
        && !focusable
        && tracked_focus_handle.is_none()
        && tracked_scroll_handle.is_none()
        && scroll_anchor.is_none()
        && scroll_offset.is_none()
        && ongoing_scroll.is_none()
        && group.is_none()
        && base_style.overflow.x != Some(Overflow::Scroll)
        && base_style.overflow.y != Some(Overflow::Scroll)
        && base_style.mouse_cursor.is_none()
        && focus_style.is_none()
        && in_focus_style.is_none()
        && focus_visible_style.is_none()
        && hover_style.is_none()
        && group_hover_style.is_none()
        && active_style.is_none()
        && group_active_style.is_none()
        && drag_over_styles.is_empty()
        && group_drag_over_styles.is_empty()
        && mouse_down_listeners.is_empty()
        && mouse_up_listeners.is_empty()
        && mouse_pressure_listeners.is_empty()
        && mouse_move_listeners.is_empty()
        && mouse_exit_listeners.is_empty()
        && file_drop_exit_listeners.is_empty()
        && scroll_wheel_listeners.is_empty()
        && pinch_listeners.is_empty()
        && fast_gesture_listeners.is_empty()
        && key_down_listeners.is_empty()
        && key_up_listeners.is_empty()
        && modifiers_changed_listeners.is_empty()
        && action_listeners.is_empty()
        && drop_listeners.is_empty()
        && can_drop_predicate.is_none()
        && click_listeners.is_empty()
        && aux_click_listeners.is_empty()
        && drag_listener.is_none()
        && hover_listener.is_none()
        && tooltip_builder.is_none()
        && window_control.is_none()
        && *hitbox_behavior == HitboxBehavior::Normal
        && tab_index.is_none()
        && !tab_group
}

fn child_parts(child: &mut AnyElement) -> ElementParts<'_> {
    child.0.fast_retention()
}

/// Whether `parts` and every element nested in it can be drawn again from
/// last frame.
fn subtree_eligible(parts: ElementParts) -> bool {
    if let Some(eligible) = parts.state.eligible {
        return eligible;
    }
    let eligible = own(parts.element).is_some_and(|own| own.plain() && nested_eligible(own));
    parts.state.eligible = Some(eligible);
    eligible
}

/// Whether every element nested in `own` can be drawn again from last
/// frame, for an element found [`Own::plain`] itself.
fn nested_eligible(own: Own) -> bool {
    match own {
        Own::Div(div) => div.children.iter_mut().all(|child| {
            let child: &mut AnyElement = child;
            subtree_eligible(child_parts(child))
        }),
        Own::Svg(_) | Own::Text { .. } => true,
    }
}

/// Whether `parts`, eligible, and every element nested in it were built as
/// the record `previous` of `subtree` and those nested in it were. Points
/// the children of `parts` to the records at their places on the way.
fn subtree_matches(parts: ElementParts, subtree: &Subtree, previous: PrevRef) -> bool {
    if let Some((matched, result)) = parts.state.matched
        && matched == previous
    {
        return result;
    }
    let records = &subtree.records;
    let index = previous.index as usize;
    let record = &records[index];
    let mut own = own(parts.element);
    // Its children pointed to the records at their places, which those with
    // the keys they had there take up, whether it was built as it was or not.
    if let (Some(Own::Div(div)), Snapshot::Div { children, .. }) = (&mut own, &record.snapshot)
        && record.complete
    {
        let mut child_index = index + 1;
        for child in div.children.iter_mut().take(*children as usize) {
            let child: &mut AnyElement = child;
            child_parts(child).state.candidate = Some(PrevRef {
                root: previous.root,
                index: child_index as u32,
            });
            child_index += records[child_index].nested as usize + 1;
        }
    }
    let result = record.reusable
        && record.paint == Paint::Painted
        && match (own, &record.snapshot) {
            (
                Some(Own::Div(div)),
                Snapshot::Div {
                    id,
                    style,
                    children,
                },
            ) => {
                let same_style = style
                    .as_ref()
                    .is_some_and(|style| **style == *div.interactivity.base_style);
                if same_style {
                    parts.state.lent_style = style.clone();
                }
                div.children.len() == *children as usize
                    && div.interactivity.element_id == *id
                    && same_style
                    && {
                        let mut child_index = index + 1;
                        div.children.iter_mut().all(|child| {
                            let child: &mut AnyElement = child;
                            let child_ref = PrevRef {
                                root: previous.root,
                                index: child_index as u32,
                            };
                            child_index += records[child_index].nested as usize + 1;
                            subtree_matches(child_parts(child), subtree, child_ref)
                        })
                    }
            }
            (
                Some(Own::Text { id, text }),
                Snapshot::Text {
                    id: previous_id,
                    text: previous_text,
                },
            ) => id == previous_id.as_ref() && text.as_str() == previous_text.as_ref(),
            (Some(Own::Svg(svg)), Snapshot::Svg { id, style, source }) => {
                let same_style = style
                    .as_ref()
                    .is_some_and(|style| **style == *svg.interactivity.base_style);
                if same_style {
                    parts.state.lent_style = style.clone();
                }
                svg.interactivity.element_id == *id && same_style && source.matches(svg)
            }
            _ => false,
        };
    parts.state.matched = Some((previous, result));
    result
}

/// Requests `drawable`'s layout as [`Drawable::request_layout`] does, or,
/// when it was built as it was last frame, keeps the nodes it had then
/// without building it.
#[inline(always)]
pub(crate) fn request_layout<E: Element>(
    drawable: &mut Drawable<E>,
    window: &mut Window,
    cx: &mut App,
) -> LayoutId {
    if !retainable_type::<E>() || !matches!(drawable.fast_retention.phase, Phase::Start) {
        return drawable.request_layout(window, cx);
    }
    request_retained_layout(drawable, window, cx)
}

#[inline(never)]
fn request_retained_layout<E: Element>(
    drawable: &mut Drawable<E>,
    window: &mut Window,
    cx: &mut App,
) -> LayoutId {
    if !active(window, cx) || window.retained_state.skipping_elements > 0 {
        drawable.fast_retention.phase = Phase::Plain;
        return drawable.request_layout(window, cx);
    }
    // Nested in an element being compared, it was looked at with it.
    let own_plain = match drawable.fast_retention.eligible {
        Some(eligible) => eligible,
        None => own(&mut drawable.element).is_some_and(|own| own.plain()),
    };
    if !own_plain {
        drawable.fast_retention.eligible = Some(false);
        drawable.fast_retention.phase = Phase::Plain;
        return drawable.request_layout(window, cx);
    }
    let id = drawable.element.id();
    let position = key_position(window);
    let key = position.key(id.as_ref());
    // A root that moved is skipped before the elements nested in it are
    // looked at: rows under a scroll would otherwise be walked every frame
    // only to be drawn as upstream draws them. One never drawn here is
    // looked at, for the elements nested in it to be drawn again on their
    // own at once should it hold one that cannot be.
    let probation = (window.retained_state.recording_elements == 0)
        .then(|| window.rendered_frame.retained.elements.probation(key));
    if let Some(Probation::Skip(previous @ Some(_), rest)) = probation {
        return request_skipped_root(drawable, key, previous, rest, window, cx);
    }
    let eligible = match drawable.fast_retention.eligible {
        Some(eligible) => eligible,
        None => {
            let eligible = own(&mut drawable.element).is_some_and(nested_eligible);
            drawable.fast_retention.eligible = Some(eligible);
            eligible
        }
    };
    if !eligible {
        let layout_id = drawable.request_layout(window, cx);
        // A root keeps its place, so that it is not skipped next frame as a
        // root never drawn there, which would skip the elements nested in it
        // that can be drawn again on their own.
        drawable.fast_retention.phase = match probation {
            Some(Probation::Record(previous, _)) => Phase::Ineligible {
                key,
                layout_id,
                previous: Some(Placed {
                    bounds: previous,
                    moved: moved_last_frame(key, window),
                }),
            },
            Some(Probation::Skip(previous, _)) => Phase::Ineligible {
                key,
                layout_id,
                previous,
            },
            None => Phase::Plain,
        };
        return layout_id;
    }
    if let Some(Probation::Skip(None, rest)) = probation {
        return request_skipped_root(drawable, key, None, rest, window, cx);
    }
    let previous = window
        .rendered_frame
        .retained
        .elements
        .find(drawable.fast_retention.candidate, key);

    // What it was built with last frame, if it was recorded, and its rest.
    let mut compared = false;
    let mut rest = match probation {
        Some(Probation::Record(_, rest)) => rest,
        _ => Rest::default(),
    };
    if window.retained_state.recording_elements > 0
        && let Some(previous) = previous
    {
        let elements = &window.rendered_frame.retained.elements;
        let record = elements.record(previous);
        // Taken over since it was built, it was drawn again in the meantime.
        let record_rest = if elements.roots[previous.root as usize].fresh {
            record.rest
        } else {
            Rest::default()
        };
        let skipped = match record.snapshot {
            // Moving: recorded as such, with nothing nested in it, until it
            // stands still.
            Snapshot::Moving if !record.still => Some((
                Snapshot::Moving,
                Some(record.context.bounds),
                Rest::default(),
            )),
            _ => record_rest
                .next()
                .map(|next| (Snapshot::Resting, None, next)),
        };
        if let Some((snapshot, previous_bounds, rest)) = skipped {
            let layout_id = request_skipped_layout(drawable, window, cx);
            drawable.fast_retention.phase = Phase::Built(BuiltLayout {
                key,
                snapshot,
                layout_id,
                claimed: 0,
                rest,
                previous_bounds,
            });
            return layout_id;
        }
        compared = !matches!(record.snapshot, Snapshot::Moving | Snapshot::Resting);
        rest = record_rest;
    } else if previous.is_some() {
        compared = true;
    }

    if let Some(previous) = previous
        && let Some(layout_id) = keep_layout(
            ElementParts::new(&mut drawable.element, &mut drawable.fast_retention),
            previous,
            window,
        )
    {
        // Begun and ended, for its siblings to be keyed as they would be.
        push_layout_key(window, id.as_ref());
        pop_layout_key(window);
        drawable.fast_retention.phase = Phase::Kept(KeptLayout {
            previous,
            key,
            layout_id,
            position,
            rest: rest.built(compared),
        });
        return layout_id;
    }

    let lent_style = drawable.fast_retention.lent_style.take();
    let snapshot = own(&mut drawable.element)
        .expect("an eligible element is one of its own kind")
        .snapshot(lent_style);
    window.retained_state.recording_elements += 1;
    let (layout_id, claimed) = count_layout(window, |window| drawable.request_layout(window, cx));
    window.retained_state.recording_elements -= 1;
    drawable.fast_retention.phase = match claimed {
        Some(claimed) => Phase::Built(BuiltLayout {
            key,
            snapshot,
            layout_id,
            claimed,
            rest: rest.built(compared),
            previous_bounds: None,
        }),
        None => Phase::Plain,
    };
    layout_id
}

/// Requests the layout of a root not recorded this frame, `previous` where
/// it was placed last frame, if it was. See [`Placement`].
fn request_skipped_root<E: Element>(
    drawable: &mut Drawable<E>,
    key: u64,
    previous: Option<Placed>,
    rest: Rest,
    window: &mut Window,
    cx: &mut App,
) -> LayoutId {
    let layout_id = request_skipped_layout(drawable, window, cx);
    drawable.fast_retention.phase = Phase::Skipped {
        key,
        layout_id,
        previous,
        rest,
    };
    layout_id
}

/// Requests `drawable`'s layout as upstream does, and that of everything
/// nested in it, none of it recorded.
fn request_skipped_layout<E: Element>(
    drawable: &mut Drawable<E>,
    window: &mut Window,
    cx: &mut App,
) -> LayoutId {
    window.retained_state.skipping_elements += 1;
    let layout_id = drawable.request_layout(window, cx);
    window.retained_state.skipping_elements -= 1;
    layout_id
}

/// Puts the keys of the layout nodes `previous` and the records nested in it
/// hold in `keys`.
fn gather_keys(records: &ElementRecords, previous: PrevRef, keys: &mut Vec<u64>) {
    let subtree = records
        .subtree(previous.root)
        .expect("a record found is painted");
    let start = previous.index as usize;
    let end = start + subtree.records[start].nested as usize + 1;
    keys.clear();
    keys.extend(subtree.records[start..end].iter().map(|record| record.key));
}

/// Keeps the layout nodes of last frame's record `previous` for `element`,
/// if it was built as that record's element was, inherits what it did, and
/// the nodes are all still there, returning the node it is laid out at.
fn keep_layout(parts: ElementParts, previous: PrevRef, window: &mut Window) -> Option<LayoutId> {
    let records = &window.rendered_frame.retained.elements;
    let subtree = records.subtree(previous.root)?;
    let record = &subtree.records[previous.index as usize];
    if record.context.rem_size != window.rem_size()
        || !same_text_style(
            &record.context.text_style,
            &crate::fast::text_style::text_style(window),
        )
        || !subtree_matches(parts, subtree, previous)
    {
        return None;
    }
    let layout_id = record.layout_id;
    let keys = &mut window.retained_state.element_keys;
    gather_keys(records, previous, keys);
    let engine = window.layout_engine.as_mut().unwrap();
    engine.try_keep_retained(keys).then_some(layout_id)
}

/// Requests a layout with `request`, counting the layout nodes it claims.
/// Returns the count unless it allocated a node that is gone at the end of
/// the frame, which nothing can be drawn again from.
fn count_layout(
    window: &mut Window,
    request: impl FnOnce(&mut Window) -> LayoutId,
) -> (LayoutId, Option<u32>) {
    let engine = window.layout_engine.as_mut().unwrap();
    let recording = engine.record_claimed_keys();
    let transient = engine.transient_count();
    let layout_id = request(window);
    let engine = window.layout_engine.as_mut().unwrap();
    let claimed = engine.finish_counting_claimed_keys(recording);
    let kept = engine.transient_count() == transient;
    (layout_id, kept.then_some(claimed as u32))
}

/// Lays `drawable` out as [`Drawable::layout_as_root`] does, or, when its
/// layout was kept from last frame, computes it at the node it kept.
#[inline(always)]
pub(crate) fn layout_as_root<E: Element>(
    drawable: &mut Drawable<E>,
    available_space: Size<AvailableSpace>,
    window: &mut Window,
    cx: &mut App,
) -> Size<Pixels> {
    if retainable_type::<E>() {
        if matches!(drawable.fast_retention.phase, Phase::Start) {
            request_layout(drawable, window, cx);
        }
        if let Phase::Kept(kept) = &drawable.fast_retention.phase {
            let layout_id = kept.layout_id;
            window.compute_layout(layout_id, available_space, cx);
            return window.layout_bounds(layout_id).size;
        }
    }
    drawable.layout_as_root(available_space, window, cx)
}

/// Prepaints `drawable` as [`Drawable::prepaint`] does, recording what it
/// prepaints, or draws it again from last frame when it was built as it was
/// then and is drawn where it was.
#[inline(always)]
pub(crate) fn prepaint<E: Element>(drawable: &mut Drawable<E>, window: &mut Window, cx: &mut App) {
    if !retainable_type::<E>() {
        return drawable.prepaint(window, cx);
    }
    prepaint_retained(drawable, window, cx)
}

#[inline(never)]
fn prepaint_retained<E: Element>(drawable: &mut Drawable<E>, window: &mut Window, cx: &mut App) {
    let built = match mem::take(&mut drawable.fast_retention.phase) {
        Phase::Built(built) => built,
        Phase::Kept(kept) => {
            if let Some(phase) = reuse_prepaint(&kept, window) {
                drawable.fast_retention.phase = phase;
                return;
            }
            let root = window.next_frame.retained.elements.open.is_empty();
            match build_at_kept_layout(drawable, &kept, root, window, cx) {
                Some(built) => built,
                None => {
                    if root {
                        // Moved: drawn as upstream draws it until it is
                        // steady again, and while it rests.
                        let bounds = window.layout_bounds(kept.layout_id);
                        let previous = Placed {
                            bounds: previous_bounds(&kept, window),
                            moved: moved_last_frame(kept.key, window),
                        };
                        let (still, moved) = placed_at(previous, bounds, window.scale_factor());
                        let rest = if still { kept.rest } else { Rest::default() };
                        leave_placement(kept.key, bounds, still, moved, rest, window);
                    }
                    drawable.fast_retention.phase = Phase::Plain;
                    return drawable.prepaint(window, cx);
                }
            }
        }
        Phase::Skipped {
            key,
            layout_id,
            previous,
            rest,
        } => {
            let bounds = window.layout_bounds(layout_id);
            let (still, moved) = previous.map_or((false, false), |previous| {
                placed_at(previous, bounds, window.scale_factor())
            });
            let rest = if still { rest } else { Rest::default() };
            leave_placement(key, bounds, still, moved, rest, window);
            drawable.fast_retention.phase = Phase::Plain;
            return drawable.prepaint(window, cx);
        }
        Phase::Ineligible {
            key,
            layout_id,
            previous,
        } => {
            // Standing still unless it is known to have moved, so that the
            // elements nested in it are not skipped for it next frame.
            let bounds = window.layout_bounds(layout_id);
            let scale_factor = window.scale_factor();
            let (still, moved) = previous.map_or((true, false), |previous| {
                placed_at(previous, bounds, scale_factor)
            });
            leave_placement(key, bounds, still, moved, Rest::default(), window);
            drawable.fast_retention.phase = Phase::Plain;
            return drawable.prepaint(window, cx);
        }
        phase => {
            drawable.fast_retention.phase = phase;
            return drawable.prepaint(window, cx);
        }
    };
    let (root, index) = begin_record(built, window);
    drawable.prepaint(window, cx);
    finish_record(index, window);
    drawable.fast_retention.phase = Phase::Recorded { root, index };
}

/// Builds an element whose layout was kept from last frame, but which cannot
/// be drawn again from it because it is drawn somewhere else: it moved, or
/// what it inherits changed. Its layout is requested now, as it would have
/// been, which finds the nodes it kept as they were.
#[inline(never)]
fn build_at_kept_layout<E: Element>(
    drawable: &mut Drawable<E>,
    kept: &KeptLayout,
    root: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<BuiltLayout> {
    let bounds = window.layout_bounds(kept.layout_id);
    {
        let records = &window.rendered_frame.retained.elements;
        let keys = &mut window.retained_state.element_keys;
        gather_keys(records, kept.previous, keys);
        window.layout_engine.as_mut().unwrap().release_kept(keys);
    }
    let changes = window.layout_changes();
    let remeasures = window.layout_remeasures();
    let layout_id = crate::fast::layout_key::with_key_position(window, &kept.position, |window| {
        request_skipped_layout(drawable, window, cx)
    });
    // Built as it was, it asks for the layout it had; should it ask for
    // another after all, it is laid out within the bounds it was given,
    // and on the next frame from scratch.
    let unchanged = layout_id == kept.layout_id && window.layout_changes() == changes;
    if !unchanged || window.layout_remeasures() != remeasures {
        window.relayout_in_place(layout_id, bounds.size.into(), cx);
    }
    if !unchanged {
        window.request_animation_frame();
    }
    // Moved: a root is left without records, one nested in an element
    // being recorded is recorded as moving.
    (!root).then(|| BuiltLayout {
        key: kept.key,
        snapshot: Snapshot::Moving,
        layout_id,
        claimed: 0,
        rest: kept.rest,
        previous_bounds: Some(previous_bounds(kept, window)),
    })
}

/// Where the element whose layout `kept` kept was drawn last frame.
fn previous_bounds(kept: &KeptLayout, window: &Window) -> Bounds<Pixels> {
    window
        .rendered_frame
        .retained
        .elements
        .record(kept.previous)
        .context
        .bounds
}

/// Leaves a root without records for the element under `key`, drawn at
/// `bounds`, `still` if it counts as drawn where it was last frame, `moved`
/// if it moved there by a whole number of device pixels. See [`Placement`].
fn leave_placement(
    key: u64,
    bounds: Bounds<Pixels>,
    still: bool,
    moved: bool,
    rest: Rest,
    window: &mut Window,
) {
    if !window.next_frame.retained.elements.open.is_empty() {
        return;
    }
    let start = window.prepaint_index();
    let motion = if moved {
        IN_MOTION
    } else {
        previous_motion(key, window)
    };
    window.next_frame.retained.elements.push_root(Root {
        key,
        records: RootRecords::Lost,
        placement: Some(Placement {
            bounds,
            still,
            moved,
        }),
        prepaint_start: start,
        paint_start: PaintIndex::default(),
        paint: Paint::Unpainted,
        usable: true,
        fresh: false,
        rest,
        motion,
    });
}

/// Where the root under `key` was drawn last frame, and how long it had
/// left to note in motion then (see [`Root::motion`]).
fn previous_root(key: u64, window: &Window) -> Option<(Bounds<Pixels>, u8)> {
    let elements = &window.rendered_frame.retained.elements;
    let root = &elements.roots[*elements.by_key.get(&key)? as usize];
    let bounds = match (&root.records, root.placement) {
        (RootRecords::Frozen(subtree), _) => subtree.records[0].context.bounds,
        (_, Some(placement)) => placement.bounds,
        _ => return None,
    };
    Some((bounds, root.motion))
}

/// Whether the root under `key` moved last frame.
fn moved_last_frame(key: u64, window: &Window) -> bool {
    previous_root(key, window).is_some_and(|(_, motion)| motion == IN_MOTION)
}

/// Whether an element placed as `previous` last frame and at `bounds` now
/// counts as drawn where it was, for it to be recorded, and whether it
/// moved there by a whole number of device pixels (see [`Placement`]).
fn placed_at(previous: Placed, bounds: Bounds<Pixels>, scale_factor: f32) -> (bool, bool) {
    if previous.bounds == bounds {
        return (true, false);
    }
    let moved = steady(previous.bounds, bounds, scale_factor);
    (moved && previous.moved, moved)
}

/// How long the root under `key` has left to note in motion, should it
/// hold still (see [`Root::motion`]).
fn previous_motion(key: u64, window: &Window) -> u8 {
    previous_root(key, window).map_or(0, |(_, motion)| motion.saturating_sub(1))
}

/// Whether an element drawn at `previous` last frame and at `bounds` now can
/// be drawn again from last frame: where it was, or moved by a whole number
/// of device pixels at the same size (see [`crate::fast::shift`]).
fn steady(previous: Bounds<Pixels>, bounds: Bounds<Pixels>, scale_factor: f32) -> bool {
    previous == bounds
        || previous.size == bounds.size && {
            let offset = (bounds.origin - previous.origin).scale(scale_factor);
            offset.x.0.fract() == 0. && offset.y.0.fract() == 0.
        }
}

fn context(layout_id: LayoutId, window: &mut Window) -> ElementContext {
    ElementContext {
        bounds: window.layout_bounds(layout_id),
        content_mask: window.content_mask(),
        opacity: window.element_opacity,
        text_style: crate::fast::text_style::text_style(window),
        rem_size: window.rem_size(),
    }
}

/// Starts this frame's record of an element being prepainted, returning
/// the root it is in and where it is among its records.
fn begin_record(built: BuiltLayout, window: &mut Window) -> (u32, u32) {
    let context = context(built.layout_id, window);
    let scale_factor = window.scale_factor();
    let start = window.prepaint_index();
    window
        .layout_engine
        .as_mut()
        .unwrap()
        .retention
        .stats
        .elements_built += 1;
    let moved = built
        .previous_bounds
        .is_some_and(|previous| previous != context.bounds);
    let root_motion = window
        .next_frame
        .retained
        .elements
        .open
        .is_empty()
        .then(|| match previous_root(built.key, window) {
            Some((bounds, _)) if bounds != context.bounds => IN_MOTION,
            Some((_, motion)) => motion.saturating_sub(1),
            None => 0,
        });
    let elements = &mut window.next_frame.retained.elements;
    if let Some(motion) = root_motion {
        debug_assert!(elements.building.is_empty());
        elements.building_root = elements.push_root(Root {
            key: built.key,
            records: RootRecords::Building,
            placement: None,
            prepaint_start: start.clone(),
            paint_start: PaintIndex::default(),
            paint: Paint::Unpainted,
            usable: true,
            fresh: true,
            rest: Rest::default(),
            motion,
        });
    }
    if moved {
        elements.roots[elements.building_root as usize].motion = IN_MOTION;
    }
    let index = elements.building.len() as u32;
    let at = PrepaintAt::between(
        &elements.roots[elements.building_root as usize].prepaint_start,
        &start,
    );
    let still = built
        .previous_bounds
        .is_some_and(|previous| steady(previous, context.bounds, scale_factor));
    elements.building.push(ElementRecord {
        key: built.key,
        snapshot: built.snapshot,
        nested: 0,
        complete: false,
        reusable: false,
        paint: Paint::Unpainted,
        still,
        rest: built.rest,
        noted: Noted::NOTHING,
        layout_id: built.layout_id,
        claimed: built.claimed,
        context,
        prepaint_range: at..at,
        paint_range: PaintAt::default()..PaintAt::default(),
    });
    elements.open.push((index, elements.reused));
    (elements.building_root, index)
}

/// Ends the record [`begin_record`] started, once its element is
/// prepainted.
fn finish_record(index: u32, window: &mut Window) {
    let end = window.prepaint_index();
    let elements = &mut window.next_frame.retained.elements;
    let end = PrepaintAt::between(
        &elements.roots[elements.building_root as usize].prepaint_start,
        &end,
    );
    let (open, reused) = elements
        .open
        .pop()
        .expect("a record is finished once begun");
    debug_assert_eq!(open, index);
    let index = index as usize;
    let len = elements.building.len();
    let mut children = 0;
    let mut child = index + 1;
    while child < len {
        children += 1;
        child += elements.building[child].nested as usize + 1;
    }
    let record = &mut elements.building[index];
    let nested = (len - index - 1) as u32;
    record.nested = nested;
    record.prepaint_range.end = end;
    record.complete = match &record.snapshot {
        Snapshot::Div { children: all, .. } => children == *all,
        Snapshot::Text { .. } | Snapshot::Svg { .. } | Snapshot::Moving | Snapshot::Resting => {
            children == 0
        }
    };
    record.reusable = record.complete
        && !matches!(record.snapshot, Snapshot::Moving | Snapshot::Resting)
        && record.claimed == nested + 1;
    if elements.reused != reused {
        record.rest = Rest::default();
    }
    if elements.open.is_empty() {
        elements.finish_prepaint();
    }
}

/// Draws the element whose layout `kept` kept again from last frame as far
/// as its prepaint goes, if it is drawn where it was, or moved where it can
/// be drawn again moved, returning what its paint takes over from there.
fn reuse_prepaint(kept: &KeptLayout, window: &mut Window) -> Option<Phase> {
    let context = context(kept.layout_id, window);
    let previous = kept.previous;
    let (prepaint_range, paint_range, moved) = {
        let records = &window.rendered_frame.retained.elements;
        let mut moved = if records.record(previous).context.matches(&context) {
            None
        } else {
            Some(moved_to(records, previous, &context, window)?)
        };
        if remeasured(records, previous, window) {
            return None;
        }
        let (prepaint_range, paint_range) = records.ranges(previous);
        if let Some(moved) = &mut moved {
            moved.operations = can_move(
                &prepaint_range,
                &paint_range,
                &moved.shift,
                &window.rendered_frame.scene,
                &mut window.next_frame.retained.elements.shifted,
            )?;
        }
        (prepaint_range, paint_range, moved)
    };
    let start = window.prepaint_index();
    window.reuse_prepaint(prepaint_range.clone());
    debug_assert!(
        window.prepaint_index() == prepaint_range.end.shifted(&prepaint_range.start, &start),
        "a reused prepaint range changed length"
    );

    let source = &window.rendered_frame.retained.elements;
    let subtree = source
        .subtree(previous.root)
        .expect("a record found is painted");
    let first = previous.index as usize;
    let last = first + subtree.records[first].nested as usize;
    let stats = &mut window.layout_engine.as_mut().unwrap().retention.stats;
    stats.elements_reused += (last - first + 1) as u64;
    if moved.is_some() {
        stats.elements_moved += (last - first + 1) as u64;
    }
    window.next_frame.retained.reused_any = true;
    let target = &mut window.next_frame.retained.elements;
    let placed = |record: &ElementRecord| match &moved {
        Some(moved) => moved.record(record),
        None => record.clone(),
    };

    if target.open.is_empty() {
        let motion = if moved.is_some() {
            IN_MOTION
        } else {
            previous_motion(kept.key, window)
        };
        let target = &mut window.next_frame.retained.elements;
        // Drawn again as a root: its subtree is taken over as it is, or,
        // when it was nested in another last frame or moved, cut out of
        // that one's.
        let subtree = if first == 0 && moved.is_none() {
            subtree.clone()
        } else {
            let from_prepaint = subtree.records[first].prepaint_range.start;
            let from_paint = subtree.records[first].paint_range.start;
            Rc::new(Subtree {
                records: subtree.records[first..=last]
                    .iter()
                    .map(|record| ElementRecord {
                        prepaint_range: record.prepaint_range.start.minus(from_prepaint)
                            ..record.prepaint_range.end.minus(from_prepaint),
                        paint_range: record.paint_range.start.minus(from_paint)
                            ..record.paint_range.end.minus(from_paint),
                        ..placed(record)
                    })
                    .collect(),
                by_key: OnceCell::new(),
            })
        };
        let root = target.push_root(Root {
            key: kept.key,
            records: RootRecords::Frozen(subtree),
            placement: None,
            prepaint_start: start,
            paint_start: paint_range.start,
            paint: Paint::Pending,
            usable: true,
            fresh: false,
            rest: Rest::default(),
            motion,
        });
        return Some(Phase::ReusedRoot {
            root,
            shift: target.push_shift(moved),
        });
    }

    // Drawn again inside an element built this frame: its records are copied
    // into that one's, placed in this frame's prepaint, and in last frame's
    // paint until it is painted.
    let index = target.building.len() as u32;
    target.reused = target.reused.wrapping_add(1);
    if moved.is_some() {
        target.roots[target.building_root as usize].motion = IN_MOTION;
    }
    let from_prepaint = subtree.records[first].prepaint_range.start;
    let to_prepaint = PrepaintAt::between(
        &target.roots[target.building_root as usize].prepaint_start,
        &start,
    );
    target
        .building
        .extend(subtree.records[first..=last].iter().map(|record| {
            ElementRecord {
                prepaint_range: record
                    .prepaint_range
                    .start
                    .minus(from_prepaint)
                    .plus(to_prepaint)
                    ..record
                        .prepaint_range
                        .end
                        .minus(from_prepaint)
                        .plus(to_prepaint),
                paint: match record.paint {
                    Paint::Painted => Paint::Pending,
                    paint => paint,
                },
                rest: Rest::default(),
                ..placed(record)
            }
        }));
    Some(Phase::ReusedNested {
        root: target.building_root,
        index,
        source: previous.root,
        shift: target.push_shift(moved),
    })
}

/// No shift: drawn again where it was.
const NO_SHIFT: u32 = u32::MAX;

/// How an element and those nested in it are drawn again moved: how their
/// primitives move, and where their records place them.
struct Moved {
    shift: Shift,
    /// Where what they paint, moved, is in [`ElementRecords::shifted`],
    /// once [`can_move`] moved it.
    operations: Range<u32>,
    /// The content mask they were drawn in, and the one they are drawn in
    /// now, when it did not move with them.
    content_masks: Option<(ContentMask<Pixels>, ContentMask<Pixels>)>,
}

impl Moved {
    /// `record`, of last frame, where it is drawn now.
    fn record(&self, record: &ElementRecord) -> ElementRecord {
        let mut context = record.context.clone();
        let offset = self.shift.by;
        context.bounds.origin += offset;
        let moved = Bounds {
            origin: context.content_mask.bounds.origin + offset,
            ..context.content_mask.bounds
        };
        context.content_mask = match &self.content_masks {
            None => ContentMask { bounds: moved },
            // The mask around it, or one inside it that moves (see
            // `moved_to`).
            Some((old, new)) if context.content_mask == *old => *new,
            Some((_, new)) => ContentMask {
                bounds: moved.intersect(&new.bounds),
            },
        };
        ElementRecord {
            context,
            noted: record.noted.moved(&self.shift),
            ..record.clone()
        }
    }
}

/// How last frame's record `previous` and those nested in it are drawn again
/// in `context`, where it moved with nothing else about it changed, if
/// drawing them afresh would paint what they painted, moved (see
/// [`crate::fast::shift`]).
fn moved_to(
    records: &ElementRecords,
    previous: PrevRef,
    context: &ElementContext,
    window: &Window,
) -> Option<Moved> {
    let subtree = records.subtree(previous.root)?;
    let first = previous.index as usize;
    let nested = &subtree.records[first..=first + subtree.records[first].nested as usize];
    let old = &nested[0].context;
    if old.bounds.size != context.bounds.size
        || old.opacity != context.opacity
        || old.rem_size != context.rem_size
        || !same_text_style(&old.text_style, &context.text_style)
    {
        return None;
    }
    let scale_factor = window.scale_factor();
    let offset = context.bounds.origin - old.bounds.origin;
    let scaled = offset.scale(scale_factor);
    if scaled.x.0.fract() != 0. || scaled.y.0.fract() != 0. {
        return None;
    }
    // Coordinates rounded half toward zero, or truncated, round alike only
    // on one side of zero: the elements' own, what their glyphs were placed
    // at.
    let clear = nested[0].noted.glyphs_clear_of_zero(scaled)
        && nested.iter().all(|record| {
            let origin = record.context.bounds.origin.scale(scale_factor);
            clear_of_zero(origin.x.0, scaled.x.0) && clear_of_zero(origin.y.0, scaled.y.0)
        });
    if !clear {
        return None;
    }
    let old_mask = old.content_mask.bounds;
    let mask_moved = Bounds {
        origin: old_mask.origin + offset,
        ..old_mask
    } == context.content_mask.bounds;
    let (masks, content_masks) = if mask_moved {
        (Masks::Moved, None)
    } else {
        // What an element inside clips lies inside the mask, clear of its
        // edges in device pixels, so that a primitive's mask is the mask
        // around, which the new one replaces, or a clip inside it, which
        // moves. What the mask around left out, it may no longer; what a
        // clip inside left out, it still does.
        let old_cover = window.cover_bounds(old_mask);
        let new_cover = window.cover_bounds(context.content_mask.bounds);
        let inside =
            |bounds: &Bounds<ScaledPixels>| known_inside(bounds, scaled, &old_cover, &new_cover);
        let noted = &nested[0].noted;
        let (sides, gap) = noted.culled_sides();
        let known = (noted.culled_in().is_none_or(|culled| inside(&culled))
            || still_beyond(
                sides,
                px(gap / scale_factor),
                offset,
                &old_mask,
                &context.content_mask.bounds,
            ) && still_beyond(sides, ScaledPixels(gap), scaled, &old_cover, &new_cover))
            && nested.iter().enumerate().all(|(index, record)| {
                (record.context.content_mask == old.content_mask
                    || inside(&window.cover_bounds(record.context.content_mask.bounds)))
                    && (!clips(&record.snapshot)
                        || nested[index + 1..=index + record.nested as usize]
                            .iter()
                            .all(|record| {
                                inside(&window.cover_bounds(record.context.content_mask.bounds))
                            }))
            });
        if !known {
            return None;
        }
        (
            Masks::Replaced {
                old: old_cover,
                new: new_cover,
            },
            Some((old.content_mask, context.content_mask)),
        )
    };
    Some(Moved {
        operations: 0..0,
        shift: Shift {
            offset: scaled,
            by: offset,
            masks,
        },
        content_masks,
    })
}

/// Whether an element's snapshot clips what is nested in it, or may: a
/// style not kept is taken to.
fn clips(snapshot: &Snapshot) -> bool {
    let style = match snapshot {
        Snapshot::Div { style, .. } | Snapshot::Svg { style, .. } => style,
        Snapshot::Text { .. } => return false,
        Snapshot::Moving | Snapshot::Resting => return true,
    };
    style.as_ref().is_none_or(|style| {
        [style.overflow.x, style.overflow.y]
            .into_iter()
            .any(|overflow| overflow.is_some_and(|overflow| overflow != Overflow::Visible))
    })
}

/// Moves what `prepaint` and `paint` of last frame hold as `shift` says,
/// onto `moved`, returning where it went, if it can be drawn again moved:
/// nothing placed but primitives, and those movable.
fn can_move(
    prepaint: &Range<PrepaintStateIndex>,
    paint: &Range<PaintIndex>,
    shift: &Shift,
    previous: &Scene,
    moved: &mut Vec<ShiftedOperation>,
) -> Option<Range<u32>> {
    let (from, to) = (&prepaint.start, &prepaint.end);
    let placed_in_prepaint = from.hitboxes_index != to.hitboxes_index
        || from.tooltips_index != to.tooltips_index
        || from.deferred_draws_index != to.deferred_draws_index;
    let (from, to) = (&paint.start, &paint.end);
    let placed_in_paint = from.fast_window_control_hitboxes_index
        != to.fast_window_control_hitboxes_index
        || from.mouse_listeners_index != to.mouse_listeners_index
        || from.input_handlers_index != to.input_handlers_index
        || from.cursor_styles_index != to.cursor_styles_index
        || from.tab_handle_index != to.tab_handle_index;
    let start = moved.len() as u32;
    (!placed_in_prepaint
        && !placed_in_paint
        && shift_operations(previous, from.scene_index..to.scene_index, shift, moved))
    .then(|| start..moved.len() as u32)
}

/// Whether a layout node `previous` or a record nested in it holds was
/// measured again this frame. A text laid out again may break into other
/// lines though its size comes out the same, so it is drawn again only while
/// its measurement stands as it was.
fn remeasured(records: &ElementRecords, previous: PrevRef, window: &Window) -> bool {
    let measured = &window.layout_engine.as_ref().unwrap().retention.measured;
    if measured.is_empty() {
        return false;
    }
    let subtree = records
        .subtree(previous.root)
        .expect("a record found is painted");
    let first = previous.index as usize;
    let last = first + subtree.records[first].nested as usize;
    subtree.records[first..=last]
        .iter()
        .any(|record| measured.contains(&record.layout_id))
}

/// Paints `drawable` as [`Drawable::paint`] does, recording what it paints,
/// or draws it again from last frame when its prepaint was.
#[inline(always)]
pub(crate) fn paint<E: Element>(drawable: &mut Drawable<E>, window: &mut Window, cx: &mut App) {
    if !retainable_type::<E>() {
        drawable.paint(window, cx);
        return;
    }
    match drawable.fast_retention.phase {
        Phase::Recorded { root, index } => {
            let start = window.paint_index();
            if index == 0 {
                window.next_frame.retained.elements.roots[root as usize].paint_start =
                    start.clone();
            }
            let in_motion = window.next_frame.retained.elements.roots[root as usize].motion > 0;
            let around = crate::fast::scene::begin_noting(&mut window.next_frame.scene, in_motion);
            drawable.paint(window, cx);
            let noted = crate::fast::scene::end_noting(&mut window.next_frame.scene, around);
            let end = window.paint_index();
            let elements = &mut window.next_frame.retained.elements;
            let root_start = &elements.roots[root as usize].paint_start;
            let range = PaintAt::between(root_start, &start)..PaintAt::between(root_start, &end);
            let record = &mut elements.pending(root)[index as usize];
            record.paint_range = range;
            record.paint = Paint::Painted;
            record.noted = noted;
            // Painted, the element needs its style no more, which its record
            // takes over.
            if let Snapshot::Div {
                style: style @ None,
                ..
            }
            | Snapshot::Svg {
                style: style @ None,
                ..
            } = &mut record.snapshot
                && let Some(built) = own(&mut drawable.element).as_mut().and_then(Own::style)
            {
                *style = Some(Rc::new(mem::take(built)));
            }
            if index == 0 {
                elements.freeze(root);
            }
        }
        Phase::ReusedRoot { root, shift } => reuse_root_paint(root, shift, window),
        Phase::ReusedNested {
            root,
            index,
            source,
            shift,
        } => reuse_nested_paint(root, index, source, shift, window),
        _ => {
            drawable.paint(window, cx);
        }
    }
}

/// Draws the root whose prepaint [`reuse_prepaint`] drew again as far as its
/// paint goes, moved as the shift at `shift` says, if it moved.
fn reuse_root_paint(root: u32, shift: u32, window: &mut Window) {
    let (source, noted) = {
        let root = &window.next_frame.retained.elements.roots[root as usize];
        let RootRecords::Frozen(subtree) = &root.records else {
            panic!("a root drawn again has a subtree");
        };
        let record = &subtree.records[0];
        let range = &record.paint_range;
        (
            range.start.at(&root.paint_start)..range.end.at(&root.paint_start),
            record.noted,
        )
    };
    let start = window.paint_index();
    reuse_paint(source.clone(), noted, shift, window);
    debug_assert!(
        window.paint_index() == source.end.shifted(&source.start, &start),
        "a reused paint range changed length"
    );
    let root = &mut window.next_frame.retained.elements.roots[root as usize];
    root.paint_start = start;
    root.paint = Paint::Painted;
}

/// Draws the records starting at `anchor` of the root `root`, whose
/// prepaint [`reuse_prepaint`] drew again, as far as their paint goes,
/// moved as the shift at `shift` says, if they moved.
fn reuse_nested_paint(root: u32, anchor: u32, source_root: u32, shift: u32, window: &mut Window) {
    let anchor = anchor as usize;
    let (from, nested, noted) = {
        let record = &window.next_frame.retained.elements.pending(root)[anchor];
        (
            record.paint_range.clone(),
            record.nested as usize,
            record.noted,
        )
    };
    let source = {
        let source_start =
            &window.rendered_frame.retained.elements.roots[source_root as usize].paint_start;
        from.start.at(source_start)..from.end.at(source_start)
    };
    let start = window.paint_index();
    reuse_paint(source.clone(), noted, shift, window);
    debug_assert!(
        window.paint_index() == source.end.shifted(&source.start, &start),
        "a reused paint range changed length"
    );
    let elements = &mut window.next_frame.retained.elements;
    let to = PaintAt::between(&elements.roots[root as usize].paint_start, &start);
    let records = elements.pending(root);
    for record in &mut records[anchor..=anchor + nested] {
        if record.paint == Paint::Pending {
            record.paint_range = record.paint_range.start.minus(from.start).plus(to)
                ..record.paint_range.end.minus(from.start).plus(to);
            record.paint = Paint::Painted;
        }
    }
}

/// Draws last frame's paint `range` again, moved as the shift at `shift`
/// says, if it moved, noting what painting it noted, `noted` where it lies
/// now, for the element being painted around it.
fn reuse_paint(range: Range<PaintIndex>, noted: Noted, shift: u32, window: &mut Window) {
    crate::fast::scene::add_noted(&mut window.next_frame.scene, noted);
    if shift == NO_SHIFT {
        window.reuse_paint(range);
        return;
    }
    let elements = &window.next_frame.retained.elements;
    let Shifted { by, operations } = &elements.shifts[shift as usize];
    let (by, operations) = (*by, operations.start as usize..operations.end as usize);
    // What [`Window::reuse_paint`] copies besides primitives, `can_move`
    // found none of, but for the debug bounds of tests.
    #[cfg(any(test, feature = "test-support"))]
    {
        for (selector, bounds) in &window.rendered_frame.debug_bounds_records
            [range.start.debug_bounds_index..range.end.debug_bounds_index]
        {
            window
                .next_frame
                .record_debug_bounds(selector.clone(), *bounds + by);
        }
    }
    window.next_frame.accessed_element_states.extend(
        window.rendered_frame.accessed_element_states
            [range.start.accessed_element_states_index..range.end.accessed_element_states_index]
            .iter()
            .map(|(id, type_id)| (id.clone(), *type_id)),
    );
    window
        .text_system()
        .reuse_layouts(range.start.line_layout_index..range.end.line_layout_index);
    let next = &mut window.next_frame;
    replay_shifted(&mut next.scene, &next.retained.elements.shifted[operations]);
}
