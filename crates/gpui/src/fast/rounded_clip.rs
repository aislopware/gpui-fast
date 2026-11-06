//! Rounded overflow (longbridge/gpui-fast#76): what an element clipping its
//! overflow on both axes paints inside it is cut along its padding edge's
//! rounded corners, each ancestor's edge kept where it is (zed#37480: a
//! descendant never takes on an ancestor's radius).
//!
//! Upstream records each clip as a scope in the scene's paint operations and
//! has the renderers draw a scope's primitives as a group of their own with
//! the clip stack bound. This fork already cuts primitives along rounded
//! rectangles per pixel through the table of edge fades
//! ([`crate::fast::edge_fade`], [`Window::with_rounded_clip`]): every
//! primitive carries the index of the fade and clip it is drawn in, in four
//! bytes of padding it had anyway, and retention, moving and scroll layers
//! already carry it. So a padding edge here is one more clip entered in
//! that table, around the element's children as they prepaint and paint,
//! and a scene stays one flat list of primitives. Each entry holds the
//! innermost clip and the one around it that it does not lie inside
//! ([`crate::EdgeFadeRamps`]); the corners of a padding edge inside borders
//! of unequal widths are quarter ellipses.

use crate::{
    App, Bounds, Corners, Overflow, Pixels, ScaledPixels, Size, Style, Window, point, size,
};

/// The curved padding edge, in device pixels. Adjacent borders can have
/// different widths, so the inner corner is generally an ellipse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shape {
    pub(crate) bounds: Bounds<ScaledPixels>,
    pub(crate) radii: Corners<Size<ScaledPixels>>,
}

impl Shape {
    fn for_style(style: &Style, bounds: Bounds<Pixels>, rem: Pixels, scale: f32) -> Option<Self> {
        // CSS curves overflow's padding edge only when both axes clip.
        if style.overflow.x == Overflow::Visible || style.overflow.y == Overflow::Visible {
            return None;
        }
        // Match the radii used to paint this element's background and border.
        let radii = style
            .corner_radii
            .to_pixels(rem)
            .clamp_radii_for_quad_size(bounds.size);
        if radii.max() <= Pixels::ZERO {
            return None;
        }
        let widths = style.border_widths.to_pixels(rem);
        let inner = |radius: Pixels, horizontal: Pixels, vertical: Pixels| {
            size(
                ScaledPixels(f32::from((radius - horizontal).max(Pixels::ZERO)) * scale),
                ScaledPixels(f32::from((radius - vertical).max(Pixels::ZERO)) * scale),
            )
        };
        // The edges cover the device pixels they cross, as the rectangular content mask an
        // element clips to does (`Window::cover_bounds`): a straight edge cuts whole pixels and
        // only the corners are antialiased, as monocode's rectangular overflow cut them
        // (FORK.md, "Onicode"). Each is first put on the 1/64 device pixel grid the shift
        // module reads as exact, so that float noise in where it was added up neither moves
        // it a pixel nor keeps a layer moved by whole pixels from keeping its clip's entry.
        let grid = |v: Pixels| ((f32::from(v) * scale) * 64.).round() / 64.;
        let floor = |v: Pixels| ScaledPixels(grid(v).floor());
        let ceil = |v: Pixels| ScaledPixels(grid(v).ceil());
        let left = floor(bounds.origin.x + widths.left);
        let top = floor(bounds.origin.y + widths.top);
        let right = ceil(bounds.origin.x + bounds.size.width - widths.right).max(left);
        let bottom = ceil(bounds.origin.y + bounds.size.height - widths.bottom).max(top);
        Some(Self {
            bounds: Bounds {
                origin: point(left, top),
                size: size(right - left, bottom - top),
            },
            radii: Corners {
                top_left: inner(radii.top_left, widths.left, widths.top),
                top_right: inner(radii.top_right, widths.right, widths.top),
                bottom_right: inner(radii.bottom_right, widths.right, widths.bottom),
                bottom_left: inner(radii.bottom_left, widths.left, widths.bottom),
            },
        })
    }

