//! Edge fades: what is painted inside a region fading out toward its edges,
//! per pixel, where a clip would cut it.
//!
//! A gradient painted over content in the background's colour fades it only
//! over an opaque background of that colour. Over a window's glass, a video
//! or another tile there is no colour to paint, so the content itself fades:
//! the renderer multiplies what it draws, pixel by pixel, by one ramp per
//! edge ([`EdgeFadeRamps`]).
//!
//! # How a primitive carries its fade
//!
//! Every primitive kind already has four bytes of padding: `pad` in a
//! shadow, an underline and the three sprites, and the padding at the end of
//! a quad's and a path's [`crate::Background`]. A fade is a number in that
//! slot, an index into the window's table of fades, 0 for none, so a fade
//! adds no byte to any primitive: what retention copies every frame is what
//! it copied without fades. The table ([`WindowFades`]) holds each fade once;
//! the scene carries a copy for the renderer ([`crate::Scene::edge_fades`]),
//! which uploads it as one small buffer a frame. A primitive is given its
//! fade not as it is inserted but as the fade closes, everything inserted
//! since it opened at once ([`enter`]), so painting outside any fade does
//! nothing it did not do before.
//!
//! An index is the fade's slot in its low 16 bits and the slot's generation
//! in the high 16. A fade keeps its slot while anything the next frame can
//! draw again from refers to it, so a primitive drawn again from last frame
//! keeps a valid index; slots nothing refers to any more are swept once
//! enough fades were added ([`WindowFades::finish_frame`]). A renderer reads
//! only the slot, and draws a slot past the end of the table unfaded, so a
//! scene built without a window, whose primitives all hold 0, is never
//! misread.
//!
//! # The ramps
//!
//! A ramp is linear in the device pixel's coordinate along its edge's axis,
//! rising from its edge, clamped to 0..1 and eased with smoothstep, so it
//! leaves the content at full strength without a crease. An edge that does
//! not fade has a rate of zero and starts at one. A fade's region is snapped
//! out to whole device pixels, as a clip's is, so moving what fades by whole
//! device pixels moves its edges exactly.
//!
//! Fades nest edge by edge: a list fading at its top and bottom holds rows
//! whose titles fade at the right, and a title fades at all three. Two fades
//! of one edge can't be told as one ramp; the one reaching further in wins,
//! which is the one that matters wherever both reach.
//!
//! # Retention
//!
//! Views, elements and keyed stretches note the fade they were drawn in, and
//! are drawn again from last frame only in the same one. One drawn again
//! moved ([`crate::fast::shift`]) moves the ramps that were set inside what
//! moved and takes the ramps around it from where it is drawn now
//! ([`EdgeFadeRamps::moved`]).
//!
//! A fade that deepens with what lies hidden past its edges
//! ([`EdgeFadeElement::hidden_by_list`], [`EdgeFadeElement::hidden_by_scroll`])
//! is known only once its child is laid out, as the child prepaints. The
//! child is prepainted in the fade from before; when the fade it lays out to
//! differs, what retention noted as it prepainted is moved into that fade
//! ([`FadeShift`] with no move), it is painted in that fade, and whatever
//! its paint drew in the old one is moved as well. The frame is drawn in
//! the fade its child lays out to, and the next one finds everything noted
//! in it.
//!
//! A scroll layer's content is painted into the layer's own scene, which
//! starts with no fade, and the fade around the container is laid over the
//! layer's tiles where they are composited, so the tiles move under it. A
//! layer whose content sets fades of its own is drawn without its tiles,
//! since those ramps would move with the tiles' content.

use crate::fast::shift::ShiftedOperation;
use crate::{
    AnyElement, App, Bounds, Edges, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ListState, Pixels, Point, ScaledPixels, Scene, ScrollHandle, Window,
    scene::Primitive,
};
use collections::FxHashMap;

/// How what is painted inside a region fades out toward the region's edges
/// ([`edge_fade`], [`Window::with_edge_fade`]).
///
/// Each edge fades over `width` in from it, to `1 - depth` at the edge: a
/// depth of 1 fades it to nothing, 0 leaves it sharp. The fade is drawn per
/// pixel, so it fades text, images and fills over any background, glass and
/// video included.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeFade {
    /// How far in from each edge the fade reaches; zero leaves an edge sharp.
    pub width: Edges<Pixels>,
    /// How faint each edge gets, from 0, not at all, to 1, faded to nothing.
    pub depth: Edges<f32>,
}

impl EdgeFade {
    /// Every edge with a width fades to nothing at the edge.
    pub fn new(width: impl Into<Edges<Pixels>>) -> Self {
        Self {
            width: width.into(),
            depth: Edges::all(1.),
        }
    }

    /// The top and bottom edges fade over `width`: a scrolling list or
    /// transcript.
    pub fn y(width: Pixels) -> Self {
        Self::new(Edges {
            top: width,
            bottom: width,
            ..Edges::default()
        })
    }

    /// The left and right edges fade over `width`: a strip scrolled
    /// sideways, or a title running past its end.
    pub fn x(width: Pixels) -> Self {
        Self::new(Edges {
            left: width,
            right: width,
            ..Edges::default()
        })
    }

    /// This fade with each edge's depth multiplied by `depth`'s.
    pub fn depth(mut self, depth: Edges<f32>) -> Self {
        self.depth = Edges {
            top: self.depth.top * depth.top,
            right: self.depth.right * depth.right,
            bottom: self.depth.bottom * depth.bottom,
            left: self.depth.left * depth.left,
        };
        self
    }

    /// This fade with each edge as deep as content lies hidden past it: none
    /// while nothing is, deepening as more is, in full once a whole width
    /// is. `hidden` is how far content runs past each edge.
    pub fn hidden(self, hidden: Edges<Pixels>) -> Self {
        let part = |hidden: Pixels, width: Pixels| {
            if width <= Pixels::ZERO {
                return 0.;
            }
            smoothstep((hidden / width).clamp(0., 1.))
        };
        self.depth(Edges {
            top: part(hidden.top, self.width.top),
            right: part(hidden.right, self.width.right),
            bottom: part(hidden.bottom, self.width.bottom),
            left: part(hidden.left, self.width.left),
        })
    }
}

