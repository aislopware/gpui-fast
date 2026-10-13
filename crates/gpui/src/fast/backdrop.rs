//! Backdrop blur: what the window drew under a region, painted back blurred
//! inside it, as a frosted popover, menu or composer shows the content
//! behind it.
//!
//! [`Window::paint_backdrop_blur`] places a [`BackdropBlur`] in the scene at
//! its place in painter's order, as a native's placement is placed
//! (`fast::composition`): it takes the next draw order over what it reads
//! from (the region and the blur's reach around it), inside a paint layer
//! it moves the rest of the layer above itself, and it is a paint operation
//! of its own, so retention draws it again from last frame with the
//! primitives around it. A renderer meets it in [`Scene::composed_batches`]
//! as [`ComposedBatch::Blurs`] between the primitive batches drawn below it
//! and those drawn over it: it copies what it drew so far under the region
//! (and the blur's reach around it), blurs that, and draws it back clipped
//! to the region's rounded corners, faded as an edge fade or rounded clip
//! around it says, and as opaque as the element opacity it was painted
//! with.
//!
//! Each blur ending the renderer's pass costs a store and a load of the
//! whole drawable, so blurs of one radius share a pass break where they
//! can: a blur joins the draw order of an earlier one (a wave) when that
//! order is above everything its own region overlaps, so nothing between
//! the two is drawn under its region. When the wave's order is below the
//! blur's own, what was drawn between them inside the blur's reach but
//! outside its region (another frosted surface's fill, three standard
//! deviations off) is missing from the edge of its blur; nothing under its
//! region ever is. Its region alone goes in the bounds tree, so what is
//! painted after it beside its region is not lifted above it, and a column
//! of frosted badges in a transcript is one wave rather than a chain.
//!
//! A scroll container's tiles hold content as it was drawn over the
//! layer's own background, not over what the frame draws under it, so a
//! layer whose content paints a blur is drawn without its tiles, as one
//! whose content places a native is. A list's layer, which draws its rows
//! again into the frame, draws their blurs again too.
//!
//! Ported from zui (<https://github.com/zeronsh/zui>, the backdrop blur in
//! `crates/gpui/src/{window,scene}.rs` and `crates/gpui_macos`, at
//! `361ff20`; Apache-2.0, by Wing and the zui contributors; zui is a fork of
//! Zed's GPUI). Changed from zui: the blur is a paint operation ordered as
//! natives' holes are, not a list ordered by its paint layer with an
//! invisible shadow to split batches; it is ordered over everything it
//! reads, so that rows a scroll layer draws as tiles and rows drawn into
//! the frame are read alike; blurs share pass breaks, where zui ends the
//! pass once per blur; its edge is antialiased; it takes the edge fade and
//! rounded clip around it and the element opacity.
//!
//! [`ComposedBatch::Blurs`]: crate::fast::composition::ComposedBatch::Blurs

use crate::{
    Bounds, ContentMask, Corners, DrawOrder, PaintOperation, Pixels, Point, ScaledPixels, Scene,
    Window,
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
            .dilate(ScaledPixels((self.blur_radius.0 * 3.).ceil() + 1.))
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
    /// fill over it, which the blur only softens where it is drawn. A blur
    /// inside a scroll container keeps the container off its scroll layer.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn paint_backdrop_blur(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        blur_radius: Pixels,
    ) {
        self.invalidator.debug_assert_paint();
        let opacity = self.element_opacity();
        if blur_radius <= Pixels::ZERO || opacity <= 0. {
            return;
        }
        let scale_factor = self.scale_factor();
        let blur = BackdropBlur {
            order: 0,
            fade: crate::fast::edge_fade::current(self),
            bounds: self.snap_bounds(bounds),
            content_mask: self.snapped_content_mask(),
            corner_radii: corner_radii.scale(scale_factor),
            blur_radius: blur_radius.scale(scale_factor),
            opacity,
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

    /// Places a backdrop blur: it takes the next draw order over what it
    /// reads from, its [`BackdropBlur::reach`], so that everything painted
    /// before it there is drawn before it; or joins blurs of its radius
    /// drawn at an order between what its region overlaps and that, so
    /// that they share one copy of what is under them (see the module).
    /// Inside a paint layer, whose primitives share one order, the rest of
    /// the layer is moved above it.
    pub fn insert_backdrop_blur(&mut self, mut blur: BackdropBlur) {
        let bounds = blur.clipped_bounds();
        if bounds.is_empty() || blur.opacity <= 0. {
            return;
        }
        let radius = blur.blur_radius.0.to_bits();
        let waves = &mut self.composition.waves;
        blur.order =
            self.primitive_bounds
                .insert_reading(&blur.reach(), bounds, |natural, covered| {
                    let same = || {
                        waves
                            .iter()
                            .filter(|wave| wave.1 == radius)
                            .map(|wave| wave.0)
                    };
                    // A wave at or past the natural order reads all this blur
                    // reads; one below it but above all its region overlaps misses
                    // only what was drawn after the wave around the region.
                    let joined = same()
                        .filter(|order| *order >= natural)
                        .min()
                        .or_else(|| same().filter(|order| *order > covered).max());
                    joined.unwrap_or_else(|| {
                        waves.push((natural, radius));
                        natural
                    })
                });
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
        "backdrop blur {} {:?} {:?} {:?} {:?} {}{}",
        blur.order,
        blur.bounds,
        blur.content_mask,
        blur.corner_radii,
        blur.blur_radius,
        blur.opacity,
        crate::fast::edge_fade::describe(scene, blur.fade),
    )
}
