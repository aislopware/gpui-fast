//! Backdrop blur: what the window drew under a region, painted back blurred
//! inside it, as a frosted popover, menu or composer shows the content
//! behind it.
//!
//! [`Window::paint_backdrop_blur`] places a [`BackdropBlur`] in the scene at
//! its place in painter's order, as monocode placed its marker quad: it
//! takes the next draw order over what its region overlaps, after the quads
//! of that order painted before it, inside a paint layer it moves the rest
//! of the layer above itself, and it is a paint operation of its own, so
//! retention draws it again from last frame with the primitives around it.
//! A renderer meets it in [`Scene::composed_batches`] as
//! [`ComposedBatch::Blurs`] between the primitive batches drawn below it
//! and those drawn over it: it copies what it drew so far under the region
//! (and the blur's reach around it), blurs that, and draws it back clipped
//! to the region's rounded corners, faded as an edge fade or rounded clip
//! around it says, and as opaque as the element opacity it was painted
//! with. What is painted beside the region, in its reach, is read when its
//! order is below the blur's, as monocode's blur read it.
//!
//! Where a blur lies over a native's hole, GPUI's drawable holds nothing
//! of the native to blur: the presented frame hands the blur to the
//! platform ([`crate::fast::composition::NativeFrameBlur`]), and macOS
//! blurs the native under GPUI's layer there with a backdrop layer, in the
//! transaction that moves the hole (`gpui_macos::fast::native_blur`).
//!
//! A scroll container's tiles hold content as it was drawn over the
//! layer's own background, not over what the frame draws under it, so a
//! layer whose content paints a blur is drawn without its tiles, as one
//! whose content places a native is. A list's layer, which draws its rows
//! again into the frame, draws their blurs again too. Tiles are one draw
//! order, so a blur reading over them would read content that drawn
//! directly is ordered above it: a layer that the last frame blurred over
//! draws its content into the frame instead (`fast::layers::paint`).
//!
//! A blur can be glass ([`Window::paint_glass`], [`Glass`]): a custom Liquid Glass for surfaces
//! over what the app draws itself, such as a window background image, which the system's glass,
//! seeing only what is behind the window, cannot sample. Within a bevel along its rounded edge the
//! lightly blurred backdrop is read refracted, pulled in along the edge's normal on a
//! quarter-circle profile as a convex lens's rim bends what lies under it; the whole region is
//! carried through a face transfer that lifts and compresses its luminance and keeps its colour; a
//! thin glint recolours the walls facing the light toward a brighter, more saturated face; and a
//! dark contour just outside draws the edge. Its parameters are whynotmake-it's fit of regular
//! glass to iOS 27 captures; none is zero only for a plain blur.
//!
//! Ported from zui (<https://github.com/zeronsh/zui>, the backdrop blur in
//! `crates/gpui/src/{window,scene}.rs` and `crates/gpui_macos`, at
//! `361ff20`; Apache-2.0, by Wing and the zui contributors; zui is a fork of
//! Zed's GPUI). Changed from zui: the blur is a paint operation ordered as
//! natives' holes are, not a list ordered by its paint layer with an
//! invisible shadow to split batches; it is ordered as monocode's marker
//! quad; its edge is antialiased; it takes the edge fade and
//! rounded clip around it and the element opacity.
//!
//! [`ComposedBatch::Blurs`]: crate::fast::composition::ComposedBatch::Blurs

use crate::{
    Bounds, ContentMask, Corners, DrawOrder, PaintOperation, Pixels, Point, ScaledPixels, Scene,
    Size, Window, px,
};

/// A region of the window whose backdrop, everything drawn under it, is
/// drawn back blurred: see [`Window::paint_backdrop_blur`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct BackdropBlur {
    /// Its draw order among the scene's primitives, like theirs.
    pub order: DrawOrder,
    /// The edge fade (and rounded clip) it is drawn in, an index into
    /// [`Scene::edge_fades`]; 0 for none.
    pub fade: u32,
    /// The region blurred, in device pixels.
    pub bounds: Bounds<ScaledPixels>,
    /// What of it shows.
    pub content_mask: ContentMask<ScaledPixels>,
    /// The region's corners.
    pub corner_radii: Corners<ScaledPixels>,
    /// The blur's standard deviation, in device pixels.
    pub blur_radius: ScaledPixels,
    /// How opaque the blurred backdrop is drawn over what is under it, from
    /// the element opacity it was painted with.
    pub opacity: f32,
    /// How many quads of its draw order are drawn before it: those painted
    /// before it, as monocode drew its marker quad after them (FORK.md,
    /// "Onicode"). Everything else of its order but shadows is drawn over it.
    pub quads_before: u32,
    /// Its glass, all zero for a plain blur.
    pub glass: Glass,
}

