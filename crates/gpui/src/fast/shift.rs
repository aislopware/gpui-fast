//! Drawing a stretch of the last frame's scene again, moved by a whole number
//! of device pixels.
//!
//! An element drawn again from last frame where it was copies what it
//! painted (see [`crate::fast::scene::replay`]). One that moved, with nothing
//! else about it changed — a row under a scroll, a tile in a strip sliding
//! sideways — painted what it painted then, moved, as long as painting it
//! afresh would put every primitive where moving it puts it:
//!
//! - It moved by a whole number of device pixels, so that everything snapped
//!   to device pixels or quantized to subpixel steps along the way lands on
//!   the same steps, moved. Coordinates that round half toward zero round the
//!   same way only on the same side of zero, so along the axes it moves,
//!   every primitive, every element it holds and every glyph it placed lies
//!   at or past the window's top and left edges, before and after (see
//!   [`crate::fast::element`], which checks the elements and the glyphs).
//! - What it is clipped by is known before and after. Either the content
//!   mask it was drawn in moved with it ([`Masks::Moved`]), and every mask
//!   inside it moved too; or nothing it painted was left out for lying
//!   outside the mask but for what a mask inside it left out, and every
//!   mask inside lies clear of its edges where it clips ([`Masks::Replaced`]): each primitive was clipped by the mask
//!   it was drawn in, which the new mask replaces, or by one lying inside
//!   that mask — a clip nested in it, the side of a border split around its
//!   interior — which moves and is clipped by the new mask. A mask or paint
//!   layer that reaches the edge of the mask around it can't tell a clip
//!   from the edge, and the stretch is painted afresh.
//!
//! A glyph is placed where the text lays it out from where the element is,
//! in floating point, then rounded to a device pixel and a subpixel step.
//! Moved, it keeps the step it was drawn at; laid out afresh at the new
//! place, a glyph within a float rounding of a step's edge could land on the
//! step beside it, a quarter of a device pixel apart, which the eye can't
//! tell and which moving keeps steady.
//! - Every place it moves is a multiple of 1/64 of a device pixel: snapped
//!   bounds, glyphs, icons centered in them. Painting afresh works a place
//!   out from where the element is, moving works it out from where it was,
//!   and the two agree to the bit only when neither rounds: the side of a
//!   border drawn around a rounded corner of a fractional radius can come
//!   out a rounding apart. Such a stretch is painted afresh.
//! - It holds nothing else placed in the window: paths, surfaces, natives
//!   or transformed sprites are not moved, and the stretch is painted afresh.
//!
//! Moved primitives are ordered as inserting them afresh orders them.

use crate::fast::scene::PaintedRef;
use crate::{
    Bounds, ContentMask, MonochromeSprite, PaintOperation, Pixels, Point, PolychromeSprite, Quad,
    ScaledPixels, Scene, Shadow, SubpixelSprite, TransformationMatrix, Underline,
};
use std::ops::Range;

/// How a stretch of the last frame's scene is drawn again, moved.
#[derive(Clone, Copy)]
pub(crate) struct Shift {
    /// How far, in device pixels, a whole number of them.
    pub(crate) offset: Point<ScaledPixels>,
    /// How far, in logical pixels.
    pub(crate) by: Point<Pixels>,
    pub(crate) masks: Masks,
}

/// What the primitives of a moved stretch are clipped by.
#[derive(Clone, Copy)]
pub(crate) enum Masks {
    /// The mask it was drawn in moved with it, and every mask with it.
    Moved,
    /// The mask it was drawn in, snapped out to device pixels, was `old`
    /// and is `new`; every mask inside it is `old`, the mask around, or is
    /// [`known_inside`] it.
    Replaced {
        old: Bounds<ScaledPixels>,
        new: Bounds<ScaledPixels>,
    },
}

