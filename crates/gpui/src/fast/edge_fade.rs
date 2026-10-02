//! Edge fades: what is painted inside a region fading out toward its edges,
//! per pixel, where a clip would cut it or an ellipsis end it.
//!
//! A gradient painted over the content in the background's colour fades it
//! only over an opaque background. Over a window's glass, a video or another
//! tile there is no colour to paint, so the content itself fades: every
//! primitive carries the fade it was painted in ([`FadeRamps`]), and the
//! renderer multiplies what it draws, pixel by pixel, by one ramp per edge.
//!
//! A ramp is linear in the device pixel's coordinate along its edge's axis,
//! rising from its edge, clamped to 0..1 and eased with smoothstep, so it
//! leaves the content at full strength without a crease. Linear in the
//! coordinate, it is worked out at a primitive's corners and interpolated
//! exactly across it, which leaves a clamp and a few multiplies per pixel.
//! An edge that does not fade has a rate of zero and starts at one.
//!
//! Fades nest edge by edge: a list fading at its top and bottom holds rows
//! whose titles fade at the right, and a row's title fades at all three. Two
//! fades of the same edge can't be told as one ramp; the one reaching further
//! in wins, which is the one that matters wherever both reach.
//!
//! Retention draws primitives again from earlier frames, moved
//! ([`crate::fast::shift`]) or translated into a scroll layer's content
//! space and back. A fade belongs to the region that set it, so moving a
//! primitive moves the ramps that were set inside what moved and leaves the
//! ones from around it to the region they belong to ([`FadeRamps::moved`]).

use crate::{Bounds, Edges, Pixels, Point, ScaledPixels, Size, Style, Window};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

/// How what an element paints inside it fades out toward the element's
/// edges, in place of a hard clip or an ellipsis ([`crate::Styled::edge_fade`]).
///
/// The fade runs inside the element's border, where its overflow clips. It
/// fades the element's children, not its own background or border. Content
/// that runs past an edge without being clipped fades on to nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EdgeFade {
    /// How far in from the top edge the fade reaches; zero leaves it sharp.
    pub top: Pixels,
    /// How far in from the right edge the fade reaches.
    pub right: Pixels,
    /// How far in from the bottom edge the fade reaches.
    pub bottom: Pixels,
    /// How far in from the left edge the fade reaches.
    pub left: Pixels,
    /// Whether an edge fades only as far as content lies hidden past it:
    /// none while nothing is, deepening as more is, and in full once a
    /// whole width is. The edges of a scroll container fade as it scrolls
    /// away from them, and a title fades at its end only when it does not
    /// fit. Otherwise every edge with a width fades in full.
    pub only_hidden: bool,
}

impl EdgeFade {
    /// Every edge with a width fades in full, whatever lies past it.
    pub fn always(width: impl Into<Edges<Pixels>>) -> Self {
        Self::new(width.into(), false)
    }

    /// Each edge fades as far as content lies hidden past it, in full once
    /// `width` of it is (see [`EdgeFade::only_hidden`]).
    pub fn hidden(width: impl Into<Edges<Pixels>>) -> Self {
        Self::new(width.into(), true)
    }

    /// The left and right edges fade as far as content lies hidden past
    /// them: a title that fades out at its end where it does not fit, or a
    /// strip of tabs scrolled sideways.
    pub fn hidden_x(width: Pixels) -> Self {
        Self::hidden(Edges {
            left: width,
            right: width,
            ..Edges::default()
        })
    }

    /// The top and bottom edges fade as far as content lies hidden past
    /// them: a scrolling list or transcript.
    pub fn hidden_y(width: Pixels) -> Self {
        Self::hidden(Edges {
            top: width,
            bottom: width,
            ..Edges::default()
        })
    }

    /// How far in from each edge the fade reaches.
    pub fn width(&self) -> Edges<Pixels> {
        Edges {
            top: self.top,
            right: self.right,
            bottom: self.bottom,
            left: self.left,
        }
    }

    fn new(width: Edges<Pixels>, only_hidden: bool) -> Self {
        Self {
            top: width.top,
            right: width.right,
            bottom: width.bottom,
            left: width.left,
            only_hidden,
        }
    }
}