/// How a glass blur bends, colours and lights its region ([`Window::paint_glass`]), fitted to
/// Apple's regular Liquid Glass (whynotmake-it's liquid_glass_renderer, matched to iOS 27
/// captures of public SwiftUI glass). Distances are in device pixels once placed. All zero is a
/// plain blur.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Glass {
    /// How far, at the very edge, the backdrop is read from further in; the shader keeps it to
    /// half the shorter side, so the edge never reads past the centre line.
    pub refraction: f32,
    /// How wide the bevel is, where the backdrop is refracted on a quarter-circle profile; at
    /// most a quarter of the shorter side. The face beyond it is not displaced.
    pub bevel: f32,
    /// How strongly the 1.2-point glint along the walls facing the light (top and bottom)
    /// recolours the face toward a brighter, more saturated version of itself.
    pub glint: f32,
    /// How dark the 0.75-point contour just outside the edge is: what draws the edge on a dark
    /// interface, rather than a glow.
    pub contour: f32,
    /// How far the contour gives way where the glint is: 0 keeps it all round, 1 lifts it
    /// entirely where the wall faces the light.
    pub contour_direction: f32,
    /// The face transfer: `emission + transmittance * lift(luma) + chroma * (backdrop - luma)`,
    /// with `lift(Y) = Y + lift * Y * (1 - Y)`. The light added to every pixel.
    pub emission: f32,
    /// How much of the backdrop's (lifted) luminance passes.
    pub transmittance: f32,
    /// How much the backdrop's mid-tones are lifted.
    pub lift: f32,
    /// How much of the backdrop's colour passes: above 1 saturates it.
    pub chroma: f32,
    /// How deep the faint shadow is that the rim casts inward from the light.
    pub shadow: f32,
    /// Device pixels per point, for the glint, contour and shadow widths.
    pub unit: f32,
}

impl Glass {
    /// Regular glass over a dark interface: the face lifts toward grey, the contour is strong and
    /// lifts where the glint is, at a 1x scale.
    pub const DARK: Self = Self {
        refraction: 60.,
        bevel: 20.,
        glint: 0.2,
        contour: 0.88,
        contour_direction: 1.,
        emission: 32. / 255.,
        transmittance: 0.526,
        lift: 0.583,
        chroma: 1.005,
        shadow: 0.036,
        unit: 1.,
    };

    /// Regular glass over a light interface: the face whitens and keeps more colour, the contour
    /// is fainter and stays on most of the edge, at a 1x scale.
    pub const LIGHT: Self = Self {
        refraction: 60.,
        bevel: 20.,
        glint: 0.2,
        contour: 0.43,
        contour_direction: 0.77,
        emission: 0.4,
        transmittance: 0.599,
        lift: 0.216,
        chroma: 1.169,
        shadow: 0.036,
        unit: 1.,
    };

    /// This glass at `factor` device pixels per point.
    #[must_use]
    pub fn scale(self, factor: f32) -> Self {
        Self {
            refraction: self.refraction * factor,
            bevel: self.bevel * factor,
            unit: self.unit * factor,
            ..self
        }
    }

    /// The blur behind glass of `size`: light, so the backdrop still reads, and softer on larger
    /// glass, as the fit measures it on square-ish controls (2.1, 4.5, 5.6 and 8 points at 63, 94,
    /// 118 and 150 points), kept between 1.5 points and 1.78 times the largest measure. Glass is
    /// measured by the square root of its area, so a wide, short card (a toast) scatters as much
    /// as a square of the same area and text behind it does not read through.
    #[must_use]
    pub fn blur_for(size: Size<Pixels>) -> Pixels {
        const FIT: [(f32, f32); 4] = [(63., 2.1), (94., 4.5), (118., 5.6), (150., 8.)];
        let side = (size.width.as_f32().max(0.) * size.height.as_f32().max(0.)).sqrt();
        let at = FIT
            .windows(2)
            .find(|pair| side <= pair[1].0)
            .unwrap_or(&FIT[2..]);
        let ((x0, y0), (x1, y1)) = (at[0], at[1]);
        px((y0 + (side - x0) * (y1 - y0) / (x1 - x0)).clamp(1.5, 8. * 1.78))
    }
}

impl BackdropBlur {
    /// The part of the region that can show.
    pub fn clipped_bounds(&self) -> Bounds<ScaledPixels> {
        self.bounds.intersect(&self.content_mask.bounds)
    }

    /// The region the blur reads from: what shows of it, and three standard
    /// deviations around, where the blur still gathers from.
    pub fn reach(&self) -> Bounds<ScaledPixels> {
        self.clipped_bounds()
            .dilate(ScaledPixels((self.blur_radius.0 * 3.).ceil() + 2.))
    }
}