    /// The edge fade table's entry for this padding edge. Borders meeting
    /// across the element leave none, and nothing inside shows: its clip is
    /// then a pixel far outside any window, so that what is inside is still
    /// laid out, prepainted, hit and kept as inside any clip.
    fn ramps(&self) -> crate::EdgeFadeRamps {
        if self.bounds.is_empty() {
            const FAR: f32 = -1.0e7;
            return crate::EdgeFadeRamps::NONE.rounded_elliptical(
                Bounds::new(
                    point(ScaledPixels(FAR), ScaledPixels(FAR)),
                    size(ScaledPixels(1.), ScaledPixels(1.)),
                ),
                [0.; 4],
                [0.; 4],
            );
        }
        let r = self.radii;
        let corners = [r.top_left, r.top_right, r.bottom_right, r.bottom_left];
        crate::EdgeFadeRamps::NONE.rounded_elliptical(
            self.bounds,
            corners.map(|radius| radius.width.0),
            corners.map(|radius| radius.height.0),
        )
    }

    /// Whether the rounded rectangle `bounds` with circular corners `radii`
    /// lies inside this edge: as large, to a 64th of a pixel, and as round
    /// or rounder at every corner.
    fn holds(&self, bounds: Bounds<ScaledPixels>, radii: Corners<ScaledPixels>) -> bool {
        const SLACK: f32 = 1. / 64.;
        let near = |a: ScaledPixels, b: ScaledPixels| (a.0 - b.0).abs() <= SLACK;
        let rounder = |radius: ScaledPixels, edge: Size<ScaledPixels>| {
            radius.0 + SLACK >= edge.width.0.max(edge.height.0)
        };
        !self.bounds.is_empty()
            && near(bounds.origin.x, self.bounds.origin.x)
            && near(bounds.origin.y, self.bounds.origin.y)
            && near(bounds.size.width, self.bounds.size.width)
            && near(bounds.size.height, self.bounds.size.height)
            && rounder(radii.top_left, self.radii.top_left)
            && rounder(radii.top_right, self.radii.top_right)
            && rounder(radii.bottom_right, self.radii.bottom_right)
            && rounder(radii.bottom_left, self.radii.bottom_left)
    }

    /// CPU reference for testing the geometric intersection independently of
    /// antialiasing. Check every corner: an unusually thick opposite border
    /// can cut through a corner's quarter ellipse.
    #[cfg(test)]
    fn contains(&self, position: crate::Point<ScaledPixels>) -> bool {
        let b = self.bounds;
        if b.is_empty() || !b.contains(&position) {
            return false;
        }
        let corners = [
            (b.origin, self.radii.top_left, 1., 1.),
            (b.top_right(), self.radii.top_right, -1., 1.),
            (b.bottom_right(), self.radii.bottom_right, -1., -1.),
            (b.bottom_left(), self.radii.bottom_left, 1., -1.),
        ];
        corners.into_iter().all(|(corner, radius, dx, dy)| {
            let rx = radius.width.0;
            let ry = radius.height.0;
            if rx == 0. || ry == 0. {
                return true;
            }
            let x = (position.x.0 - corner.x.0) * dx;
            let y = (position.y.0 - corner.y.0) * dy;
            x >= rx || y >= ry || ((x - rx) / rx).powi(2) + ((y - ry) / ry).powi(2) <= 1.
        })
    }
}

/// Paints an element's children, `paint`, inside its rounded padding edge
/// when it clips its overflow. The element's own background, border and
/// shadows stay outside. The shape is the element's own bounds, never the
/// rectangular content mask they were cropped to.
pub(crate) fn paint_children(
    style: &Style,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
    paint: impl FnOnce(&mut Window, &mut App),
) {
    within(style, bounds, window, |window| paint(window, cx))
}