/// What painting notes besides what it paints, for an element drawn again
/// moved to tell whether painting it afresh would paint what it painted,
/// moved, kept small in every record: in whole device pixels, which is all
/// the rules that read them need.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Noted {
    /// The least x and the least y that glyphs were placed at before they
    /// were rounded to a pixel and a subpixel step, rounded down; `i16::MAX`,
    /// or past it, with no glyph.
    glyphs: [i16; 2],
    /// Left, top, right and bottom of the content masks, snapped out to
    /// device pixels, that primitives and paint layers were left out for
    /// lying outside of, in one bounds: empty, left past right, when none
    /// was, and as wide as it goes when one lay out of reach.
    culled: [i16; 4],
    /// The sides of those masks what was left out lay beyond (see
    /// [`beyond`]).
    sides: u8,
    /// How far beyond them, the least of it, rounded down: `i16::MAX`, or
    /// past it, when nothing was left out.
    gap: i16,
}

const LEFT: u8 = 1;
const TOP: u8 = 2;
const RIGHT: u8 = 4;
const BOTTOM: u8 = 8;

/// The side of `mask`, left, top, right and bottom, that `bounds`, left out
/// for not meeting it, lies beyond, and how far beyond: all of them, no way
/// beyond, for bounds lying beyond none, which only empty bounds do, and
/// stay left out anywhere.
pub(crate) fn beyond(bounds: [f32; 4], mask: [f32; 4]) -> (u8, f32) {
    let [left, top, right, bottom] = bounds;
    if right <= mask[0] {
        (LEFT, mask[0] - right)
    } else if bottom <= mask[1] {
        (TOP, mask[1] - bottom)
    } else if left >= mask[2] {
        (RIGHT, left - mask[2])
    } else if top >= mask[3] {
        (BOTTOM, top - mask[3])
    } else if left < right && top < bottom {
        (LEFT | TOP | RIGHT | BOTTOM, 0.)
    } else {
        (0, f32::INFINITY)
    }
}

/// Whether what lay beyond `sides` of a mask that was `old` and is `new`,
/// `gap` beyond them at least, still does once moved by `offset`: whatever
/// mask nested in it moves with it, and it moves toward that side of the
/// mask around no further than the gap, and that side's own move, allow.
pub(crate) fn still_beyond<T>(
    sides: u8,
    gap: T,
    offset: Point<T>,
    old: &Bounds<T>,
    new: &Bounds<T>,
) -> bool
where
    T: Copy
        + PartialOrd
        + std::ops::Add<Output = T>
        + std::ops::Sub<Output = T>
        + Default
        + std::fmt::Debug,
{
    let Point { x, y } = offset;
    (sides & LEFT == 0 || x <= new.left() - old.left() + gap)
        && (sides & TOP == 0 || y <= new.top() - old.top() + gap)
        && (sides & RIGHT == 0 || x >= new.right() - old.right() - gap)
        && (sides & BOTTOM == 0 || y >= new.bottom() - old.bottom() - gap)
}

const NO_CULLS: [i16; 4] = [i16::MAX, i16::MAX, i16::MIN, i16::MIN];
const ANY_CULLS: [i16; 4] = [i16::MIN, i16::MIN, i16::MAX, i16::MAX];

impl Noted {
    pub(crate) const NOTHING: Noted = Noted {
        glyphs: [i16::MAX; 2],
        culled: NO_CULLS,
        sides: 0,
        gap: i16::MAX,
    };

    /// What an element painted without noting noted: glyphs as far out of
    /// reach as they go, which no move can tell to stay clear of zero, so
    /// that it is never drawn again moved, and anything left out anywhere,
    /// right at every side of the mask, so that it is not drawn again in
    /// place under a mask that grew either.
    pub(crate) const UNKNOWN: Noted = Noted {
        glyphs: [i16::MIN; 2],
        culled: ANY_CULLS,
        sides: LEFT | TOP | RIGHT | BOTTOM,
        gap: 0,
    };

