//! Radial gradients: a colour at a point fading out across an ellipse around it, the background
//! CSS's `radial-gradient(ellipse <rx> <ry> at <x> <y>, <from>, <to>)` paints. Upstream GPUI's
//! backgrounds are a solid colour, a linear gradient and two patterns; a soft glow, a spotlight or
//! a lit surface needs this one.
//!
//! The ellipse is placed in fractions of the bounds it is painted in, so one background fits a
//! quad of any size: a centre of `(0.5, 0.5)` is the middle, a radius of `1.0` the full width or
//! height. A caller that wants a radius in pixels divides by the bounds it paints.
//!
//! Backgrounds cross to the shaders as one `repr(C)` struct the four renderers share, so a radial
//! gradient rides the fields a linear one leaves alone instead of growing it: the radii in
//! `solid`'s hue and saturation, which [`Background::opacity`] never touches, and the centre in
//! the two stops' percentages. The shaders read them back in `fast_radial_gradient`.

use std::fmt;

use crate::{Background, BackgroundTag, Hsla, LinearColorStop, Point, Size};

/// A radial gradient: `from` at `center`, fading to `to` at the edge of an ellipse `radius`
/// across and down, and `to` beyond it. Both are fractions of the bounds it is painted in.
///
/// <https://developer.mozilla.org/en-US/docs/Web/CSS/gradient/radial-gradient>
pub fn radial_gradient(
    center: Point<f32>,
    radius: Size<f32>,
    from: impl Into<Hsla>,
    to: impl Into<Hsla>,
) -> Background {
    Background {
        tag: BackgroundTag::RadialGradient,
        solid: Hsla {
            h: radius.width,
            s: radius.height,
            l: 0.,
            a: 0.,
        },
        colors: [
            LinearColorStop {
                color: from.into(),
                percentage: center.x,
            },
            LinearColorStop {
                color: to.into(),
                percentage: center.y,
            },
        ],
        ..Default::default()
    }
}

/// Where a radial gradient's ellipse is centred, in fractions of its bounds.
pub(crate) fn center(background: &Background) -> Point<f32> {
    Point {
        x: background.colors[0].percentage,
        y: background.colors[1].percentage,
    }
}

/// A radial gradient's radii, in fractions of its bounds.
pub(crate) fn radius(background: &Background) -> Size<f32> {
    Size {
        width: background.solid.h,
        height: background.solid.s,
    }
}

/// Whether a radial gradient paints nothing: both its colours are clear.
pub(crate) fn is_transparent(background: &Background) -> bool {
    background
        .colors
        .iter()
        .all(|stop| stop.color.is_transparent())
}

/// The colour a radial gradient starts from, at its centre, which the style's clear border takes
/// its hue from as it does a linear gradient's first stop.
pub(crate) fn from_color(background: &Background) -> Hsla {
    background.colors[0].color
}

/// A radial gradient as `Debug` writes it.
pub(crate) fn debug(background: &Background, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let (center, radius) = (center(background), radius(background));
    write!(
        f,
        "RadialGradient(at {}, {}; {} x {}; {:?}, {:?})",
        center.x,
        center.y,
        radius.width,
        radius.height,
        background.colors[0].color,
        background.colors[1].color
    )
}

#[cfg(test)]
mod tests {
    use crate::{hsla, point, size};

    use super::{center, from_color, radial_gradient, radius};

    #[test]
    fn a_radial_gradient_keeps_its_ellipse_through_opacity() {
        let from = hsla(0.7, 0.6, 0.5, 0.2);
        let to = hsla(0.7, 0.6, 0.5, 0.0);
        let glow = radial_gradient(point(0.25, 0.2), size(0.7, 1.5), from, to);
        assert_eq!(center(&glow), point(0.25, 0.2));
        assert_eq!(radius(&glow), size(0.7, 1.5));
        assert!(!glow.is_transparent());
        assert_eq!(from_color(&glow), from);

        let half = glow.opacity(0.5);
        assert_eq!(center(&half), point(0.25, 0.2), "the centre stays");
        assert_eq!(radius(&half), size(0.7, 1.5), "the ellipse stays");
        assert_eq!(half.colors[0].color.a, 0.1, "the colours fade");
        assert!(glow.opacity(0.0).is_transparent());
        assert!(format!("{glow:?}").starts_with("RadialGradient(at 0.25, 0.2; 0.7 x 1.5"));
    }
}