/// Prepaints an element's children, `f`, inside the clip [`paint_children`]
/// paints them in, which retention notes as they prepaint: a view or element
/// inside drawn again from last frame was drawn in the same clip.
pub(crate) fn prepaint_children<R>(
    style: &Style,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    within(style, bounds, window, f)
}

fn within<R>(
    style: &Style,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    let Some(shape) = Shape::for_style(style, bounds, window.rem_size(), window.scale_factor())
    else {
        return f(window);
    };
    use crate::fast::edge_fade;
    let around = edge_fade::current(window);
    let inner = edge_fade::entered_ramps(window, &shape.ramps());
    if inner == edge_fade::NONE {
        return f(window);
    }
    window
        .fast_edge_fade
        .padding_edges
        .push((shape, inner, around));
    edge_fade::enter(&mut window.next_frame.scene, inner);
    let result = f(window);
    edge_fade::leave(&mut window.next_frame.scene);
    window.fast_edge_fade.padding_edges.pop();
    result
}

/// `quad`, drawn in the fade around the rounded padding edge it is painted
/// inside when it fills that edge (see [`uncut_fade`]).
pub(crate) fn uncut_quad(window: &mut Window, mut quad: crate::Quad) -> crate::Quad {
    if let Some(fade) = uncut_fade(window, quad.bounds, quad.corner_radii) {
        quad.background.pad = fade;
    }
    quad
}

/// The fade a rounded rectangle painted now is drawn in, when the rounded
/// padding edge it is painted inside does not cut it: one filling the edge
/// at its corners or rounder, as a clipped element's fill, glass or border
/// overlay drawn at its own radius does. Such a shape is drawn in the fade
/// around the edge, and antialiased once, at its own edge, as monocode drew
/// it (FORK.md, "Onicode"); cut again along the same curve its outer pixels
/// would be covered twice over. `None` when it is drawn in the fade
/// painting is in.
pub(crate) fn uncut_fade(
    window: &mut Window,
    bounds: Bounds<ScaledPixels>,
    radii: Corners<ScaledPixels>,
) -> Option<u32> {
    let current = crate::fast::edge_fade::current(window);
    let &(shape, entered, around) = window.fast_edge_fade.padding_edges.last()?;
    if entered != current || !shape.holds(bounds, radii) {
        return None;
    }
    if around != crate::fast::edge_fade::NONE {
        return Some(around);
    }
    // Drawn in no fade, yet stamped, so that closing the edge's fade leaves it be: a clip
    // around the whole plane.
    const FAR: f32 = 1.0e7;
    let everywhere = crate::EdgeFadeRamps::NONE.rounded(
        Bounds::new(
            point(ScaledPixels(-FAR), ScaledPixels(-FAR)),
            size(ScaledPixels(2. * FAR), ScaledPixels(2. * FAR)),
        ),
        Corners::default(),
    );
    Some(window.fast_edge_fade.intern(&everywhere))
}

#[cfg(any(test, feature = "test-support"))]
impl Window {
    /// The scene of the frame last drawn, for renderer tests to draw.
    pub fn painted_scene(&self) -> &crate::Scene {
        &self.rendered_frame.scene
    }
}

#[cfg(test)]
mod tests {
    use super::Shape;
    use crate::{Bounds, Corners, Edges, Overflow, ScaledPixels, Style, point, px, size};

    fn style(radius: f32) -> Style {
        Style {
            overflow: point(Overflow::Hidden, Overflow::Hidden),
            corner_radii: Corners::all(px(radius)).map(|radius| (*radius).into()),
            ..Default::default()
        }
    }

    fn shape() -> Shape {
        Shape::for_style(
            &style(24.),
            Bounds::new(point(px(0.), px(0.)), size(px(64.), px(64.))),
            px(16.),
            1.,
        )
        .unwrap()
    }