/// The ramps a primitive is drawn with, in device pixels, as the renderers
/// read them. For the left, top, right and bottom edges in that order, the
/// ramp at a pixel is `start + rate × (coordinate − edge)`, the coordinate
/// being `x` at the left and right and `y` at the top and bottom, clamped to
/// 0..1 and eased with smoothstep; what is drawn is multiplied by all four.
/// See the [module](self).
///
/// Moving what a fade belongs to moves only its edges, which lie on whole
/// device pixels, so a primitive drawn again moved fades exactly as one
/// painted afresh there. The rates and starts are packed two to a word, as
/// half floats and 16-bit fractions, which keeps a primitive's fade to 32
/// bytes and is finer than a pixel's 8 bits of alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct FadeRamps {
    /// Where each edge lies: a whole device pixel.
    pub edge: [f32; 4],
    /// How fast each ramp rises per device pixel, as two half floats a
    /// word (left and top, then right and bottom, the first in the low
    /// bits): positive at the left and top, negative at the right and
    /// bottom, zero where an edge does not fade.
    pub rate: [u32; 2],
    /// Each ramp at its edge, as two 16-bit fractions of 1 a word, in the
    /// order of `rate`.
    pub start: [u32; 2],
}

/// A ramp of [`FadeRamps`]: its edge, and its rate and start as packed.
#[derive(Clone, Copy, PartialEq)]
struct Ramp {
    edge: f32,
    rate: u16,
    start: u16,
}

impl Ramp {
    fn fades(&self) -> bool {
        f16_value(self.rate) != 0.
    }

    /// The ramp at `coordinate`, before it is clamped and eased.
    fn at(&self, coordinate: f32) -> f32 {
        f32::from(self.start) / f32::from(u16::MAX) + f16_value(self.rate) * (coordinate - self.edge)
    }

    /// How far the ramp reaches in from its side, along the inward
    /// direction: where it reaches 1, signed so that further in is larger.
    fn reach(&self) -> f32 {
        let rate = f16_value(self.rate);
        let start = f32::from(self.start) / f32::from(u16::MAX);
        (self.edge + (1. - start) / rate) * rate.signum()
    }
}

impl FadeRamps {
    /// No edge fades.
    pub const NONE: Self = Self {
        edge: [0.; 4],
        rate: [0; 2],
        start: [u32::MAX; 2],
    };

    /// Whether any edge fades.
    pub fn fades(&self) -> bool {
        (0..4).any(|slot| self.ramp(slot).fades())
    }

    fn ramp(&self, slot: usize) -> Ramp {
        let shift = 16 * (slot % 2);
        Ramp {
            edge: self.edge[slot],
            rate: (self.rate[slot / 2] >> shift) as u16,
            start: (self.start[slot / 2] >> shift) as u16,
        }
    }

    fn set_ramp(&mut self, slot: usize, ramp: Ramp) {
        let shift = 16 * (slot % 2);
        let keep = !(0xffff_u32 << shift);
        self.edge[slot] = ramp.edge;
        self.rate[slot / 2] = self.rate[slot / 2] & keep | u32::from(ramp.rate) << shift;
        self.start[slot / 2] = self.start[slot / 2] & keep | u32::from(ramp.start) << shift;
    }