/// `child`, painted fading out toward its edges as `fade` says. See
/// [`EdgeFade`] and [`Window::with_edge_fade`].
///
/// The fade covers everything `child` paints, its own background included;
/// put it around the content that scrolls, inside what paints the surface
/// behind it.
pub fn edge_fade(child: impl IntoElement, fade: EdgeFade) -> EdgeFadeElement {
    EdgeFadeElement {
        child: child.into_any_element(),
        fade,
        hidden: None,
    }
}

/// An element painting its child fading out toward its edges, made by
/// [`edge_fade`].
pub struct EdgeFadeElement {
    child: AnyElement,
    fade: EdgeFade,
    hidden: Option<Hidden>,
}

/// What tells how far content runs past each edge of an [`EdgeFadeElement`].
enum Hidden {
    Scroll(ScrollHandle),
    List(ListState),
}

impl Hidden {
    fn edges(&self) -> Edges<Pixels> {
        let (offset, max) = match self {
            Hidden::Scroll(handle) => (handle.offset(), handle.max_offset()),
            Hidden::List(list) => (
                list.scroll_px_offset_for_scrollbar(),
                list.max_offset_for_scrollbar(),
            ),
        };
        let zero = Pixels::ZERO;
        Edges {
            top: (-offset.y).max(zero),
            right: (max.x + offset.x).max(zero),
            bottom: (max.y + offset.y).max(zero),
            left: (-offset.x).max(zero),
        }
    }
}

impl EdgeFadeElement {
    /// Fades each edge only as far as the content `handle` scrolls lies
    /// hidden past it (see [`EdgeFade::hidden`]).
    pub fn hidden_by_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.hidden = Some(Hidden::Scroll(handle.clone()));
        self
    }

    /// Fades each edge only as far as the rows of `list` lie hidden past it
    /// (see [`EdgeFade::hidden`]).
    pub fn hidden_by_list(mut self, list: &ListState) -> Self {
        self.hidden = Some(Hidden::List(list.clone()));
        self
    }

    fn resolved(&self) -> EdgeFade {
        match &self.hidden {
            Some(hidden) => self.fade.hidden(hidden.edges()),
            None => self.fade,
        }
    }
}

impl IntoElement for EdgeFadeElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// What [`EdgeFadeElement`] prepainted its child in, for its paint.
pub struct EdgeFadePrepaint {
    /// The fade its child is painted in.
    fade: EdgeFade,
    /// How the fades its child was prepainted in become those it is painted
    /// in, when they differ (see [`EdgeFadeElement`]'s `prepaint`).
    refade: Option<FadeShift>,
}

impl Element for EdgeFadeElement {
    type RequestLayoutState = ();
    type PrepaintState = EdgeFadePrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> EdgeFadePrepaint {
        // Prepaint and paint run in the same fade, so that what retention
        // notes as the child prepaints is what its paint draws in.
        let guess = self.resolved();
        let notes = self.hidden.is_some().then(|| PrepaintNotes::mark(window));
        window.with_edge_fade(bounds, guess, |window| self.child.prepaint(window, cx));
        let Some(notes) = notes else {
            return EdgeFadePrepaint {
                fade: guess,
                refade: None,
            };
        };
        // What lies hidden is known once the child is laid out: a list lays
        // out its rows as it prepaints, a scroll container works out how far
        // it scrolls. What was noted in the fade from before is moved into
        // the one it lays out to, and the child is painted in that one.
        let fade = self.resolved();
        let refade = (fade != guess)
            .then(|| FadeShift {
                old: fade_in(window, bounds, guess),
                new: fade_in(window, bounds, fade),
                delta: Point::default(),
            })
            .filter(|shift| shift.old != shift.new);
        if let Some(shift) = &refade {
            notes.refade(window, shift);
        }
        EdgeFadePrepaint { fade, refade }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut EdgeFadePrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let notes = prepaint.refade.map(|_| PaintNotes::mark(window));
        window.with_edge_fade(bounds, prepaint.fade, |window| self.child.paint(window, cx));
        if let (Some(notes), Some(shift)) = (notes, &prepaint.refade) {
            notes.refade(window, shift);
        }
    }
}

/// The fade painting `bounds` in `fade` is in, inside the one painting is
/// in now: the index [`Window::with_edge_fade`] enters, or the one around
/// when `fade` fades nothing.
fn fade_in(window: &mut Window, bounds: Bounds<Pixels>, fade: EdgeFade) -> u32 {
    entered(window, bounds, fade).unwrap_or_else(|| current(window))
}

/// The index [`Window::with_edge_fade`] enters for `fade` around `bounds`,
/// or `None` when it fades nothing and painting stays in the fade around.
fn entered(window: &mut Window, bounds: Bounds<Pixels>, fade: EdgeFade) -> Option<u32> {
    let ramps = EdgeFadeRamps::new(
        window.cover_bounds(bounds),
        fade.width.scale(window.scale_factor()),
        fade.depth,
    );
    if !ramps.fades() {
        return None;
    }
    let outer = current(window);
    Some(if outer == NONE {
        window.fast_edge_fade.intern(&ramps)
    } else {
        let around = window
            .fast_edge_fade
            .ramps(outer)
            .unwrap_or(&EdgeFadeRamps::NONE);
        let within = around.within(&ramps);
        window.fast_edge_fade.intern(&within)
    })
}

/// Where what is noted of the fades as an element prepaints begins: the
/// retained views and the elements recorded.
struct PrepaintNotes {
    views: usize,
    elements: crate::fast::element::RecordsMark,
}

impl PrepaintNotes {
    fn mark(window: &Window) -> Self {
        let retained = &window.next_frame.retained;
        Self {
            views: retained.records.len(),
            elements: crate::fast::element::mark(&retained.elements),
        }
    }

