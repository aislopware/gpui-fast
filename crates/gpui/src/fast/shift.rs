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

use crate::{
    Bounds, ContentMask, PaintOperation, Pixels, Point, Primitive, ScaledPixels, Scene,
    TransformationMatrix,
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
    T: Clone
        + PartialOrd
        + std::ops::Add<Output = T>
        + std::ops::Sub<Output = T>
        + Default
        + std::fmt::Debug,
{
    let Point { x, y } = offset;
    (sides & LEFT == 0 || x.clone() <= new.left() - old.left() + gap.clone())
        && (sides & TOP == 0 || y.clone() <= new.top() - old.top() + gap.clone())
        && (sides & RIGHT == 0 || x >= new.right() - old.right() - gap.clone())
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

/// What painting is noting, as [`Noted`] has it but kept as it comes, a
/// glyph and a primitive left out at a time, until a record takes it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Noting {
    glyphs: Point<f32>,
    /// Left, top, right and bottom, as in [`Noted`], empty when nothing was
    /// left out.
    culled: [f32; 4],
    /// The mask something was last left out for lying outside, which most
    /// of what is left out in a row lies outside too.
    last_culled: Bounds<ScaledPixels>,
    sides: u8,
    gap: f32,
}

const NOT_CULLED: [f32; 4] = [
    f32::INFINITY,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NEG_INFINITY,
];

impl Noting {
    pub(crate) const NOTHING: Noting = Noting {
        glyphs: Point {
            x: f32::INFINITY,
            y: f32::INFINITY,
        },
        culled: NOT_CULLED,
        // Equal to no mask.
        last_culled: Bounds {
            origin: Point {
                x: ScaledPixels(f32::NAN),
                y: ScaledPixels(f32::NAN),
            },
            size: crate::Size {
                width: ScaledPixels(0.),
                height: ScaledPixels(0.),
            },
        },
        sides: 0,
        gap: f32::INFINITY,
    };

    /// Notes a glyph placed at `origin`, in device pixels.
    #[inline]
    pub(crate) fn glyph_at(&mut self, origin: Point<ScaledPixels>) {
        self.glyphs.x = self.glyphs.x.min(origin.x.0);
        self.glyphs.y = self.glyphs.y.min(origin.y.0);
    }

    /// Notes something left out for lying outside `mask`, snapped out to
    /// device pixels, beyond `sides` of it by `gap` device pixels.
    #[inline]
    pub(crate) fn culled(&mut self, mask: &Bounds<ScaledPixels>, (sides, gap): (u8, f32)) {
        self.sides |= sides;
        self.gap = self.gap.min(gap);
        if *mask != self.last_culled {
            self.last_culled = *mask;
            self.cull([mask.left().0, mask.top().0, mask.right().0, mask.bottom().0]);
        }
    }

    fn cull(&mut self, [left, top, right, bottom]: [f32; 4]) {
        let culled = &mut self.culled;
        culled[0] = culled[0].min(left);
        culled[1] = culled[1].min(top);
        culled[2] = culled[2].max(right);
        culled[3] = culled[3].max(bottom);
    }

    /// Notes what a record noted.
    pub(crate) fn add_noted(&mut self, noted: Noted) {
        self.glyphs.x = self.glyphs.x.min(f32::from(noted.glyphs[0]));
        self.glyphs.y = self.glyphs.y.min(f32::from(noted.glyphs[1]));
        if noted.culled != NO_CULLS {
            self.cull(noted.culled.map(f32::from));
        }
        self.sides |= noted.sides;
        self.gap = self.gap.min(f32::from(noted.gap));
    }

    /// Notes what `other` noted.
    #[inline]
    pub(crate) fn add(&mut self, other: &Noting) {
        self.glyph_at(other.glyphs.map(ScaledPixels));
        if other.culled[0] <= other.culled[2] {
            self.cull(other.culled);
        }
        self.sides |= other.sides;
        self.gap = self.gap.min(other.gap);
    }

    /// What was noted, as a record keeps it.
    #[inline]
    pub(crate) fn noted(&self) -> Noted {
        Noted {
            // Saturating: a glyph past the reach is kept at its edge.
            glyphs: [self.glyphs.x.floor() as i16, self.glyphs.y.floor() as i16],
            culled: if self.culled[0] > self.culled[2] {
                NO_CULLS
            } else {
                self.culled_edges()
            },
            sides: self.sides,
            // Saturating, as the glyphs.
            gap: self.gap.floor() as i16,
        }
    }

    #[cold]
    fn culled_edges(&self) -> [i16; 4] {
        let [left, top, right, bottom] = self.culled;
        let edges = [left.floor(), top.floor(), right.ceil(), bottom.ceil()];
        let reach = f32::from(i16::MIN + 1)..=f32::from(i16::MAX - 1);
        if edges.iter().all(|edge| reach.contains(edge)) {
            edges.map(|edge| edge as i16)
        } else {
            ANY_CULLS
        }
    }
}

impl Default for Noting {
    fn default() -> Self {
        Noting::NOTHING
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

    /// `primitive`, moved, if it can be.
    fn primitive(&self, primitive: &Primitive) -> Option<Primitive> {
        let offset = self.offset;
        let bounds = primitive.bounds();
        if !clear_of_zero(bounds.origin.x.0, offset.x.0)
            || !clear_of_zero(bounds.origin.y.0, offset.y.0)
            || !movable(bounds)
        {
            return None;
        }
        let content_mask = self.mask(primitive.content_mask())?;
        let unit = TransformationMatrix::unit();
        let moved = match primitive {
            Primitive::Shadow(shadow) if movable(&shadow.element_bounds) => {
                Primitive::Shadow(crate::Shadow {
                    bounds: shadow.bounds + offset,
                    element_bounds: shadow.element_bounds + offset,
                    content_mask,
                    ..*shadow
                })
            }
            Primitive::Quad(quad) => Primitive::Quad(crate::Quad {
                bounds: quad.bounds + offset,
                content_mask,
                ..*quad
            }),
            Primitive::Underline(underline) => Primitive::Underline(crate::Underline {
                bounds: underline.bounds + offset,
                content_mask,
                ..*underline
            }),
            Primitive::MonochromeSprite(sprite) if sprite.transformation == unit => {
                Primitive::MonochromeSprite(crate::MonochromeSprite {
                    bounds: sprite.bounds + offset,
                    content_mask,
                    ..*sprite
                })
            }
            Primitive::SubpixelSprite(sprite) if sprite.transformation == unit => {
                Primitive::SubpixelSprite(crate::SubpixelSprite {
                    bounds: sprite.bounds + offset,
                    content_mask,
                    ..*sprite
                })
            }
            Primitive::PolychromeSprite(sprite) => {
                Primitive::PolychromeSprite(crate::PolychromeSprite {
                    bounds: sprite.bounds + offset,
                    content_mask,
                    ..*sprite
                })
            }
            Primitive::Shadow(_)
            | Primitive::MonochromeSprite(_)
            | Primitive::SubpixelSprite(_)
            | Primitive::Path(_)
            | Primitive::Surface(_) => return None,
        };
        // Moved out of its mask, it would not be painted afresh.
        let clipped = moved.bounds().intersect(&moved.content_mask().bounds);
        (!clipped.is_empty()).then_some(moved)
    }
}

/// Whether the paint operations `range` of `previous`, the last frame's
/// scene, can be drawn again moved as `shift` says.
pub(crate) fn can_shift(previous: &Scene, range: Range<usize>, shift: &Shift) -> bool {
    previous.paint_operations[range]
        .iter()
        .all(|operation| match operation {
            PaintOperation::Primitive(at) => shift
                .primitive(&crate::fast::scene::painted(previous, *at))
                .is_some(),
            PaintOperation::StartLayer(bounds) => shift.layer(bounds).is_some(),
            PaintOperation::EndLayer => true,
            PaintOperation::Native(_) => false,
        })
}

/// Draws the paint operations `range` of `previous`, the last frame's
/// scene, again, moved as `shift` says, which [`can_shift`] allowed.
pub(crate) fn replay_shifted(
    scene: &mut Scene,
    range: Range<usize>,
    previous: &Scene,
    shift: &Shift,
) {
    for operation in &previous.paint_operations[range] {
        match operation {
            PaintOperation::Primitive(at) => {
                if let Some(primitive) =
                    shift.primitive(&crate::fast::scene::painted(previous, *at))
                {
                    scene.insert_primitive(primitive);
                }
            }
            PaintOperation::StartLayer(bounds) => {
                if let Some(bounds) = shift.layer(bounds) {
                    scene.push_layer(bounds);
                }
            }
            PaintOperation::EndLayer => scene.pop_layer(),
            PaintOperation::Native(_) => {}
        }
    }
}