    /// What painting what this was noted of, moved by `offset`, a whole
    /// number of device pixels, notes.
    pub(crate) fn moved(self, shift: &Shift) -> Noted {
        let offset = shift.offset;
        let by = [offset.x.0 as i32, offset.y.0 as i32];
        // A glyph past the reach on the negative side is not known to come
        // back into it.
        let glyph = |at: i16, by: i32| {
            if at == i16::MIN {
                at
            } else {
                (i32::from(at) + by).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
            }
        };
        // Under a mask that stood still, what the mask around left out is
        // not known to lie inside it once moved; how far beyond its sides
        // it lies moves with it and with them, a device pixel less for
        // logical masks snapped out.
        let (keep_culled, gap) = match &shift.masks {
            Masks::Moved => (true, self.gap),
            Masks::Replaced { old, new } => {
                let side = |flag: u8, toward: f32| {
                    if self.sides & flag == 0 {
                        f32::INFINITY
                    } else {
                        toward
                    }
                };
                let least = side(LEFT, new.left().0 - old.left().0 - offset.x.0)
                    .min(side(TOP, new.top().0 - old.top().0 - offset.y.0))
                    .min(side(RIGHT, offset.x.0 - (new.right().0 - old.right().0)))
                    .min(side(BOTTOM, offset.y.0 - (new.bottom().0 - old.bottom().0)));
                let snapped = if old == new { 0. } else { 1. };
                let gap = if least.is_finite() {
                    (f32::from(self.gap) + least - snapped).floor() as i16
                } else {
                    self.gap
                };
                let inside = self
                    .culled_in()
                    .is_some_and(|culled| known_inside(&culled, offset, old, new));
                (inside, gap)
            }
        };
        let culled = if self.culled == NO_CULLS || self.culled == ANY_CULLS {
            self.culled
        } else if !keep_culled {
            ANY_CULLS
        } else {
            let moved = [0, 1, 2, 3].map(|edge| i32::from(self.culled[edge]) + by[edge % 2]);
            let reach = i32::from(i16::MIN + 1)..=i32::from(i16::MAX - 1);
            if moved.iter().all(|edge| reach.contains(edge)) {
                moved.map(|edge| edge as i16)
            } else {
                ANY_CULLS
            }
        };
        Noted {
            glyphs: [glyph(self.glyphs[0], by[0]), glyph(self.glyphs[1], by[1])],
            culled,
            sides: self.sides,
            gap,
        }
    }

    /// Whether every glyph lies at or past zero along the axes `offset`
    /// moves on, before and after (see [`clear_of_zero`]).
    pub(crate) fn glyphs_clear_of_zero(&self, offset: Point<ScaledPixels>) -> bool {
        let clear = |at: i16, by: f32| by == 0. || at >= 0 && i32::from(at) + by as i32 >= 0;
        clear(self.glyphs[0], offset.x.0) && clear(self.glyphs[1], offset.y.0)
    }

    /// The sides of the masks what was left out lay beyond, and how far
    /// beyond them at least, in device pixels.
    pub(crate) fn culled_sides(&self) -> (u8, f32) {
        (self.sides, f32::from(self.gap))
    }

    /// The masks what was left out lay outside of, in one bounds, if any.
    pub(crate) fn culled_in(&self) -> Option<Bounds<ScaledPixels>> {
        let [left, top, right, bottom] = self.culled.map(|edge| ScaledPixels(f32::from(edge)));
        (self.culled != NO_CULLS)
            .then(|| Bounds::from_corners(Point::new(left, top), Point::new(right, bottom)))
    }
}

/// What painting is noting, as [`Noted`] has it but kept as it comes, for
/// the element being painted and those around it.
///
/// Every element with a record notes on its own, so beginning and ending
/// one is kept cheap: the glyphs are a least place saved and restored around
/// it, and what was left out, which most elements leave nothing out of, is
/// kept in an entry of its own only for an element that does, made when it
/// first does and folded into the entry of the element around it when it
/// ends.
///
/// Noting only matters for an element that may be drawn again moved, so it
/// is done only while an element in motion is painted: outside those,
/// painting notes nothing, and an element painted so is noted as
/// [`Noted::UNKNOWN`], as is any element holding one.
#[derive(Debug)]
pub(crate) struct Noting {
    active: bool,
    glyphs: Point<f32>,
    /// An entry for each element being painted that left something out,
    /// innermost last.
    culls: Vec<Cull>,
    /// Where the entry of the element being painted is, or would be.
    floor: u32,
}

/// What an element left out for lying outside masks, as [`Noting`] keeps
/// it.
#[derive(Clone, Copy, Debug)]
struct Cull {
    /// Left, top, right and bottom of the masks, snapped out to device
    /// pixels, in one bounds.
    edges: [f32; 4],
    sides: u8,
    gap: f32,
}