    /// Moves what was noted since the mark from the fades `shift` moves
    /// from to those it moves to, so that the next frame, drawn in those,
    /// finds it drawn in them.
    fn refade(self, window: &mut Window, shift: &FadeShift) {
        let mut refade = Refade::new(shift, &mut window.fast_edge_fade);
        let retained = &mut window.next_frame.retained;
        crate::fast::retained::refade(&mut retained.records[self.views..], &mut |fade| {
            refade.map(fade)
        });
        crate::fast::element::refade(&mut retained.elements, &self.elements, &mut |fade| {
            refade.map(fade)
        });
    }
}

/// Where what an element paints begins in the scene: its primitives and
/// the stretches it paints under keys.
struct PaintNotes {
    starts: [u32; KINDS],
    operations: usize,
}

impl PaintNotes {
    fn mark(window: &Window) -> Self {
        let scene = &window.next_frame.scene;
        Self {
            starts: lengths(scene),
            operations: scene.paint_operations.len(),
        }
    }

    /// Moves what was painted since the mark, and the stretches painted
    /// under keys, from the fades `shift` moves from to those it moves to.
    ///
    /// Painted in the fade it is moved to, a primitive is in it already.
    /// Drawn again from last frame, or inside a view built again where it
    /// was, it is in the fade it was prepainted in.
    fn refade(self, window: &mut Window, shift: &FadeShift) {
        let mut refade = Refade::new(shift, &mut window.fast_edge_fade);
        let scene = &mut window.next_frame.scene;
        map_since(scene, &self.starts, &mut |fade| refade.map(fade));
        crate::fast::keyed::refade(
            &mut scene.fast_painted.keyed,
            self.operations..scene.paint_operations.len(),
            &mut |fade| refade.map(fade),
        );
    }
}

/// A [`FadeShift`] in place, remembering what each fade became.
struct Refade<'a> {
    shift: &'a FadeShift,
    fades: &'a mut WindowFades,
    known: FxHashMap<u32, u32>,
}

impl<'a> Refade<'a> {
    fn new(shift: &'a FadeShift, fades: &'a mut WindowFades) -> Self {
        Self {
            shift,
            fades,
            known: FxHashMap::default(),
        }
    }

    fn map(&mut self, fade: u32) -> u32 {
        if fade == self.shift.old {
            return self.shift.new;
        }
        if fade == self.shift.new {
            return fade;
        }
        if let Some(&known) = self.known.get(&fade) {
            return known;
        }
        // Noted this frame, so its slot is held.
        let mapped = self.shift.map_slow(fade, self.fades).unwrap_or(fade);
        self.known.insert(fade, mapped);
        mapped
    }
}

impl Window {
    /// Runs `f`, painting what it paints fading out toward the edges of
    /// `bounds` as `fade` says. Call it around both the prepaint and the
    /// paint of what fades, with the same arguments, as [`edge_fade`]
    /// does: retention notes the fade in prepaint.
    ///
    /// Content past an edge fades on to nothing; clip it with
    /// [`Window::with_content_mask`] where it should stop at the edge.
    /// Fades nest edge by edge: where an enclosing fade and this one fade
    /// the same edge, the one reaching further in is drawn.
    pub fn with_edge_fade<R>(
        &mut self,
        bounds: Bounds<Pixels>,
        fade: EdgeFade,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let Some(inner) = entered(self, bounds, fade) else {
            return f(self);
        };
        self.fast_edge_fade.applied += 1;
        enter(&mut self.next_frame.scene, inner);
        let result = f(self);
        leave(&mut self.next_frame.scene);
        result
    }
}

impl Scene {
    /// The table of edge fades the primitives of this scene index, as the
    /// renderers upload it: entry 0 holds, in the bits of its first
    /// float, how many entries the table has, and every slot from 1 is an
    /// [`EdgeFadeRamps`]. Empty for a scene without fades.
    ///
    /// A primitive's fade is the low 16 bits of the four bytes of padding
    /// it carries (`pad`, or its background's padding for a quad and a
    /// path); 0, or a slot past the table's end, is no fade.
    pub fn edge_fades(&self) -> &[EdgeFadeRamps] {
        &self.fast_painted.fades.table
    }
}

/// No fade: the index every primitive holds unless painted in one.
pub(crate) const NONE: u32 = 0;

#[cfg(any(test, feature = "test-support"))]
/// Takes `primitive`'s fade away.
pub(crate) fn unfade(primitive: &mut Primitive) {
    if let Some(slot) = slot_of(primitive) {
        *slot = NONE;
    }
}

#[cfg(any(test, feature = "test-support"))]
/// The fade `primitive` is drawn in.
pub(crate) fn fade_of(primitive: &Primitive) -> u32 {
    let mut primitive = primitive.clone();
    slot_of(&mut primitive).map_or(NONE, |slot| *slot)
}

/// The fade of a primitive: its slot of padding.
#[inline]
fn slot_of(primitive: &mut Primitive) -> Option<&mut u32> {
    Some(match primitive {
        Primitive::Shadow(shadow) => &mut shadow.pad,
        Primitive::Quad(quad) => &mut quad.background.pad,
        Primitive::Path(path) => &mut path.color.pad,
        Primitive::Underline(underline) => &mut underline.pad,
        Primitive::MonochromeSprite(sprite) => &mut sprite.pad,
        Primitive::SubpixelSprite(sprite) => &mut sprite.pad,
        Primitive::PolychromeSprite(sprite) => &mut sprite.pad,
        // A surface is composited by the system's video path, which takes
        // no fade.
        Primitive::Surface(_) => return None,
    })
}

#[cfg(any(test, feature = "test-support"))]
/// Gives `primitive` the fade `fade` unless it holds one.
pub(crate) fn stamp_with(fade: u32, primitive: &mut Primitive) {
    if let Some(slot) = slot_of(primitive)
        && *slot == NONE
    {
        *slot = fade;
    }
}

/// The fade painting is in now.
#[inline(always)]
pub(crate) fn current(window: &Window) -> u32 {
    window.next_frame.scene.fast_painted.fades.current
}