    /// The ramps of a fade in `region`, whose edges lie on whole device
    /// pixels: each edge `width` wide, `depth` deep at the edge (0 not at
    /// all, 1 to nothing).
    pub(crate) fn new(
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
            if !(width > 0. && depth > 0.) {
                continue;
            }
            // At the edge the eased ramp is `1 - depth`; a width in, it is 1.
            let start = unsmoothstep(1. - depth.min(1.));
            let ramp = Ramp {
                edge,
                rate: f16_bits_away_from_zero(inward * (1. - start) / width),
                start: (start * f32::from(u16::MAX)).round() as u16,
            };
            if ramp.fades() {
                ramps.set_ramp(slot, ramp);
            }
        }
        ramps
    }

    /// These ramps with `inner`'s laid over them, edge by edge: where both
    /// fade an edge, the ramp that reaches further in.
    pub(crate) fn within(&self, inner: &Self) -> Self {
        let mut ramps = *self;
        for slot in 0..4 {
            let (outer, inner) = (self.ramp(slot), inner.ramp(slot));
            if inner.fades() && (!outer.fades() || inner.reach() > outer.reach()) {
                ramps.set_ramp(slot, inner);
            }
        }
        ramps
    }

    /// These ramps for what they fade moved by `delta`.
    pub(crate) fn translated(&self, delta: Point<ScaledPixels>) -> Self {
        let mut ramps = *self;
        for slot in 0..4 {
            if self.ramp(slot).fades() {
                ramps.edge[slot] += if slot % 2 == 0 { delta.x.0 } else { delta.y.0 };
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
            } else if ramp.fades() {
                ramps.edge[slot] += if slot % 2 == 0 { delta.x.0 } else { delta.y.0 };
            }
        }
        ramps
    }

    /// Whether nothing drawn inside `bounds` is faded at all.
    pub(crate) fn whole_over(&self, bounds: Bounds<ScaledPixels>) -> bool {
        let ends = [
            (bounds.left().0, bounds.right().0),
            (bounds.top().0, bounds.bottom().0),
        ];
        (0..4).all(|slot| {
            let ramp = self.ramp(slot);
            if !ramp.fades() {
                return true;
            }
            // A ramp is least at the end of `bounds` it rises away from.
            let (low, high) = ends[slot % 2];
            let at = if f16_value(ramp.rate) > 0. { low } else { high };
            ramp.at(at) >= 1.
        })
    }

    /// How strongly a pixel centred at `point` is drawn, as the renderers
    /// compute it.
    pub fn alpha_at(&self, point: Point<ScaledPixels>) -> f32 {
        (0..4)
            .map(|slot| {
                let coordinate = if slot % 2 == 0 { point.x.0 } else { point.y.0 };
                let ramp = self.ramp(slot).at(coordinate).clamp(0., 1.);
                ramp * ramp * (3. - 2. * ramp)
            })
            .product()
    }
}

impl Default for FadeRamps {
    fn default() -> Self {
        Self::NONE
    }
}

/// `value` as the bits of a half float, rounded to the nearest, ties to
/// even, and held to the largest finite half.
fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let magnitude = value.abs();
    if magnitude.is_nan() {
        return 0;
    }
    if magnitude >= 65504. {
        return sign | 0x7bff;
    }
    if magnitude < 6.103_515_6e-5 {
        // Below the least normal half: a multiple of 2^-24.
        return sign | (magnitude * 16_777_216.).round_ties_even() as u16;
    }
    let exponent = ((bits >> 23) & 0xff) + 15 - 127;
    let mantissa = bits & 0x7f_ffff;
    let mut half = (exponent << 10) | (mantissa >> 13);
    let rest = mantissa & 0x1fff;
    if rest > 0x1000 || rest == 0x1000 && half & 1 == 1 {
        // A carry out of the mantissa raises the exponent, as it should.
        half += 1;
    }
    sign | half as u16
}

/// The bits of the half float nearest `value` no nearer zero than it, so
/// that a ramp packed with it is whole by a width in.
fn f16_bits_away_from_zero(value: f32) -> u16 {
    let bits = f16_bits(value);
    if f16_value(bits).abs() < value.abs() && bits & 0x7fff < 0x7bff {
        bits + 1
    } else {
        bits
    }
}

/// The value of the half float whose bits are `bits`.
fn f16_value(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1. } else { -1. };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1. + mantissa / 1024.) * 2f32.powi(exponent - 15),
    }
}

/// Where smoothstep is `value`, for `value` in 0..1.
fn unsmoothstep(value: f32) -> f32 {
    0.5 - ((1. - 2. * value).asin() / 3.).sin()
}

/// The window's fade where painting is now: what every primitive painted now
/// is stamped with.
#[inline]
pub(crate) fn current(window: &Window) -> FadeRamps {
    window.fast_edge_fade
}

/// Runs `f` with no fade, for what paints into a scroll layer's content:
/// the container's fade belongs to its viewport and is laid over the
/// content where the layer is composited ([`compose`]).
pub(crate) fn without<R>(window: &mut Window, f: impl FnOnce(&mut Window) -> R) -> R {
    let outer = std::mem::replace(&mut window.fast_edge_fade, FadeRamps::NONE);
    let result = f(window);
    window.fast_edge_fade = outer;
    result
}

