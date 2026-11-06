//! Layer masks: a group of painting drawn into a cleared region and laid
//! back over what was under it through a mask, as CSS's `mask-image` lays an
//! element (monocode's project background and its Haze, FORK.md, "Onicode").
//!
//! [`Window::begin_layer_mask`] and [`Window::end_layer_mask`] bracket the
//! group. Each places a [`LayerMaskCut`] in the scene at its place in
//! painter's order, as a backdrop blur is placed (`fast::backdrop`): it takes
//! the next draw order over its region, inside a paint layer it moves the
//! rest of the layer above itself, and it is a paint operation of its own, so
//! retention draws it again with the primitives around it. The begin is
//! ordered over everything painted under the region before it, the end over
//! everything the group painted there. A renderer meets the cuts in
//! [`Scene::composed_batches`] as [`ComposedBatch::Masks`]: at a begin it
//! copies what it drew so far in the region and clears the region; at the
//! end it copies the group, which it drew over nothing, and draws it back
//! over the first copy, masked. Only Metal does: elsewhere the group is drawn
//! as painted.
//!
//! The mask is monocode's (its `project_background_fragment`): CSS gradient
//! stops down the region, Haze's two gaussian blurs of the group faded in
//! toward the bottom, its darkening with a darker line one CSS pixel in
//! every three, the region's four corner radii and an opacity. The group is
//! the whole of what is painted between the two over the region, text and
//! fills and images together; what it paints beyond the region is drawn as
//! painted, as monocode's is, so clip the group to it.
//!
//! A scroll container whose content paints a mask is drawn without its
//! tiles, as one whose content paints a blur is (`fast::layers::overlay`).
//!
//! [`ComposedBatch::Masks`]: crate::fast::composition::ComposedBatch::Masks

use crate::{
    Bounds, ContentMask, Corners, DrawOrder, EdgeFade, Edges, PaintOperation, Pixels, Point,
    ScaledPixels, Scene, Window,
};

/// How a [`Window::begin_layer_mask`] group is laid back over what was
/// under it: monocode's project background mask, or its group fade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerMask {
    /// A group fade ([`Window::with_group_fade`]) rather than the project
    /// background: the region is not cleared, and the group is mixed back
    /// into what was under it by the fade's alpha. The fields below go
    /// unused.
    pub fade: Option<GroupFade>,
    /// The stops of a project with no session yet, which keep more of the
    /// image lower down; otherwise a session's, which fade it sooner.
    pub empty: bool,
    /// Haze: the group's soft and strong blurs faded in toward the bottom,
    /// darkened, with the darker line.
    pub haze: bool,
    /// A small preview: Haze's narrowest blurs, with the empty stops.
    pub preview: bool,
    /// How opaque the masked group is, from 0 to 1.
    pub opacity: f32,
}

impl LayerMask {
    /// monocode's project background mask (its `background_mask_marker`'s
    /// arguments, without `begin`).
    pub fn project_background(empty: bool, haze: bool, preview: bool, opacity: f32) -> Self {
        Self {
            fade: None,
            empty,
            haze,
            preview,
            opacity: opacity.clamp(0., 1.),
        }
    }

    /// A group fade's mask: see [`Window::with_group_fade`].
    pub fn group_fade(fade: GroupFade) -> Self {
        Self {
            fade: Some(fade),
            empty: false,
            haze: false,
            preview: false,
            opacity: 1.,
        }
    }

    /// monocode's mode: 1 empty and 2 not, without Haze; with it, 3 empty,
    /// 4 not, and 5 a preview, which takes the empty stops. A group fade is
    /// 6 by its edges and 7 by a raw ramp along x.
    pub fn mode(&self) -> u32 {
        if let Some(fade) = &self.fade {
            return if fade.ramp_x.is_some() { 7 } else { 6 };
        }
        match (self.haze, self.preview, self.empty) {
            (false, _, true) => 1,
            (false, _, false) => 2,
            (true, true, _) => 5,
            (true, false, true) => 3,
            (true, false, false) => 4,
        }
    }

    /// Whether a begin clears the region: every mask but a group fade, which
    /// mixes the group into what it is drawn over.
    pub fn clears(&self) -> bool {
        self.fade.is_none()
    }

    /// The standard deviations of Haze's soft and strong blurs in CSS
    /// pixels, if it has them.
    pub fn haze_sigmas(&self) -> Option<(f32, f32)> {
        match self.mode() {
            5 => Some((2., 5.)),
            4 => Some((5., 12.)),
            3 => Some((7., 18.)),
            _ => None,
        }
    }
}