/// The kinds of primitive that take a fade: all but surfaces, which the
/// system's video path composites.
const KINDS: usize = 7;

/// A fade painting entered in a scene, and how long each list of
/// primitives that takes a fade was as it did.
#[derive(Clone, Copy)]
struct Open {
    fade: u32,
    starts: [u32; KINDS],
}

/// Paints what `scene` is given next in `fade`, until [`leave`].
///
/// A primitive is not stamped as it is inserted but when the fade it was
/// painted in closes, all at once: a primitive inserted outside any fade
/// then costs nothing, where checking for a fade as each is inserted made
/// a frame painting a strip of terminals 0.4% dearer.
pub(crate) fn enter(scene: &mut Scene, fade: u32) {
    let starts = lengths(scene);
    let fades = &mut scene.fast_painted.fades;
    fades.open.push(Open { fade, starts });
    fades.current = fade;
}

/// Closes the fade [`enter`] opened last, giving what was painted in it its
/// fade.
pub(crate) fn leave(scene: &mut Scene) {
    let Some(open) = scene.fast_painted.fades.open.pop() else {
        return;
    };
    stamp_since(scene, &open);
    let fades = &mut scene.fast_painted.fades;
    fades.current = fades.open.last().map_or(NONE, |open| open.fade);
}

/// Gives what was painted in the fades still open the fade it is in, for
/// what reads `scene` before they close: a scroll layer baking the
/// background it is painted over.
pub(crate) fn settle(scene: &mut Scene) {
    for ix in (0..scene.fast_painted.fades.open.len()).rev() {
        let open = scene.fast_painted.fades.open[ix];
        stamp_since(scene, &open);
    }
}

/// Gives each primitive inserted since `open` was entered its fade, unless
/// it holds one: one drawn again from last frame, moved or not, holds what
/// it was drawn with, which is this fade or one set inside it, and one
/// painted in a fade nested in this one was given that one as it closed.
fn stamp_since(scene: &mut Scene, open: &Open) {
    let fade = open.fade;
    if fade == NONE {
        return;
    }
    map_since(scene, &open.starts, &mut |slot| {
        if slot == NONE { fade } else { slot }
    });
}

/// How long each list of primitives that takes a fade is.
fn lengths(scene: &Scene) -> [u32; KINDS] {
    [
        scene.shadows.len(),
        scene.quads.len(),
        scene.paths.len(),
        scene.underlines.len(),
        scene.monochrome_sprites.len(),
        scene.subpixel_sprites.len(),
        scene.polychrome_sprites.len(),
    ]
    .map(|len| len as u32)
}

/// Gives each primitive inserted since the lists were `starts` long the
/// fade `map` makes of its own.
#[inline]
fn map_since(scene: &mut Scene, starts: &[u32; KINDS], map: &mut impl FnMut(u32) -> u32) {
    #[inline(always)]
    fn each<T>(
        list: &mut [T],
        start: u32,
        map: &mut impl FnMut(u32) -> u32,
        slot: impl Fn(&mut T) -> &mut u32,
    ) {
        for item in list.get_mut(start as usize..).unwrap_or_default() {
            let slot = slot(item);
            *slot = map(*slot);
        }
    }
    let [
        shadows,
        quads,
        paths,
        underlines,
        monochrome,
        subpixel,
        polychrome,
    ] = *starts;
    each(&mut scene.shadows, shadows, map, |shadow| &mut shadow.pad);
    each(&mut scene.quads, quads, map, |quad| {
        &mut quad.background.pad
    });
    each(&mut scene.paths, paths, map, |path| &mut path.color.pad);
    each(&mut scene.underlines, underlines, map, |underline| {
        &mut underline.pad
    });
    each(&mut scene.monochrome_sprites, monochrome, map, |sprite| {
        &mut sprite.pad
    });
    each(&mut scene.subpixel_sprites, subpixel, map, |sprite| {
        &mut sprite.pad
    });
    each(&mut scene.polychrome_sprites, polychrome, map, |sprite| {
        &mut sprite.pad
    });
}

/// How many times painting entered a fade, for a scroll layer to tell
/// whether its content set fades of its own.
#[inline(always)]
pub(crate) fn applied(window: &Window) -> u64 {
    window.fast_edge_fade.applied
}

/// Whether a quad, the background a scroll layer bakes into its tiles, is
/// drawn faded.
#[inline]
pub(crate) fn quad_fades(quad: &crate::Quad) -> bool {
    quad.background.pad != NONE
}

/// What a scene holds of the fades: those painting is in, and the copy of
/// the window's table renderers upload.
#[derive(Default)]
pub(crate) struct SceneFades {
    /// The fade painting is in, the innermost of `open`'s.
    current: u32,
    /// The fades painting entered and has not left, innermost last.
    open: Vec<Open>,
    /// The window's table, as [`crate::Scene::edge_fades`] hands it out.
    table: Vec<EdgeFadeRamps>,
    /// The [`WindowFades::version`] `table` was copied at.
    version: u64,
}

/// Lays the fade `outer`, around a scroll layer drawn now, over the fade of
/// `primitive`, painted into the layer's content: a primitive with no fade
/// of its own gets `outer` as the fade it is drawn in closes (see
/// [`enter`]), and one with its own gets both.
pub(crate) fn compose(fades: &mut WindowFades, outer: u32, primitive: &mut Primitive) {
    if outer == NONE {
        return;
    }
    let Some(slot) = slot_of(primitive) else {
        return;
    };
    if *slot == NONE {
        return;
    }
    let around = fades.ramps(outer).copied().unwrap_or(EdgeFadeRamps::NONE);
    let own = fades.ramps(*slot).copied().unwrap_or(EdgeFadeRamps::NONE);
    *slot = fades.intern(&around.within(&own));
}