    fn at(x: f32, y: f32) -> crate::Point<ScaledPixels> {
        point(ScaledPixels(x), ScaledPixels(y))
    }

    /// A padding edge moved by whole device pixels, as a frame drawn again
    /// from the last one moves it, lies where one laid out there afresh
    /// does, to the bit: its sides hold to the grid moving adds to exactly.
    #[test]
    fn a_padding_edge_moved_by_whole_pixels_lies_where_one_laid_out_there_does() {
        for scale in [1., 1.5, 2., 3.] {
            for step in 0..400 {
                let y = step as f32 * 0.37 + 0.13;
                let at_y = |y: f32| {
                    Shape::for_style(
                        &style(12.),
                        Bounds::new(point(px(3.3), px(y)), size(px(100.7), px(60.1))),
                        px(16.),
                        scale,
                    )
                    .unwrap()
                    .bounds
                };
                for pixels in [1., 7., 300., 2377.] {
                    let moved = at_y(y + pixels / scale);
                    let mut expected = at_y(y);
                    expected.origin.y += ScaledPixels(pixels);
                    assert_eq!(moved, expected, "scale {scale}, y {y}, moved {pixels}");
                }
            }
        }
    }

    #[test]
    fn rounded_clip_preserves_original_geometry_under_rectangular_cropping() {
        let mut shape = shape();
        shape.bounds.size.height = ScaledPixels(200.);
        let crop = Bounds::new(at(0., 0.), size(ScaledPixels(64.), ScaledPixels(50.)));
        assert!(!shape.contains(at(1., 1.)));
        assert!(shape.contains(at(1., 49.)) && crop.contains(&at(1., 49.)));
        assert!(!crop.contains(&at(32., 51.)));
        // The table's entry cuts where the shape does, not where the crop is.
        let ramps = shape.ramps();
        assert_eq!(ramps.coverage_at(at(1.5, 49.5)), 1.);
        assert_eq!(ramps.coverage_at(at(0.5, 0.5)), 0.);
    }

    #[test]
    fn rounded_clip_does_not_give_an_inset_rect_an_ancestor_radius() {
        let parent = shape();
        let inset = Bounds::new(at(16., 16.), size(ScaledPixels(32.), ScaledPixels(32.)));
        assert!(parent.contains(at(17., 17.)) && inset.contains(&at(17., 17.)));
        assert!(
            !Shape {
                bounds: inset,
                ..parent
            }
            .contains(at(17., 17.))
        );
        assert_eq!(parent.ramps().coverage_at(at(17.5, 17.5)), 1.);
    }

    #[test]
    fn transparent_unequal_borders_produce_an_elliptical_padding_edge() {
        let mut style = style(24.);
        style.border_widths = Edges {
            left: px(8.).into(),
            top: px(4.).into(),
            right: px(2.).into(),
            bottom: px(6.).into(),
        };
        let shape = Shape::for_style(
            &style,
            Bounds::new(point(px(0.), px(0.)), size(px(64.), px(64.))),
            px(16.),
            2.,
        )
        .unwrap();
        assert_eq!(shape.bounds.origin, at(16., 8.));
        assert_eq!(
            shape.radii.top_left,
            size(ScaledPixels(32.), ScaledPixels(40.))
        );
        assert_eq!(
            shape.radii.bottom_right,
            size(ScaledPixels(44.), ScaledPixels(36.))
        );
        assert!(!shape.contains(at(17., 9.)));
        assert!(shape.contains(at(48., 9.)));
        let ramps = shape.ramps();
        assert_eq!(ramps.coverage_at(at(17.5, 9.5)), 0.);
        assert_eq!(ramps.coverage_at(at(48.5, 9.5)), 1.);
    }