/// `primitive`, painted inside a layer's content and drawn now into the
/// frame where the layer stands, with the fade of where it stands laid
/// under its own.
pub(crate) fn compose(primitive: &mut crate::scene::Primitive, outer: &FadeRamps) {
    if !outer.fades() {
        return;
    }
    let fade = primitive_fade(primitive);
    *fade = outer.within(fade);
}

/// The fade `primitive` is drawn with.
pub(crate) fn primitive_fade(primitive: &mut crate::scene::Primitive) -> &mut FadeRamps {
    use crate::scene::Primitive;
    match primitive {
        Primitive::Shadow(p) => &mut p.fast_fade,
        Primitive::Quad(p) => &mut p.fast_fade,
        Primitive::Path(p) => &mut p.fast_fade,
        Primitive::Underline(p) => &mut p.fast_fade,
        Primitive::MonochromeSprite(p) => &mut p.fast_fade,
        Primitive::SubpixelSprite(p) => &mut p.fast_fade,
        Primitive::PolychromeSprite(p) => &mut p.fast_fade,
        Primitive::Surface(p) => &mut p.fast_fade,
    }
}

impl Window {
    /// Paints what `f` paints fading out toward the edges of `bounds`: each
    /// edge over `width` of it, `depth` deep at the edge, from 0, no fade,
    /// to 1, faded to nothing. The fade is drawn per pixel, so it fades
    /// text, images and fills over any background, glass and video
    /// included. Content past an edge fades on to nothing; clip it with
    /// [`Window::with_content_mask`] where it should stop at the edge.
    ///
    /// Fades nest edge by edge. Where an enclosing fade and this one fade
    /// the same edge, the one reaching further in is drawn.
    pub fn with_edge_fade<R>(
        &mut self,
        bounds: Bounds<Pixels>,
        width: Edges<Pixels>,
        depth: Edges<f32>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        // On whole device pixels, as the clip's edges are, so that moving
        // what fades by whole pixels moves its edges exactly.
        let ramps = FadeRamps::new(
            self.cover_bounds(bounds),
            width.scale(self.scale_factor()),
            depth,
        );
        let inner = self.fast_edge_fade.within(&ramps);
        let outer = std::mem::replace(&mut self.fast_edge_fade, inner);
        let result = f(self);
        self.fast_edge_fade = outer;
        result
    }
}

/// Prepaints or paints an element's children, `f`, inside the edge fade its
/// style sets, if any: `bounds` are the element's, `content_size` its
/// children's extent and `scroll_offset` how far they are scrolled, which
/// tell how much lies hidden past each edge. Prepaint and paint both run
/// inside it, so that what retention notes in prepaint is what paint draws.
pub(crate) fn children<R>(
    style: &Style,
    bounds: Bounds<Pixels>,
    content_size: Size<Pixels>,
    scroll_offset: Point<Pixels>,
    window: &mut Window,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    let Some(fade) = style.edge_fade else {
        return f(window);
    };
    let rem_size = window.rem_size();
    let region = region(style, bounds, rem_size);
    let depth = if fade.only_hidden {
        let padding = style
            .padding
            .to_pixels(bounds.size.into(), rem_size)
            .map(|edge| window.pixel_snap(*edge));
        let far = Point {
            x: content_size.width + padding.left + padding.right - bounds.size.width,
            y: content_size.height + padding.top + padding.bottom - bounds.size.height,
        };
        let hidden = Edges {
            left: -scroll_offset.x,
            top: -scroll_offset.y,
            right: far.x + scroll_offset.x,
            bottom: far.y + scroll_offset.y,
        };
        let depth = |hidden: Pixels, width: Pixels| {
            if width <= Pixels::ZERO {
                return 0.;
            }
            let part = (hidden / width).clamp(0., 1.);
            part * part * (3. - 2. * part)
        };
        Edges {
            left: depth(hidden.left, fade.left),
            top: depth(hidden.top, fade.top),
            right: depth(hidden.right, fade.right),
            bottom: depth(hidden.bottom, fade.bottom),
        }
    } else {
        Edges::all(1.)
    };
    if depth == Edges::all(0.) {
        return f(window);
    }
    window.with_edge_fade(region, fade.width(), depth, f)
}