/// The ramps of an edge fade, in device pixels, as the renderers read them.
///
/// For the left, top, right and bottom edges in that order, the ramp at a
/// pixel is `start + rate × (coordinate − edge)`, the coordinate being the
/// pixel centre's `x` at the left and right and `y` at the top and bottom,
/// clamped to 0..1 and eased with smoothstep; what is drawn is multiplied
/// by all four. An edge that does not fade has a rate of 0 and a start of 1.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct EdgeFadeRamps {
    /// Where each edge lies: a whole device pixel.
    pub edge: [f32; 4],
    /// How fast each ramp rises per device pixel: positive at the left and
    /// top, negative at the right and bottom.
    pub rate: [f32; 4],
    /// Each ramp at its edge.
    pub start: [f32; 4],
}

impl Default for EdgeFadeRamps {
    fn default() -> Self {
        Self::NONE
    }
}

impl EdgeFadeRamps {
    /// No edge fades.
    pub const NONE: Self = Self {
        edge: [0.; 4],
        rate: [0.; 4],
        start: [1.; 4],
    };

    /// Whether any edge fades.
    pub fn fades(&self) -> bool {
        self.rate.iter().any(|rate| *rate != 0.)
    }

    /// The ramps of a fade in `region`, whose edges lie on whole device
    /// pixels: each edge `width` wide, `depth` deep at the edge (0 not at
    /// all, 1 to nothing).
    pub fn new(
        region: Bounds<ScaledPixels>,
        width: Edges<ScaledPixels>,
        depth: Edges<f32>,
    ) -> Self {
        let mut ramps = Self::NONE;
        let edges = [
            (region.left().0, width.left.0, depth.left, 1.),
            (region.top().0, width.top.0, depth.top, 1.),
            (region.right().0, width.right.0, depth.right, -1.),
            (region.bottom().0, width.bottom.0, depth.bottom, -1.),
        ];
        for (slot, (edge, width, depth, inward)) in edges.into_iter().enumerate() {
            if !(width > 0. && depth > 0.) || !edge.is_finite() || !width.is_finite() {
                continue;
            }
            // At the edge the eased ramp is `1 - depth`; a width in, it is 1.
            let start = unsmoothstep(1. - depth.min(1.));
            let mut rate = (1. - start) / width;
            // Whole a width in, not a rounding short of it.
            while start + rate * width < 1. {
                rate = rate.next_up();
            }
            if rate == 0. {
                continue;
            }
            ramps.edge[slot] = edge;
            ramps.rate[slot] = inward * rate;
            ramps.start[slot] = start;
        }
        ramps
    }

    /// One ramp of these: its edge, rate and start.
    fn ramp(&self, slot: usize) -> (f32, f32, f32) {
        (self.edge[slot], self.rate[slot], self.start[slot])
    }

    fn set_ramp(&mut self, slot: usize, (edge, rate, start): (f32, f32, f32)) {
        self.edge[slot] = edge;
        self.rate[slot] = rate;
        self.start[slot] = start;
    }

    /// How far a ramp reaches in from its side, along the inward direction:
    /// where it reaches 1, signed so that further in is larger.
    fn reach((edge, rate, start): (f32, f32, f32)) -> f32 {
        (edge + (1. - start) / rate) * rate.signum()
    }

    /// These ramps with `inner`'s laid over them, edge by edge: where both
    /// fade an edge, the ramp that reaches further in.
    pub(crate) fn within(&self, inner: &Self) -> Self {
        let mut ramps = *self;
        for slot in 0..4 {
            let (outer, inner) = (self.ramp(slot), inner.ramp(slot));
            if inner.1 != 0. && (outer.1 == 0. || Self::reach(inner) > Self::reach(outer)) {
                ramps.set_ramp(slot, inner);
            }
        }
        ramps
    }

    /// The ramps of a primitive drawn under `old` moved by `delta` to where
    /// it is drawn under `new`: an edge's ramp that is `old`'s came from
    /// around what moved and becomes `new`'s; any other was set inside what
    /// moved, and moves with it.
    pub(crate) fn moved(&self, delta: Point<ScaledPixels>, old: &Self, new: &Self) -> Self {
        if self == old {
            return *new;
        }
        let mut ramps = *self;
        for slot in 0..4 {
            let ramp = self.ramp(slot);
            if ramp == old.ramp(slot) {
                ramps.set_ramp(slot, new.ramp(slot));
            } else if ramp.1 != 0. {
                ramps.edge[slot] += if slot % 2 == 0 { delta.x.0 } else { delta.y.0 };
            }
        }
        ramps
    }

    /// How strongly a pixel centred at `point` is drawn, as the renderers
    /// work it out.
    pub fn alpha_at(&self, point: Point<ScaledPixels>) -> f32 {
        (0..4)
            .map(|slot| {
                let coordinate = if slot % 2 == 0 { point.x.0 } else { point.y.0 };
                let (edge, rate, start) = self.ramp(slot);
                smoothstep((start + rate * (coordinate - edge)).clamp(0., 1.))
            })
            .product()
    }

    /// The bits these ramps are told apart by, with no edge that does not
    /// fade making a difference.
    fn key(&self) -> [u32; 12] {
        let mut key = [0; 12];
        for slot in 0..4 {
            if self.rate[slot] != 0. {
                key[slot] = self.edge[slot].to_bits();
                key[4 + slot] = self.rate[slot].to_bits();
                key[8 + slot] = self.start[slot].to_bits();
            }
        }
        key
    }

    /// These ramps with every edge that does not fade as [`Self::NONE`]
    /// has it.
    fn canonical(&self) -> Self {
        let mut ramps = Self::NONE;
        for slot in 0..4 {
            if self.rate[slot] != 0. {
                ramps.set_ramp(slot, self.ramp(slot));
            }
        }
        ramps
    }
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3. - 2. * t)
}

/// Where smoothstep is `value`, for `value` in 0..1.
fn unsmoothstep(value: f32) -> f32 {
    0.5 - ((1. - 2. * value).asin() / 3.).sin()
}

/// Fades are added this many times at least between two sweeps of the table.
const SWEEP_AFTER: usize = 256;

/// The most slots a table holds: an index keeps its slot in 16 bits.
const SLOTS: usize = 1 << 16;