impl Cull {
    #[inline]
    fn add(&mut self, edges: [f32; 4], sides: u8, gap: f32) {
        self.edges[0] = self.edges[0].min(edges[0]);
        self.edges[1] = self.edges[1].min(edges[1]);
        self.edges[2] = self.edges[2].max(edges[2]);
        self.edges[3] = self.edges[3].max(edges[3]);
        self.sides |= sides;
        self.gap = self.gap.min(gap);
    }
}

/// What the elements around one being painted noted before it began, until
/// it ends.
#[derive(Clone, Copy)]
pub(crate) struct Around {
    glyphs: Point<f32>,
    floor: u32,
    /// Whether the elements around it were noting.
    active: bool,
    /// Whether it is.
    noting: bool,
}

const NO_GLYPHS: Point<f32> = Point {
    x: f32::INFINITY,
    y: f32::INFINITY,
};

impl Noting {
    /// Whether an element being painted is noting.
    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn clear(&mut self) {
        self.active = false;
        self.glyphs = NO_GLYPHS;
        self.culls.clear();
        self.floor = 0;
    }

    /// Notes a glyph placed at `origin`, in device pixels.
    #[inline]
    pub(crate) fn glyph_at(&mut self, origin: Point<ScaledPixels>) {
        if self.active {
            self.glyphs.x = self.glyphs.x.min(origin.x.0);
            self.glyphs.y = self.glyphs.y.min(origin.y.0);
        }
    }

    /// Notes something left out for lying outside `mask`, snapped out to
    /// device pixels, beyond `sides` of it by `gap` device pixels.
    #[inline]
    pub(crate) fn culled(&mut self, mask: &Bounds<ScaledPixels>, (sides, gap): (u8, f32)) {
        if !self.active {
            return;
        }
        let edges = [mask.left().0, mask.top().0, mask.right().0, mask.bottom().0];
        self.cull(edges, sides, gap);
    }

    #[inline]
    fn cull(&mut self, edges: [f32; 4], sides: u8, gap: f32) {
        if self.culls.len() as u32 > self.floor
            && let Some(own) = self.culls.last_mut()
        {
            own.add(edges, sides, gap);
        } else {
            self.culls.push(Cull { edges, sides, gap });
        }
    }

    /// Notes what a record noted.
    #[inline]
    pub(crate) fn add_noted(&mut self, noted: Noted) {
        if !self.active {
            return;
        }
        self.glyphs.x = self.glyphs.x.min(f32::from(noted.glyphs[0]));
        self.glyphs.y = self.glyphs.y.min(f32::from(noted.glyphs[1]));
        if noted.culled != NO_CULLS {
            self.cull(
                noted.culled.map(f32::from),
                noted.sides,
                f32::from(noted.gap),
            );
        }
    }

    /// Begins noting for an element painted from here, if `noting`,
    /// returning what the elements around it noted so far, for
    /// [`Noting::end`].
    #[inline]
    pub(crate) fn begin(&mut self, noting: bool) -> Around {
        let around = Around {
            glyphs: self.glyphs,
            floor: self.floor,
            active: self.active,
            noting,
        };
        self.active = noting;
        if noting {
            self.glyphs = NO_GLYPHS;
            self.floor = self.culls.len() as u32;
        }
        around
    }

    /// Ends noting for the element [`Noting::begin`] began it for, returning
    /// what it noted, which the elements around it noted too.
    #[inline]
    pub(crate) fn end(&mut self, around: Around) -> Noted {
        self.active = around.active;
        if !around.noting {
            if around.active {
                self.glyphs = Point {
                    x: f32::NEG_INFINITY,
                    y: f32::NEG_INFINITY,
                };
            }
            return Noted::UNKNOWN;
        }
        let glyphs = self.glyphs;
        self.glyphs.x = glyphs.x.min(around.glyphs.x);
        self.glyphs.y = glyphs.y.min(around.glyphs.y);
        let own = self.culls.len() as u32 > self.floor;
        self.floor = around.floor;
        let (culled, sides, gap) = if own {
            self.fold()
        } else {
            (NO_CULLS, 0, i16::MAX)
        };
        Noted {
            // Saturating: a glyph past the reach is kept at its edge.
            glyphs: [glyphs.x.floor() as i16, glyphs.y.floor() as i16],
            culled,
            sides,
            gap,
        }
    }