/// The region an element's fade runs in: inside its border, where its
/// overflow clips ([`Style::overflow_mask`]).
fn region(style: &Style, bounds: Bounds<Pixels>, rem_size: Pixels) -> Bounds<Pixels> {
    if style
        .border_color
        .is_none_or(|color| color.is_transparent())
    {
        return bounds;
    }
    let border = style.border_widths.to_pixels(rem_size);
    Bounds::from_corners(
        Point {
            x: bounds.left() + border.left,
            y: bounds.top() + border.top,
        },
        Point {
            x: bounds.right() - border.right,
            y: bounds.bottom() - border.bottom,
        },
    )
}

/// The scroll offset an element's children are painted at, from the state
/// its interactivity keeps.
pub(crate) fn scroll_offset(offset: Option<&Rc<RefCell<Point<Pixels>>>>) -> Point<Pixels> {
    offset.map_or(Point::default(), |offset| *offset.borrow())
}

#[cfg(test)]
mod tests {
    use super::{FadeRamps, f16_bits, f16_value, unsmoothstep};
    use crate::{Bounds, Edges, Point, ScaledPixels, point, size};

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

    /// Packed as a half float and read back, a rate is within a half
    /// float's rounding of itself, the extremes held.
    #[test]
    fn rates_survive_packing() {
        for value in [1., -1., 0.5, 1. / 24., -1. / 48., 0.0123, 3.75, -1e-4, 1e-6, 0.] {
            let back = f16_value(f16_bits(value));
            assert!(
                (back - value).abs() <= value.abs() / 2048. + 6e-8,
                "{value} came back {back}"
            );
        }
        assert_eq!(f16_bits(1.), 0x3c00);
        assert_eq!(f16_bits(-2.), 0xc000);
        assert_eq!(f16_value(f16_bits(1e9)), 65504.);
        assert_eq!(f16_bits(f32::NAN), 0);
    }

    #[test]
    fn unsmoothstep_inverts_smoothstep() {
        for step in 0..=20 {
            let value = step as f32 / 20.;
            let t = unsmoothstep(value);
            let back = t * t * (3. - 2. * t);
            assert!((back - value).abs() < 1e-5, "{value} → {t} → {back}");
        }
    }

    /// A full fade is nothing at the edge, eases in over its width and
    /// leaves the content whole from there in; an edge with no width or
    /// depth leaves it whole.
    #[test]
    fn a_ramp_runs_from_nothing_at_the_edge_to_whole_a_width_in() {
        let ramps = FadeRamps::new(
            region(),
            Edges {
                left: ScaledPixels(20.),
                ..widths(0.)
            },
            Edges::all(1.),
        );
        assert!(ramps.alpha_at(at(100., 300.)) < 1e-6);
        assert!((ramps.alpha_at(at(110., 300.)) - 0.5).abs() < 2e-3);
        assert_eq!(ramps.alpha_at(at(120., 300.)), 1.);
        assert_eq!(ramps.alpha_at(at(499., 201.)), 1.);
        // Past the edge it stays at nothing.
        assert_eq!(ramps.alpha_at(at(90., 300.)), 0.);
        assert_eq!(FadeRamps::NONE.alpha_at(at(-1e6, 1e6)), 1.);
        assert!(!FadeRamps::NONE.fades());
        assert!(ramps.fades());
    }

    /// Every edge fades toward itself, the far ones too.
    #[test]
    fn each_edge_fades_toward_itself() {
        let ramps = FadeRamps::new(region(), widths(20.), Edges::all(1.));
        let middle = at(300., 350.);
        assert_eq!(ramps.alpha_at(middle), 1.);
        for (edge, inside) in [
            (at(100., 350.), at(110., 350.)),
            (at(300., 200.), at(300., 210.)),
            (at(500., 350.), at(490., 350.)),
            (at(300., 500.), at(300., 490.)),
        ] {
            assert!(ramps.alpha_at(edge) < 1e-5, "{edge:?}");
            assert!((ramps.alpha_at(inside) - 0.5).abs() < 2e-3, "{inside:?}");
        }
        assert!(ramps.whole_over(Bounds {
            origin: at(120., 220.),
            size: size(ScaledPixels(360.), ScaledPixels(260.)),
        }));
        assert!(!ramps.whole_over(Bounds {
            origin: at(119., 220.),
            size: size(ScaledPixels(360.), ScaledPixels(260.)),
        }));
    }