/// A window's table of edge fades, which primitives index. See the
/// [module](self).
pub(crate) struct WindowFades {
    /// Each slot's fade; slot 0 is no fade, and a free slot's is stale.
    ramps: Vec<EdgeFadeRamps>,
    /// Each slot's generation, bumped when it is freed, so that an index
    /// kept past its slot's sweep is told from the slot's next fade.
    generations: Vec<u16>,
    /// The index of each fade held, by its [`EdgeFadeRamps::key`].
    ids: FxHashMap<[u32; 12], u32>,
    /// The slots swept free, reused before the table grows.
    free: Vec<u32>,
    /// Fades added since the table was last swept.
    added: usize,
    /// Slots in use after the last sweep.
    live: usize,
    /// Bumped whenever a slot is given a fade, for scenes to tell their
    /// copy is current.
    version: u64,
    /// How many times painting entered a fade. See [`applied`].
    applied: u64,
    #[cfg(test)]
    sweeps: usize,
}

impl Default for WindowFades {
    fn default() -> Self {
        Self {
            ramps: vec![EdgeFadeRamps::NONE],
            generations: vec![0],
            ids: FxHashMap::default(),
            free: Vec::new(),
            added: 0,
            live: 0,
            version: 0,
            applied: 0,
            #[cfg(test)]
            sweeps: 0,
        }
    }
}

impl WindowFades {
    /// The index of `ramps`, giving it a slot if it has none.
    pub(crate) fn intern(&mut self, ramps: &EdgeFadeRamps) -> u32 {
        if !ramps.fades() {
            return NONE;
        }
        let key = ramps.key();
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let slot = match self.free.pop() {
            Some(slot) => slot as usize,
            None if self.ramps.len() < SLOTS => {
                self.ramps.push(EdgeFadeRamps::NONE);
                self.generations.push(0);
                self.ramps.len() - 1
            }
            // Sixty-five thousand fades at once: what does not fit is drawn
            // unfaded rather than misread.
            None => return NONE,
        };
        self.ramps[slot] = ramps.canonical();
        let id = slot as u32 | u32::from(self.generations[slot]) << 16;
        self.ids.insert(key, id);
        self.added += 1;
        self.version += 1;
        id
    }

    /// The fade `id` indexes, unless its slot was swept since.
    pub(crate) fn ramps(&self, id: u32) -> Option<&EdgeFadeRamps> {
        let slot = (id & 0xffff) as usize;
        (slot < self.ramps.len() && u32::from(self.generations[slot]) == id >> 16)
            .then(|| &self.ramps[slot])
    }

    /// Frees the slots no index in `live` refers to: `live` holds, for each
    /// slot, whether something the next frame can draw from refers to it.
    fn sweep(&mut self, live: &[bool]) {
        let mut kept = 0;
        for slot in 1..self.ramps.len() {
            if live.get(slot).copied().unwrap_or(false) {
                kept += 1;
                continue;
            }
            let id = slot as u32 | u32::from(self.generations[slot]) << 16;
            if self.ids.remove(&self.ramps[slot].key()) == Some(id) {
                self.generations[slot] = self.generations[slot].wrapping_add(1);
                self.free.push(slot as u32);
            }
        }
        self.added = 0;
        self.live = kept;
        #[cfg(test)]
        {
            self.sweeps += 1;
        }
    }

    /// How many times the table was swept.
    #[cfg(test)]
    pub(crate) fn sweeps(&self) -> usize {
        self.sweeps
    }
}

/// Ends the fades' part of the frame being drawn: sweeps the table if
/// enough fades were added since it last was, and gives the frame's scene
/// a copy of it for the renderer.
pub(crate) fn finish_frame(window: &mut Window) {
    debug_assert!(
        window.next_frame.scene.fast_painted.fades.open.is_empty(),
        "a fade was left open as the frame ended"
    );
    if window.fast_edge_fade.added >= SWEEP_AFTER.max(window.fast_edge_fade.live) {
        let live = live_slots(window);
        window.fast_edge_fade.sweep(&live);
    }
    let fades = &window.fast_edge_fade;
    let scene = &mut window.next_frame.scene.fast_painted.fades;
    if scene.version == fades.version {
        return;
    }
    scene.table.clear();
    scene.table.extend_from_slice(&fades.ramps);
    scene.table[0].edge[0] = f32::from_bits(fades.ramps.len() as u32);
    scene.version = fades.version;
}

/// For each slot of the window's table, whether anything the next frame can
/// draw again from refers to it: the frame's primitives, the fades its
/// views, elements and keyed stretches were drawn in, and the scroll
/// layers' content.
fn live_slots(window: &Window) -> Vec<bool> {
    let mut live = vec![false; window.fast_edge_fade.ramps.len()];
    let mut mark = |id: u32| {
        if let Some(slot) = live.get_mut((id & 0xffff) as usize) {
            *slot = true;
        }
    };
    mark_scene(&window.next_frame.scene, &mut mark);
    for record in &window.next_frame.retained.records {
        mark(record.context.fade);
    }
    crate::fast::element::for_each_fade(&window.next_frame.retained.elements, &mut mark);
    crate::fast::keyed::for_each_fade(&window.next_frame.scene.fast_painted.keyed, &mut mark);
    for layer in window.fast_layers.layers.values() {
        crate::fast::layers::for_each_fade(layer, &mut mark);
    }
    live
}

/// Calls `mark` with the fade of every primitive of `scene`.
pub(crate) fn mark_scene(scene: &Scene, mark: &mut impl FnMut(u32)) {
    scene.shadows.iter().for_each(|p| mark(p.pad));
    scene.quads.iter().for_each(|p| mark(p.background.pad));
    scene.paths.iter().for_each(|p| mark(p.color.pad));
    scene.underlines.iter().for_each(|p| mark(p.pad));
    scene.monochrome_sprites.iter().for_each(|p| mark(p.pad));
    scene.subpixel_sprites.iter().for_each(|p| mark(p.pad));
    scene.polychrome_sprites.iter().for_each(|p| mark(p.pad));
}