impl Window {
    /// Paints, inside `bounds` with corners `corner_radii`, everything the
    /// window drew under it blurred by `blur_radius` (the gaussian's
    /// standard deviation): a frosted surface. What is painted after it is
    /// drawn over the blur.
    ///
    /// The blur is clipped by the content mask, faded by an edge fade or
    /// rounded clip around it, and as opaque as the element opacity. Only
    /// Metal draws it: elsewhere nothing is drawn, so paint a translucent
    /// fill over it, which the blur only softens where it is drawn. Over a
    /// native view, macOS blurs the native too. A blur inside a scroll
    /// container keeps the container off its scroll layer.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn paint_backdrop_blur(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        blur_radius: Pixels,
    ) {
        let opacity = self.element_opacity();
        self.place_backdrop_blur(bounds, corner_radii, blur_radius, opacity, Glass::default());
    }

    /// Paints a backdrop blur as [`Window::paint_backdrop_blur`] does, but
    /// whole whatever the element opacity it is painted in, as monocode's
    /// marker blur is drawn (FORK.md, "Onicode"): a surface fading in or out
    /// fades its tint, border and content over a blur at full strength, and
    /// eases the blur's radius itself where the blur should come and go.
    /// Edge fades and rounded clips around it still apply.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn paint_backdrop_blur_whole(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        blur_radius: Pixels,
    ) {
        self.place_backdrop_blur(bounds, corner_radii, blur_radius, 1., Glass::default());
    }

    /// Paints glass over `bounds`: the backdrop blurred as [`Glass::blur_for`] its size says,
    /// refracted within the bevel along its edge, carried through the glass's face transfer,
    /// glinting along the walls facing the light and edged by a dark contour just outside
    /// ([`Glass`]). `glass` is in points and scaled with the window.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn paint_glass(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        glass: Glass,
    ) {
        let blur = Glass::blur_for(bounds.size);
        let glass = glass.scale(self.scale_factor());
        self.place_backdrop_blur(bounds, corner_radii, blur, 1., glass);
    }

    fn place_backdrop_blur(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        blur_radius: Pixels,
        opacity: f32,
        glass: Glass,
    ) {
        self.invalidator.debug_assert_paint();
        if blur_radius <= Pixels::ZERO || opacity <= 0. {
            return;
        }
        let scale_factor = self.scale_factor();
        let snapped = self.snap_bounds(bounds);
        // A radius past half the shorter side would bend the distance field the glass reads.
        let scaled_corners = corner_radii
            .clamp_radii_for_quad_size(bounds.size)
            .scale(scale_factor);
        let blur = BackdropBlur {
            order: 0,
            fade: crate::fast::rounded_clip::uncut_fade(self, snapped, scaled_corners)
                .unwrap_or_else(|| crate::fast::edge_fade::current(self)),
            bounds: snapped,
            content_mask: self.snapped_content_mask(),
            corner_radii: scaled_corners,
            blur_radius: blur_radius.scale(scale_factor),
            opacity,
            quads_before: 0,
            glass,
        };
        self.next_frame.scene.insert_backdrop_blur(blur);
    }
}

impl Scene {
    /// The backdrop blurs of this scene, in draw order once it is finished:
    /// what [`crate::fast::composition::ComposedBatch::Blurs`] indexes.
    pub fn backdrop_blurs(&self) -> &[BackdropBlur] {
        &self.composition.blurs
    }

    /// Places a backdrop blur: it takes the next draw order over what its
    /// region overlaps, after the quads of that order painted before it
    /// (see the module). Inside a paint layer, whose primitives share one order, the rest of
    /// the layer is moved above it.
    pub fn insert_backdrop_blur(&mut self, mut blur: BackdropBlur) {
        let bounds = blur.clipped_bounds();
        if bounds.is_empty() || blur.opacity <= 0. {
            return;
        }
        // Above what its region overlaps, as monocode orders its marker quad: what is painted
        // beside the region, in the blur's reach, is read when it is ordered below the blur, or
        // at its order and painted before it, as monocode's quad batch drew it.
        blur.order = self.primitive_bounds.insert(bounds);
        blur.quads_before = self
            .quads
            .iter()
            .filter(|quad| quad.order == blur.order)
            .count() as u32;
        crate::fast::composition::scene::lift_open_layer(self);
        self.composition.blurs.push(blur);
        crate::fast::scene::push(self, PaintOperation::Backdrop(Box::new(blur)));
    }
}

/// `blur` moved by `delta`.
pub(crate) fn translate(blur: &BackdropBlur, delta: Point<ScaledPixels>) -> BackdropBlur {
    let mut blur = *blur;
    blur.bounds.origin += delta;
    blur.content_mask.bounds.origin += delta;
    blur
}

/// Places `blur` again as the last frame placed it, as [`Scene::replay`]
/// does.
pub(crate) fn replay(scene: &mut Scene, blur: &BackdropBlur) {
    scene.insert_backdrop_blur(*blur);
}

/// `blur`, placed in the finished `scene`, as text two scenes can be
/// compared by.
#[cfg(test)]
pub(crate) fn describe(scene: &Scene, blur: &BackdropBlur) -> String {
    format!(
        "backdrop blur {} {:?} {:?} {:?} {:?} {} {:?}{}",
        blur.order,
        blur.bounds,
        blur.content_mask,
        blur.corner_radii,
        blur.blur_radius,
        blur.opacity,
        blur.glass,
        crate::fast::edge_fade::describe(scene, blur.fade),
    )
}
