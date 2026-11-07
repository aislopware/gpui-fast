//! Whether a pointer that left a trigger is on its way to the popup it opened.
//!
//! Upstream keeps a hoverable tooltip for a flat 500 ms after the pointer leaves
//! its element, whichever way it goes: long enough to linger after the pointer
//! moved on, too short for a slow trip to a button inside it. Base UI and
//! floating-ui's `safePolygon` judge the direction instead: from the point the
//! pointer left, a triangle reaches the near edge of the popup, and while the
//! pointer stays inside it the popup stays; once it strays out, the popup goes.
//! A hoverable tooltip goes by [`travel`]: each move toward it starts its
//! 500 ms again from where the pointer now is, so only resting on the way
//! closes it; a popup of a component library can ask [`heading_to`] too.

use crate::{Bounds, Pixels, Point, px};

/// How far past the triangle's sides the pointer may wander and still count as
/// on its way, for a hand that does not move in a straight line.
pub const SLACK: Pixels = px(4.);

/// Whether `pointer`, having left a trigger at `apex`, is still on its way to
/// `popup`: inside the popup, or inside the triangle from `apex` to the popup's
/// edge that faces it, widened by [`SLACK`]. An apex inside the popup has no
/// way to go, so only the popup itself counts.
pub fn heading_to(apex: Point<Pixels>, popup: Bounds<Pixels>, pointer: Point<Pixels>) -> bool {
    if popup.contains(&pointer) {
        return true;
    }
    let (a, b) = if apex.y < popup.top() {
        (popup.origin, popup.top_right())
    } else if apex.y > popup.bottom() {
        (popup.bottom_left(), popup.bottom_right())
    } else if apex.x < popup.left() {
        (popup.origin, popup.bottom_left())
    } else if apex.x > popup.right() {
        (popup.top_right(), popup.bottom_right())
    } else {
        return false;
    };
    within_triangle(apex, a, b, pointer)
}

/// Where a pointer that left a hoverable tooltip's element is, for the tooltip
/// waiting to hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Travel {
    /// Back over the element or on the tooltip: it stays.
    Arrived,
    /// Where it was when last judged: the wait runs on.
    Resting,
    /// Moved and still on its way: the wait starts again from here.
    Heading,
    /// Moved off the way: it hides now.
    Away,
}

/// Judges the window's pointer, which left at (or was last judged at) `apex`,
/// `arrived` when it is over the element or the tooltip.
pub(crate) fn travel(
    arrived: bool,
    apex: Point<Pixels>,
    popup: Bounds<Pixels>,
    window: &crate::Window,
) -> Travel {
    let pointer = window.mouse_position();
    if arrived {
        Travel::Arrived
    } else if pointer == apex {
        Travel::Resting
    } else if heading_to(apex, popup, pointer) {
        Travel::Heading
    } else {
        Travel::Away
    }
}

/// Whether `p` lies inside triangle `a b c`, or within [`SLACK`] of one of its
/// sides.
fn within_triangle(a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>, p: Point<Pixels>) -> bool {
    let point = |p: Point<Pixels>| (f32::from(p.x), f32::from(p.y));
    let (a, b, c, p) = (point(a), point(b), point(c), point(p));
    let cross = |o: (f32, f32), u: (f32, f32), v: (f32, f32)| {
        (u.0 - o.0) * (v.1 - o.1) - (u.1 - o.1) * (v.0 - o.0)
    };
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let negative = d1 < 0. || d2 < 0. || d3 < 0.;
    let positive = d1 > 0. || d2 > 0. || d3 > 0.;
    if !(negative && positive) {
        return true;
    }
    let slack = f32::from(SLACK);
    [(a, b), (b, c), (c, a)]
        .into_iter()
        .any(|(u, v)| distance_to_segment(u, v, p) <= slack)
}

fn distance_to_segment(u: (f32, f32), v: (f32, f32), p: (f32, f32)) -> f32 {
    let (dx, dy) = (v.0 - u.0, v.1 - u.1);
    let length = dx * dx + dy * dy;
    let t = if length == 0. {
        0.
    } else {
        (((p.0 - u.0) * dx + (p.1 - u.1) * dy) / length).clamp(0., 1.)
    };
    let (x, y) = (u.0 + t * dx - p.0, u.1 + t * dy - p.1);
    (x * x + y * y).sqrt()
}

#[cfg(test)]
mod tests {
    use super::{Bounds, Pixels, heading_to, px};
    use crate::{point, size};
    use proptest::{prop_assert, proptest};

    fn popup() -> Bounds<Pixels> {
        Bounds::new(point(px(100.), px(100.)), size(px(200.), px(80.)))
    }

    #[test]
    fn a_straight_trip_to_the_popup_stays_and_a_turn_away_does_not() {
        let apex = point(px(200.), px(60.));
        for y in [61., 70., 80., 90., 99.] {
            assert!(heading_to(apex, popup(), point(px(200.), px(y))));
        }
        // Diagonally toward its corner.
        assert!(heading_to(apex, popup(), point(px(150.), px(90.))));
        // Back up, or off to the side.
        assert!(!heading_to(apex, popup(), point(px(200.), px(50.))));
        assert!(!heading_to(apex, popup(), point(px(60.), px(70.))));
    }

    #[test]
    fn each_side_faces_the_apex() {
        let popup = popup();
        assert!(heading_to(
            point(px(200.), px(220.)),
            popup,
            point(px(200.), px(190.))
        ));
        assert!(heading_to(
            point(px(60.), px(140.)),
            popup,
            point(px(90.), px(140.))
        ));
        assert!(heading_to(
            point(px(340.), px(140.)),
            popup,
            point(px(310.), px(140.))
        ));
        assert!(!heading_to(
            point(px(340.), px(140.)),
            popup,
            point(px(350.), px(140.))
        ));
    }

    #[test]
    fn an_apex_inside_the_popup_counts_only_the_popup() {
        let apex = point(px(150.), px(150.));
        assert!(heading_to(apex, popup(), point(px(160.), px(150.))));
        assert!(!heading_to(apex, popup(), point(px(150.), px(95.))));
    }

    proptest! {
        /// Every point on the segment from the apex to a point of the popup's
        /// facing edge is on the way: a straight trip never closes it.
        #[test]
        fn every_straight_trip_to_the_facing_edge_is_on_the_way(
            apex_x in -200f32..500.,
            apex_y in -200f32..99.,
            along in 0f32..=1.,
            step in 0f32..=1.,
        ) {
            let popup = popup();
            let apex = point(px(apex_x), px(apex_y));
            let target = point(popup.left() + popup.size.width * along, popup.top());
            let pointer = point(
                apex.x + (target.x - apex.x) * step,
                apex.y + (target.y - apex.y) * step,
            );
            prop_assert!(heading_to(apex, popup, pointer));
        }

        /// A point farther from the popup than the apex, on the apex's side,
        /// is never on the way.
        #[test]
        fn moving_away_is_never_on_the_way(
            apex_x in 100f32..300.,
            apex_y in 0f32..99.,
            back in 5f32..200.,
            sideways in -300f32..300.,
        ) {
            let apex = point(px(apex_x), px(apex_y));
            let pointer = point(px(apex_x + sideways), px(apex_y - back));
            prop_assert!(!heading_to(apex, popup(), pointer));
        }
    }
}