/// How a group fade ([`Window::with_group_fade`]) mixes its group into what
/// was under it, as monocode's `edge_fade_fragment` and
/// `text_reveal_fragment` do, in device pixels from the region's top left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GroupFade {
    /// How far in each edge's linear ramp runs, from nothing at the edge to
    /// whole; zero leaves the edge sharp. The four take their least.
    pub width: Edges<f32>,
    /// How faint each edge gets, from 0 to 1.
    pub depth: Edges<f32>,
    /// monocode's text reveal, `(start, end)` from the region's left:
    /// `saturate((end - x) / (end - start))`, in place of the edges.
    pub ramp_x: Option<(f32, f32)>,
}

impl GroupFade {
    /// `fade`'s ramps over a region at `scale_factor` device pixels to a
    /// pixel, as a group fade draws them: each edge linear, an axis' two
    /// edges taking the lesser.
    pub fn from_edge_fade(fade: &EdgeFade, scale_factor: f32) -> Self {
        Self {
            width: fade.width.map(|width: &Pixels| width.0 * scale_factor),
            depth: fade.depth,
            ramp_x: fade
                .ramp_x
                .map(|(start, end)| (start.0 * scale_factor, end.0 * scale_factor)),
        }
    }

    /// Whether it fades anything.
    pub fn fades(&self) -> bool {
        let edge = |width: f32, depth: f32| width > 0. && depth > 0.;
        self.ramp_x.is_some()
            || edge(self.width.top, self.depth.top)
            || edge(self.width.right, self.depth.right)
            || edge(self.width.bottom, self.depth.bottom)
            || edge(self.width.left, self.depth.left)
    }
}

/// One end of a layer mask in a scene: see [`Window::begin_layer_mask`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerMaskCut {
    /// Its draw order among the scene's primitives, like theirs.
    pub order: DrawOrder,
    /// Which mask it ends: a begin and its end share it, and no other
    /// mask of the scene does.
    pub id: u32,
    /// Whether it begins the group, rather than ending it.
    pub begin: bool,
    /// The region masked, in device pixels.
    pub bounds: Bounds<ScaledPixels>,
    /// What of it shows.
    pub content_mask: ContentMask<ScaledPixels>,
    /// The region's corners.
    pub corner_radii: Corners<ScaledPixels>,
    /// How the group is laid back.
    pub mask: LayerMask,
    /// Device pixels to a CSS pixel: Haze's blurs and its line are sized in
    /// CSS pixels.
    pub scale: f32,
    /// How many quads of its draw order are drawn before it: those painted
    /// before it, as monocode drew its marker quad after them (FORK.md,
    /// "Onicode"). Everything else of its order but shadows is drawn after it.
    pub quads_before: u32,
}

impl LayerMaskCut {
    /// The part of the region that can show.
    pub fn clipped_bounds(&self) -> Bounds<ScaledPixels> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl Window {
    /// Begins a group of painting laid back over what is under `bounds`
    /// through `mask` when [`Window::end_layer_mask`] ends it, with corners
    /// `corner_radii`: everything painted in between is drawn into the
    /// region cleared, and only the region shows of it. Masks nest.
    ///
    /// Only Metal draws the mask: elsewhere the group is drawn as painted. A
    /// mask inside a scroll container keeps the container off its scroll
    /// layer.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn begin_layer_mask(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        mask: LayerMask,
    ) {
        self.invalidator.debug_assert_paint();
        let scale_factor = self.scale_factor();
        let cut = LayerMaskCut {
            order: 0,
            id: 0,
            begin: true,
            bounds: self.snap_bounds(bounds),
            content_mask: self.snapped_content_mask(),
            corner_radii: corner_radii.scale(scale_factor),
            mask,
            scale: scale_factor,
            quads_before: 0,
        };
        self.next_frame.scene.insert_layer_mask(cut);
    }