/// How the fades of a stretch drawn again moved change: those of the
/// region around it, `old` where it was drawn and `new` where it is, and
/// the move in device pixels. See [`EdgeFadeRamps::moved`].
#[derive(Clone, Copy)]
pub(crate) struct FadeShift {
    pub(crate) old: u32,
    pub(crate) new: u32,
    pub(crate) delta: Point<ScaledPixels>,
}

impl FadeShift {
    /// The fade a primitive or record drawn in `id` is drawn in moved, if
    /// it can be told: an index swept since it was drawn can't.
    #[inline(always)]
    pub(crate) fn map(&self, id: u32, fades: &mut WindowFades) -> Option<u32> {
        if id == self.old {
            return Some(self.new);
        }
        self.map_slow(id, fades)
    }

    #[inline(never)]
    fn map_slow(&self, id: u32, fades: &mut WindowFades) -> Option<u32> {
        let ramps = *fades.ramps(id)?;
        let old = *fades.ramps(self.old)?;
        let new = *fades.ramps(self.new)?;
        Some(fades.intern(&ramps.moved(self.delta, &old, &new)))
    }
}

/// Gives the primitives of a stretch drawn again moved, `moved`, the fades
/// they are drawn in where they are now; false if one can't be told.
///
/// While the window holds no fade every primitive holds none, drawn
/// anywhere, so moving costs nothing more for the fades it doesn't use.
#[inline(always)]
pub(crate) fn shift_fades(
    shift: &FadeShift,
    moved: &mut [ShiftedOperation],
    fades: &mut WindowFades,
) -> bool {
    fades.ids.is_empty() || shift_held_fades(shift, moved, fades)
}

#[inline(never)]
fn shift_held_fades(
    shift: &FadeShift,
    moved: &mut [ShiftedOperation],
    fades: &mut WindowFades,
) -> bool {
    for operation in moved {
        let slot = match operation {
            ShiftedOperation::Shadow(shadow) => &mut shadow.pad,
            ShiftedOperation::Quad(quad) => &mut quad.background.pad,
            ShiftedOperation::Underline(underline) => &mut underline.pad,
            ShiftedOperation::MonochromeSprite(sprite) => &mut sprite.pad,
            ShiftedOperation::SubpixelSprite(sprite) => &mut sprite.pad,
            ShiftedOperation::PolychromeSprite(sprite) => &mut sprite.pad,
            ShiftedOperation::StartLayer(_) | ShiftedOperation::EndLayer => continue,
        };
        match shift.map(*slot, fades) {
            Some(fade) => *slot = fade,
            None => return false,
        }
    }
    true
}