    /// Takes the entry of the element ending, folding it into that of the
    /// element around it, and returns it as [`Noted`] keeps it.
    fn fold(&mut self) -> ([i16; 4], u8, i16) {
        let Some(own) = self.culls.pop() else {
            return (NO_CULLS, 0, i16::MAX);
        };
        if self.active {
            self.cull(own.edges, own.sides, own.gap);
        }
        // Saturating, as the glyphs.
        (culled_edges(own.edges), own.sides, own.gap.floor() as i16)
    }
}

impl Default for Noting {
    fn default() -> Self {
        Noting {
            active: false,
            glyphs: NO_GLYPHS,
            culls: Vec::new(),
            floor: 0,
        }
    }
}

/// The edges of the masks something was left out of, as [`Noted`] keeps
/// them: rounded out to whole device pixels, or as wide as they go when one
/// lay out of reach.
fn culled_edges([left, top, right, bottom]: [f32; 4]) -> [i16; 4] {
    let edges = [left.floor(), top.floor(), right.ceil(), bottom.ceil()];
    let reach = f32::from(i16::MIN + 1)..=f32::from(i16::MAX - 1);
    if edges.iter().all(|edge| reach.contains(edge)) {
        edges.map(|edge| edge as i16)
    } else {
        ANY_CULLS
    }
}

/// Whether `inner`, lying inside the mask `old`, is known from it: each of
/// its edges clear of the mask's, or on an edge of the mask that stayed
/// where it was, as `new`, along an axis nothing moved on (see [`reclip`]).
pub(crate) fn known_inside(
    inner: &Bounds<ScaledPixels>,
    offset: Point<ScaledPixels>,
    old: &Bounds<ScaledPixels>,
    new: &Bounds<ScaledPixels>,
) -> bool {
    let still = |inner: ScaledPixels, offset: ScaledPixels, old: ScaledPixels, new| {
        inner == old && offset.0 == 0. && old == new
    };
    (inner.left() > old.left() || still(inner.left(), offset.x, old.left(), new.left()))
        && (inner.top() > old.top() || still(inner.top(), offset.y, old.top(), new.top()))
        && (inner.right() < old.right() || still(inner.right(), offset.x, old.right(), new.right()))
        && (inner.bottom() < old.bottom()
            || still(inner.bottom(), offset.y, old.bottom(), new.bottom()))
}

/// Whether `value` is a place moving adds to without rounding: a multiple
/// of 1/64 of a device pixel, well within the range `f32` holds those in.
fn exact(value: ScaledPixels) -> bool {
    (value.0 * 64.).fract() == 0. && value.0.abs() < 65536.
}

/// Whether moving `bounds` moves each of its edges without rounding.
fn movable(bounds: &Bounds<ScaledPixels>) -> bool {
    exact(bounds.left()) && exact(bounds.top()) && exact(bounds.right()) && exact(bounds.bottom())
}

/// Where `bounds`, clipped by the mask `old` and lying inside it, lies once
/// moved by `offset` and clipped by the mask `new` instead, if it can be
/// told: an edge short of the mask's own is the thing's, which moves and is
/// clipped by the new mask, but one on the mask's edge may be the mask's or
/// the thing's, and is known only where the thing did not move across it
/// and the mask's edge stayed where it was.
fn reclip(
    bounds: &Bounds<ScaledPixels>,
    offset: Point<ScaledPixels>,
    old: &Bounds<ScaledPixels>,
    new: &Bounds<ScaledPixels>,
) -> Option<Bounds<ScaledPixels>> {
    let edge = |edge: ScaledPixels,
                offset: ScaledPixels,
                old: ScaledPixels,
                new: ScaledPixels,
                clip: fn(ScaledPixels, ScaledPixels) -> ScaledPixels| {
        if edge != old {
            Some(clip(edge + offset, new))
        } else if offset.0 == 0. && old == new {
            Some(new)
        } else {
            None
        }
    };
    let left = edge(
        bounds.left(),
        offset.x,
        old.left(),
        new.left(),
        ScaledPixels::max,
    )?;
    let top = edge(
        bounds.top(),
        offset.y,
        old.top(),
        new.top(),
        ScaledPixels::max,
    )?;
    let right = edge(
        bounds.right(),
        offset.x,
        old.right(),
        new.right(),
        ScaledPixels::min,
    )?;
    let bottom = edge(
        bounds.bottom(),
        offset.y,
        old.bottom(),
        new.bottom(),
        ScaledPixels::min,
    )?;
    let moved = Bounds::from_corners(Point::new(left, top), Point::new(right, bottom));
    (!moved.is_empty()).then_some(moved)
}