    #[test]
    fn visible_axis_and_square_corners_need_no_rounded_clip() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(64.), px(64.)));
        assert!(Shape::for_style(&style(0.), bounds, px(16.), 1.).is_none());
        let mut style = style(24.);
        style.overflow.x = Overflow::Visible;
        assert!(Shape::for_style(&style, bounds, px(16.), 1.).is_none());
    }

    #[test]
    fn a_clip_inside_one_it_pokes_out_of_keeps_both() {
        // A child reaching past its rounded parent's bottom: the parent's
        // corner still cuts it, and its own top corners cut it too.
        let parent = shape().ramps();
        let child = crate::EdgeFadeRamps::NONE.rounded(
            Bounds::new(at(0., 32.), size(ScaledPixels(64.), ScaledPixels(64.))),
            Corners::all(ScaledPixels(4.)),
        );
        let both = parent.within(&child);
        assert!(both.outer_clip[2] > both.outer_clip[0]);
        assert_eq!(both.coverage_at(at(1.5, 1.5)), 0.);
        assert_eq!(both.coverage_at(at(32.5, 40.5)), 1.);
        assert_eq!(both.coverage_at(at(1.5, 62.5)), 0.);
        // A square-cornered child flush with it lies around the parent's
        // padding edge, which alone clips.
        let flush = parent.within(&crate::EdgeFadeRamps::NONE.rounded(
            Bounds::new(at(0., 0.), size(ScaledPixels(64.), ScaledPixels(64.))),
            Corners::all(ScaledPixels(4.)),
        ));
        assert_eq!((flush.clip, flush.radii), (parent.clip, parent.radii));
        assert!(flush.outer_clip[2] <= flush.outer_clip[0]);
        // One lying well inside the other needs no second clip.
        let inset = crate::EdgeFadeRamps::NONE.rounded(
            Bounds::new(at(16., 16.), size(ScaledPixels(32.), ScaledPixels(32.))),
            Corners::all(ScaledPixels(4.)),
        );
        let nested = parent.within(&inset);
        assert_eq!(nested.clip, inset.clip);
        assert!(nested.outer_clip[2] <= nested.outer_clip[0]);
        // Of two of the same bounds, the larger corners cut further.
        let wider = shape().ramps().within(&crate::EdgeFadeRamps::NONE.rounded(
            Bounds::new(at(0., 0.), size(ScaledPixels(64.), ScaledPixels(64.))),
            Corners::all(ScaledPixels(30.)),
        ));
        assert_eq!(wider.radii, [30.; 4]);
        assert!(wider.outer_clip[2] <= wider.outer_clip[0]);
    }

    #[test]
    fn changing_only_the_clip_changes_a_scroll_tile_s_hash() {
        let scene = |fade: u32| {
            let bounds = Bounds::new(at(0., 0.), size(ScaledPixels(64.), ScaledPixels(64.)));
            let mut background: crate::Background = crate::black().into();
            background.pad = fade;
            let mut scene = crate::Scene::default();
            scene.insert_primitive(crate::Quad {
                bounds,
                content_mask: crate::ContentMask { bounds },
                background,
                ..Default::default()
            });
            scene.finish();
            scene
        };
        let region = Bounds::new(at(0., 0.), size(ScaledPixels(64.), ScaledPixels(64.)));
        let hashes =
            |scene: &crate::Scene| crate::fast::layers::tiles::tile_hashes(scene, 32, region);
        assert_ne!(hashes(&scene(1)), hashes(&scene(2)));
    }

    #[test]
    fn a_clip_moved_keeps_those_set_inside_it_moved() {
        let outer = shape().ramps();
        let inner = crate::EdgeFadeRamps::NONE.rounded(
            Bounds::new(at(0., 32.), size(ScaledPixels(64.), ScaledPixels(64.))),
            Corners::all(ScaledPixels(4.)),
        );
        let drawn = outer.within(&inner);
        let delta = at(0., 10.);
        let moved_outer = outer.translated(delta);
        let moved = drawn.moved(delta, &outer, &moved_outer);
        assert_eq!(moved, moved_outer.within(&inner.translated(delta)));
    }
}