#[cfg(any(test, feature = "test-support"))]
/// The fade `id` stands for in the finished `scene`, as text two scenes
/// can be compared by: empty for none.
pub(crate) fn describe(scene: &Scene, id: u32) -> String {
    if id == NONE {
        return String::new();
    }
    match scene.edge_fades().get((id & 0xffff) as usize) {
        Some(ramps) => format!(" fade {ramps:?}"),
        None => format!(" fade {id} missing"),
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Scene {
    /// The ramps of the fade `id` stands for in this finished scene.
    pub fn edge_fade_ramps(&self, id: u32) -> Option<EdgeFadeRamps> {
        (id != NONE)
            .then(|| self.edge_fades().get((id & 0xffff) as usize).copied())
            .flatten()
    }

    /// Adds `ramps` to this scene's table of fades, as a window's frame
    /// gives it its window's, returning the index a primitive is drawn in
    /// them with: for renderer tests, which build scenes without a window.
    pub fn add_edge_fade(&mut self, ramps: EdgeFadeRamps) -> u32 {
        let table = &mut self.fast_painted.fades.table;
        if table.is_empty() {
            table.push(EdgeFadeRamps::NONE);
        }
        table.push(ramps);
        table[0].edge[0] = f32::from_bits(table.len() as u32);
        (table.len() - 1) as u32
    }

    /// Inserts `primitive` drawn in the fade `fade`, an index
    /// [`Scene::add_edge_fade`] returned or any other.
    pub fn insert_faded_primitive(&mut self, primitive: impl Into<Primitive>, fade: u32) {
        enter(self, fade);
        self.insert_primitive(primitive);
        leave(self);
    }
}

#[cfg(test)]
mod tests {
    use super::{EdgeFade, EdgeFadeRamps, NONE, WindowFades, smoothstep, unsmoothstep};
    use crate::{Bounds, Edges, Point, ScaledPixels, point, px, size};

    fn at(x: f32, y: f32) -> Point<ScaledPixels> {
        point(ScaledPixels(x), ScaledPixels(y))
    }

    fn region() -> Bounds<ScaledPixels> {
        Bounds {
            origin: at(100., 200.),
            size: size(ScaledPixels(400.), ScaledPixels(300.)),
        }
    }

    fn widths(width: f32) -> Edges<ScaledPixels> {
        Edges::all(ScaledPixels(width))
    }

    #[test]
    fn unsmoothstep_inverts_smoothstep() {
        for step in 0..=20 {
            let value = step as f32 / 20.;
            let back = smoothstep(unsmoothstep(value));
            assert!((back - value).abs() < 1e-5, "{value} came back {back}");
        }
    }

    /// A full fade is nothing at the edge, eases in over its width and
    /// leaves the content whole from there in.
    #[test]
    fn a_ramp_runs_from_nothing_at_the_edge_to_whole_a_width_in() {
        let ramps = EdgeFadeRamps::new(
            region(),
            Edges {
                left: ScaledPixels(20.),
                ..widths(0.)
            },
            Edges::all(1.),
        );
        assert!(ramps.alpha_at(at(100., 300.)) < 1e-6);
        assert!((ramps.alpha_at(at(110., 300.)) - 0.5).abs() < 1e-3);
        assert_eq!(ramps.alpha_at(at(120., 300.)), 1.);
        assert_eq!(ramps.alpha_at(at(499., 201.)), 1.);
        assert_eq!(ramps.alpha_at(at(90., 300.)), 0.);
        assert_eq!(EdgeFadeRamps::NONE.alpha_at(at(-1e6, 1e6)), 1.);
        assert!(!EdgeFadeRamps::NONE.fades());
        assert!(ramps.fades());
    }

    /// Every edge fades toward itself, the far ones too.
    #[test]
    fn each_edge_fades_toward_itself() {
        let ramps = EdgeFadeRamps::new(region(), widths(20.), Edges::all(1.));
        assert_eq!(ramps.alpha_at(at(300., 350.)), 1.);
        for (edge, inside) in [
            (at(100., 350.), at(110., 350.)),
            (at(300., 200.), at(300., 210.)),
            (at(500., 350.), at(490., 350.)),
            (at(300., 500.), at(300., 490.)),
        ] {
            assert!(ramps.alpha_at(edge) < 1e-5, "{edge:?}");
            assert!((ramps.alpha_at(inside) - 0.5).abs() < 1e-3, "{inside:?}");
        }
    }

    /// A shallow fade leaves `1 - depth` at the edge; no depth, no fade.
    #[test]
    fn depth_is_how_faint_the_edge_gets() {
        let ramps = EdgeFadeRamps::new(
            region(),
            Edges {
                right: ScaledPixels(24.),
                ..widths(0.)
            },
            Edges::all(0.25),
        );
        assert!((ramps.alpha_at(at(500., 300.)) - 0.75).abs() < 1e-4);
        assert_eq!(ramps.alpha_at(at(476., 300.)), 1.);
        assert!(!EdgeFadeRamps::new(region(), widths(24.), Edges::all(0.)).fades());
    }

    /// An edge fades only as far as content lies hidden past it.
    #[test]
    fn a_hidden_fade_deepens_with_what_lies_past_the_edge() {
        let fade = EdgeFade::y(px(20.));
        let hidden = |top: f32, bottom: f32| {
            fade.hidden(Edges {
                top: px(top),
                bottom: px(bottom),
                ..Edges::default()
            })
        };
        assert_eq!(hidden(0., 0.).depth.top, 0.);
        assert_eq!(hidden(0., 0.).depth.bottom, 0.);
        assert_eq!(hidden(10., 40.).depth.top, 0.5);
        assert_eq!(hidden(10., 40.).depth.bottom, 1.);
        assert_eq!(hidden(500., 0.).depth.left, 0., "an edge with no width");
    }

    /// Fades nest edge by edge; of two on one edge, the one reaching
    /// further in is drawn.
    #[test]
    fn nested_fades_meet_edge_by_edge() {
        let list = EdgeFadeRamps::new(
            region(),
            Edges {
                top: ScaledPixels(24.),
                bottom: ScaledPixels(24.),
                ..widths(0.)
            },
            Edges::all(1.),
        );
        let title = EdgeFadeRamps::new(
            Bounds {
                origin: at(120., 210.),
                size: size(ScaledPixels(200.), ScaledPixels(20.)),
            },
            Edges {
                right: ScaledPixels(16.),
                top: ScaledPixels(4.),
                ..widths(0.)
            },
            Edges::all(1.),
        );
        let both = list.within(&title);
        assert!(both.alpha_at(at(320., 300.)) < 1e-5);
        assert!(both.alpha_at(at(200., 200.)) < 1e-5);
        assert!(both.alpha_at(at(200., 500.)) < 1e-5);
        // The list's top reaches 224, the title's 214: the list's is drawn.
        assert_eq!(both.alpha_at(at(200., 212.)), list.alpha_at(at(200., 212.)));
        assert_eq!(list.within(&EdgeFadeRamps::NONE), list);
        assert_eq!(EdgeFadeRamps::NONE.within(&list), list);
    }

    /// Moved, a primitive keeps its own ramps with it and takes the region
    /// around it as it is now, which is what painting it afresh there
    /// gives.
    #[test]
    fn moving_keeps_inner_ramps_with_the_content_and_outer_ones_in_place() {
        let top = |depth: f32| {
            EdgeFadeRamps::new(
                region(),
                Edges {
                    top: ScaledPixels(24.),
                    ..widths(0.)
                },
                Edges::all(depth),
            )
        };
        let title = |x: f32, y: f32| {
            EdgeFadeRamps::new(
                Bounds {
                    origin: at(x, y),
                    size: size(ScaledPixels(400.), ScaledPixels(20.)),
                },
                Edges {
                    right: ScaledPixels(16.),
                    ..widths(0.)
                },
                Edges::all(0.6),
            )
        };
        let painted = top(1.).within(&title(100., 300.));
        let moved = painted.moved(at(-30., -40.), &top(1.), &top(0.5));
        assert_eq!(moved, top(0.5).within(&title(70., 260.)));
        assert_eq!(painted.moved(at(-30., -40.), &painted, &top(0.5)), top(0.5));
    }

    /// The table gives a fade one index, reuses a swept slot under a new
    /// generation, and tells an index kept past its sweep.
    #[test]
    fn the_table_tells_a_swept_index_from_its_slot_s_next_fade() {
        let mut fades = WindowFades::default();
        let a = EdgeFadeRamps::new(region(), widths(8.), Edges::all(1.));
        let b = EdgeFadeRamps::new(region(), widths(16.), Edges::all(1.));
        assert_eq!(fades.intern(&EdgeFadeRamps::NONE), NONE);
        let first = fades.intern(&a);
        assert_eq!(fades.intern(&a), first);
        assert_eq!(fades.ramps(first), Some(&a));
        fades.sweep(&[true, false]);
        assert_eq!(fades.ramps(first), None, "swept");
        let second = fades.intern(&b);
        assert_eq!(second & 0xffff, first & 0xffff, "the slot is reused");
        assert_ne!(second, first);
        assert_eq!(fades.ramps(second), Some(&b));
        assert_eq!(fades.ramps(first), None);
        let third = fades.intern(&a);
        fades.sweep(&[true, true, true]);
        assert_eq!(fades.ramps(third), Some(&a), "kept while referred to");
    }
}