/// Whether a coordinate at `value`, moved by `offset`, rounds as it did
/// moved: rounding half toward zero and truncating round alike on one side
/// of zero only, so a coordinate that moves stays at or past zero, where it
/// is the same side of zero as what it was rounded from. The sign of a
/// coordinate rounded to zero from below is negative.
pub(crate) fn clear_of_zero(value: f32, offset: f32) -> bool {
    offset == 0. || value >= 0. && value.is_sign_positive() && value + offset >= 0.
}

/// A paint operation of the last frame's scene, moved, which an element
/// drawn again moved paints: worked out once, while its prepaint decides it
/// can be moved, and inserted as it is while it paints.
#[derive(Clone, Copy)]
pub(crate) enum ShiftedOperation {
    Shadow(Shadow),
    Quad(Quad),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    SubpixelSprite(SubpixelSprite),
    PolychromeSprite(PolychromeSprite),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

/// The mask the last primitive moved was clipped by, and where it lies
/// moved, since most primitives in a row share their mask.
struct LastMask {
    from: Bounds<ScaledPixels>,
    to: Option<ContentMask<ScaledPixels>>,
}

impl Shift {
    /// Where `mask`, a primitive's, lies once moved, if it can be told.
    fn mask(&self, mask: &ContentMask<ScaledPixels>) -> Option<ContentMask<ScaledPixels>> {
        if !movable(&mask.bounds) {
            return None;
        }
        let bounds = match &self.masks {
            Masks::Moved => mask.bounds + self.offset,
            // No mask inside is `old` (see `Masks::Replaced`).
            Masks::Replaced { old, new } if mask.bounds == *old => *new,
            Masks::Replaced { old, new } => reclip(&mask.bounds, self.offset, old, new)?,
        };
        Some(ContentMask { bounds })
    }

    /// Where a paint layer's `bounds`, clipped by the mask it was pushed in,
    /// lies once moved, if it can be told and is not left out.
    fn layer(&self, bounds: &Bounds<ScaledPixels>) -> Option<Bounds<ScaledPixels>> {
        if !movable(bounds) {
            return None;
        }
        match &self.masks {
            Masks::Moved => Some(*bounds + self.offset),
            Masks::Replaced { old, new } => reclip(bounds, self.offset, old, new),
        }
    }

    /// Where a primitive at `bounds` clipped by `mask` lies once moved, and
    /// the mask it is clipped by then, if it can be told and it is not left
    /// out there.
    #[inline]
    fn place(
        &self,
        bounds: &Bounds<ScaledPixels>,
        mask: &ContentMask<ScaledPixels>,
        last: &mut LastMask,
    ) -> Option<(Bounds<ScaledPixels>, ContentMask<ScaledPixels>)> {
        let offset = self.offset;
        if !clear_of_zero(bounds.origin.x.0, offset.x.0)
            || !clear_of_zero(bounds.origin.y.0, offset.y.0)
            || !movable(bounds)
        {
            return None;
        }
        if last.from != mask.bounds {
            *last = LastMask {
                from: mask.bounds,
                to: self.mask(mask),
            };
        }
        let content_mask = last.to?;
        let moved = *bounds + offset;
        // Moved out of its mask, it would not be painted afresh.
        (!moved.intersect(&content_mask.bounds).is_empty()).then_some((moved, content_mask))
    }