    /// Ends the innermost group [`Window::begin_layer_mask`] began, drawing
    /// it back through its mask. Without one open, it does nothing.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn end_layer_mask(&mut self) {
        self.invalidator.debug_assert_paint();
        let scene = &mut self.next_frame.scene;
        if let Some(begin) = scene.composition.open_masks.last() {
            let end = LayerMaskCut {
                begin: false,
                ..*begin
            };
            scene.insert_layer_mask(end);
        }
    }

    /// Paints `f` as a group faded as one inside `bounds` through `fade`'s
    /// ramps, as monocode's marker groups are: what was drawn under the
    /// region is copied before the group, what is drawn with it after, and
    /// the region is drawn back as `mix(before, after, alpha)`, `alpha` the
    /// least of `fade`'s linear edge ramps (each from `1 - depth` at the
    /// edge to whole a width in) or its raw ramp along x. The group's text,
    /// fills and images fade together, so a glyph over a fill shows none of
    /// the fill through it, where [`Window::with_edge_fade`] fades each
    /// primitive by itself. What the group draws past the region is left as
    /// drawn, as monocode's is. The region is `bounds` rounded to device
    /// pixels as a quad's are; nothing in `f` should fade by itself too.
    ///
    /// Each group ends the renderer's pass twice, so keep it to the
    /// surfaces monocode faded so. Only Metal draws the fade: elsewhere the
    /// group is drawn as painted. A group fade inside a scroll container
    /// keeps the container off its scroll layer.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn with_group_fade<R>(
        &mut self,
        bounds: Bounds<Pixels>,
        fade: EdgeFade,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let fade = GroupFade::from_edge_fade(&fade, self.scale_factor());
        if !fade.fades() {
            return f(self);
        }
        self.with_layer_mask(bounds, Corners::default(), LayerMask::group_fade(fade), f)
    }

    /// Paints `f` as a group masked by `mask` inside `bounds`: a
    /// [`Window::begin_layer_mask`] before it and a
    /// [`Window::end_layer_mask`] after.
    pub fn with_layer_mask<R>(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        mask: LayerMask,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.begin_layer_mask(bounds, corner_radii, mask);
        let result = f(self);
        self.end_layer_mask();
        result
    }
}

impl Scene {
    /// The layer masks' cuts of this scene, in draw order once it is
    /// finished: what [`crate::fast::composition::ComposedBatch::Masks`]
    /// indexes. Every begin listed has its end.
    pub fn layer_masks(&self) -> &[LayerMaskCut] {
        &self.composition.masks
    }

    /// Places a cut of a layer mask: a begin opens a mask, an end closes the
    /// innermost open one (and is dropped when none is). It takes the next
    /// draw order over its region, so that everything painted before it
    /// there is drawn before it. Inside a paint layer, whose primitives
    /// share one order, the rest of the layer is moved above it.
    pub fn insert_layer_mask(&mut self, mut cut: LayerMaskCut) {
        let composition = &mut self.composition;
        if cut.begin {
            cut.id = composition.mask_ids;
            composition.mask_ids += 1;
        } else {
            let Some(begin) = composition.open_masks.pop() else {
                return;
            };
            cut.id = begin.id;
        }
        let region = cut.clipped_bounds();
        // A region that shows nothing is still opened and closed, so that
        // its end pairs as painted; neither cut is drawn.
        if !region.is_empty() {
            cut.order = self.primitive_bounds.insert(region);
            cut.quads_before = self
                .quads
                .iter()
                .filter(|quad| quad.order == cut.order)
                .count() as u32;
            crate::fast::composition::scene::lift_open_layer(self);
            self.composition.masks.push(cut);
        }
        if cut.begin {
            self.composition.open_masks.push(cut);
        }
        crate::fast::scene::push(self, PaintOperation::LayerMask(Box::new(cut)));
    }
}

/// Sorts the finished scene's cuts into draw order, a begin before its end
/// at one order, and drops the begins of masks never ended.
pub(crate) fn sort(masks: &mut Vec<LayerMaskCut>, open: &mut Vec<LayerMaskCut>) {
    if !open.is_empty() {
        masks.retain(|cut| !open.iter().any(|open| open.id == cut.id));
        open.clear();
    }
    masks.sort_by_key(|cut| (cut.order, cut.quads_before, !cut.begin));
}

/// `cut` moved by `delta`.
pub(crate) fn translate(cut: &LayerMaskCut, delta: Point<ScaledPixels>) -> LayerMaskCut {
    let mut cut = *cut;
    cut.bounds.origin += delta;
    cut.content_mask.bounds.origin += delta;
    cut
}

/// Places `cut` again as the last frame placed it, as [`Scene::replay`]
/// does.
pub(crate) fn replay(scene: &mut Scene, cut: &LayerMaskCut) {
    scene.insert_layer_mask(*cut);
}

/// `cut`, placed in the finished `scene`, as text two scenes can be
/// compared by.
#[cfg(test)]
pub(crate) fn describe(cut: &LayerMaskCut) -> String {
    format!(
        "layer mask {} {} {} {:?} {:?} {:?} {:?} {}",
        if cut.begin { "begin" } else { "end" },
        cut.order,
        cut.id,
        cut.bounds,
        cut.content_mask,
        cut.corner_radii,
        cut.mask,
        cut.scale,
    )
}