    /// A shallow fade leaves `1 - depth` at the edge.
    #[test]
    fn depth_is_how_faint_the_edge_gets() {
        let ramps = FadeRamps::new(
            region(),
            Edges {
                right: ScaledPixels(24.),
                ..widths(0.)
            },
            Edges::all(0.25),
        );
        assert!((ramps.alpha_at(at(500., 300.)) - 0.75).abs() < 1e-4);
        assert!(ramps.alpha_at(at(476.5, 300.)) > 0.999);
        assert!(!FadeRamps::new(region(), widths(24.), Edges::all(0.)).fades());
    }

    /// Fades nest edge by edge; of two on one edge, the one reaching
    /// further in is drawn.
    #[test]
    fn nested_fades_meet_edge_by_edge() {
        let list = FadeRamps::new(
            region(),
            Edges {
                top: ScaledPixels(24.),
                bottom: ScaledPixels(24.),
                ..widths(0.)
            },
            Edges::all(1.),
        );
        let title_region = Bounds {
            origin: at(120., 210.),
            size: size(ScaledPixels(200.), ScaledPixels(20.)),
        };
        let title = FadeRamps::new(
            title_region,
            Edges {
                right: ScaledPixels(16.),
                top: ScaledPixels(4.),
                ..widths(0.)
            },
            Edges::all(1.),
        );
        let both = list.within(&title);
        // The title's right edge and the list's top and bottom.
        assert!(both.alpha_at(at(320., 300.)) < 1e-5);
        assert!(both.alpha_at(at(200., 200.)) < 1e-5);
        assert!(both.alpha_at(at(200., 500.)) < 1e-5);
        // The list's top reaches 224, the title's 214: the list's is drawn.
        assert_eq!(both.alpha_at(at(200., 212.)), list.alpha_at(at(200., 212.)));
        assert_eq!(list.within(&FadeRamps::NONE), list);
        assert_eq!(FadeRamps::NONE.within(&list), list);
    }

    /// Moved, a primitive keeps its own ramps where it is and takes the
    /// new ones of the region around it, and its own come out as painting
    /// them afresh where it moved to makes them.
    #[test]
    fn moving_keeps_inner_ramps_with_the_content_and_outer_ones_in_place() {
        let viewport = FadeRamps::new(
            region(),
            Edges {
                top: ScaledPixels(24.),
                ..widths(0.)
            },
            Edges::all(1.),
        );
        let row = |y: f32| Bounds {
            origin: at(100., y),
            size: size(ScaledPixels(400.), ScaledPixels(20.)),
        };
        let title = |y: f32| {
            FadeRamps::new(
                row(y),
                Edges {
                    right: ScaledPixels(16.),
                    ..widths(0.)
                },
                Edges::all(0.6),
            )
        };
        let painted = viewport.within(&title(300.));
        let scrolled = FadeRamps::new(
            region(),
            Edges {
                top: ScaledPixels(24.),
                ..widths(0.)
            },
            Edges::all(0.5),
        );
        let delta = at(-30., -40.);
        let moved = painted.moved(delta, &viewport, &scrolled);
        let afresh = scrolled.within(&FadeRamps::new(
            Bounds {
                origin: at(70., 260.),
                ..row(260.)
            },
            Edges {
                right: ScaledPixels(16.),
                ..widths(0.)
            },
            Edges::all(0.6),
        ));
        assert_eq!(moved, afresh);
        assert_eq!(painted.moved(delta, &painted, &scrolled), scrolled);
        assert_eq!(
            title(300.).translated(at(0., -40.)),
            title(260.),
            "a translated ramp is the ramp painted where it went"
        );
    }
}