    /// `primitive`, moved, if it can be.
    fn primitive(
        &self,
        primitive: PaintedRef<'_>,
        last: &mut LastMask,
    ) -> Option<ShiftedOperation> {
        let unit = TransformationMatrix::unit();
        Some(match primitive {
            PaintedRef::Shadow(shadow) if movable(&shadow.element_bounds) => {
                let (bounds, content_mask) =
                    self.place(&shadow.bounds, &shadow.content_mask, last)?;
                ShiftedOperation::Shadow(Shadow {
                    bounds,
                    element_bounds: shadow.element_bounds + self.offset,
                    content_mask,
                    ..*shadow
                })
            }
            PaintedRef::Quad(quad) => {
                let (bounds, content_mask) = self.place(&quad.bounds, &quad.content_mask, last)?;
                ShiftedOperation::Quad(Quad {
                    bounds,
                    content_mask,
                    ..*quad
                })
            }
            PaintedRef::Underline(underline) => {
                let (bounds, content_mask) =
                    self.place(&underline.bounds, &underline.content_mask, last)?;
                ShiftedOperation::Underline(Underline {
                    bounds,
                    content_mask,
                    ..*underline
                })
            }
            PaintedRef::MonochromeSprite(sprite) if sprite.transformation == unit => {
                let (bounds, content_mask) =
                    self.place(&sprite.bounds, &sprite.content_mask, last)?;
                ShiftedOperation::MonochromeSprite(MonochromeSprite {
                    bounds,
                    content_mask,
                    ..*sprite
                })
            }
            PaintedRef::SubpixelSprite(sprite) if sprite.transformation == unit => {
                let (bounds, content_mask) =
                    self.place(&sprite.bounds, &sprite.content_mask, last)?;
                ShiftedOperation::SubpixelSprite(SubpixelSprite {
                    bounds,
                    content_mask,
                    ..*sprite
                })
            }
            // A scroll layer's tile: the layer places it from its own record,
            // which only painting the container keeps current.
            PaintedRef::PolychromeSprite(sprite)
                if crate::fast::layers::scene::decode_layer_tile(
                    sprite.tile.texture_id,
                    sprite.tile.tile_id,
                )
                .is_some() =>
            {
                return None;
            }
            PaintedRef::PolychromeSprite(sprite) => {
                let (bounds, content_mask) =
                    self.place(&sprite.bounds, &sprite.content_mask, last)?;
                ShiftedOperation::PolychromeSprite(PolychromeSprite {
                    bounds,
                    content_mask,
                    ..*sprite
                })
            }
            PaintedRef::Shadow(_)
            | PaintedRef::MonochromeSprite(_)
            | PaintedRef::SubpixelSprite(_)
            | PaintedRef::Other => return None,
        })
    }
}

/// Moves the paint operations `range` of `previous`, the last frame's scene,
/// as `shift` says, onto `moved`, returning whether all of them could be;
/// when one can't, `moved` is left as it was.
pub(crate) fn shift_operations(
    previous: &Scene,
    range: Range<usize>,
    shift: &Shift,
    moved: &mut Vec<ShiftedOperation>,
) -> bool {
    let start = moved.len();
    let mut last = LastMask {
        // Equal to no mask.
        from: Bounds {
            origin: Point::new(ScaledPixels(f32::NAN), ScaledPixels(f32::NAN)),
            size: crate::Size::default(),
        },
        to: None,
    };
    for operation in &previous.paint_operations[range] {
        let shifted = match operation {
            PaintOperation::Primitive(at) => {
                shift.primitive(crate::fast::scene::painted(previous, *at), &mut last)
            }
            PaintOperation::StartLayer(bounds) => {
                shift.layer(bounds).map(ShiftedOperation::StartLayer)
            }
            PaintOperation::EndLayer => Some(ShiftedOperation::EndLayer),
            PaintOperation::Native(_) => None,
        };
        let Some(shifted) = shifted else {
            moved.truncate(start);
            return false;
        };
        moved.push(shifted);
    }
    true
}

/// Paints `moved`, what [`shift_operations`] moved.
pub(crate) fn replay_shifted(scene: &mut Scene, moved: &[ShiftedOperation]) {
    for operation in moved {
        match *operation {
            ShiftedOperation::Shadow(shadow) => scene.insert_primitive(shadow),
            ShiftedOperation::Quad(quad) => scene.insert_primitive(quad),
            ShiftedOperation::Underline(underline) => scene.insert_primitive(underline),
            ShiftedOperation::MonochromeSprite(sprite) => scene.insert_primitive(sprite),
            ShiftedOperation::SubpixelSprite(sprite) => scene.insert_primitive(sprite),
            ShiftedOperation::PolychromeSprite(sprite) => scene.insert_primitive(sprite),
            ShiftedOperation::StartLayer(bounds) => scene.push_layer(bounds),
            ShiftedOperation::EndLayer => scene.pop_layer(),
        }
    }
}
