//! What one presented frame shows of its natives, for the platform to
//! reconcile its containers with.

use crate::{
    Bounds, Corners, DrawOrder, Pixels, ScaledPixels, Scene, Size, fast::backdrop::BackdropBlur,
    point, px, size,
};

use super::NativeId;

/// Where each native shows in a frame, bottom to top.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeFrame {
    /// One entry per native placed, in draw order: the first is the lowest.
    pub placements: Vec<NativeFramePlacement>,
    /// The backdrop blurs drawn over a native's hole, bottom to top. GPUI's
    /// own blur reads only its drawable, where a hole is transparent, so the
    /// platform blurs the natives under each of these (see
    /// [`NativeFrameBlur`]).
    pub blurs: Vec<NativeFrameBlur>,
    /// Whether any hole was cut, so GPUI's layer has to be composited over
    /// what is under it rather than drawn opaque.
    pub any_hole: bool,
    /// Whether one native fills the window with nothing GPUI draws above it,
    /// so GPUI's layer can be hidden for the frame.
    pub covers_window: bool,
}

/// Where one native shows, in the window's logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeFramePlacement {
    /// The native.
    pub id: NativeId,
    /// The element's bounds: where the native's content goes.
    pub bounds: Bounds<Pixels>,
    /// The part that can show, grown to whole device pixels: the native's
    /// container covers exactly this, and clips the content to it.
    pub visible: Bounds<Pixels>,
    /// How opaque the native is drawn. GPUI's hole applies it; it is here for
    /// platforms that have to apply it to the container themselves.
    pub opacity: f32,
    /// Its place from the bottom among the frame's natives.
    pub rank: usize,
}

/// A backdrop blur over a native, in the window's logical pixels: where the
/// platform blurs what its natives show under GPUI's layer, as GPUI blurs
/// its own drawable there.
///
/// GPUI's blur reads its drawable, which a hole leaves transparent, so over
/// a hole it blurs GPUI's content alone and the native would show through
/// sharp. The platform puts a blur of what is composited under GPUI's layer
/// (the natives and the window's background) between the natives this blur
/// is drawn over and those drawn over it, clipped to the region. GPUI's
/// layer, with its own blur, goes over that as before: where GPUI's blurred
/// content is opaque it hides the platform's blur, where it is transparent
/// (inside the hole) the blurred native shows, and at the hole's edge the
/// two blurs meet, both soft, so no sharp edge shows.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeFrameBlur {
    /// The part of the region that shows: its bounds clipped by its content
    /// mask, on whole device pixels.
    pub bounds: Bounds<Pixels>,
    /// The region's corners where they show: clamped to half its shorter
    /// side, as GPUI clamps a quad's, and zero where the content mask cut
    /// the corner off.
    pub corner_radii: Corners<Pixels>,
    /// The gaussian's standard deviation.
    pub blur_radius: Pixels,
    /// How opaque the blur is drawn, from the element opacity it was painted
    /// with.
    pub opacity: f32,
    /// How many of the frame's natives, the lowest first, are drawn under the
    /// blur: it goes above the natives whose rank is below this and under
    /// the rest.
    pub natives_below: usize,
}

impl NativeFrameBlur {
    /// `blur` as the platform draws it, if it lies over the part that shows
    /// of one of `natives` drawn under it. `natives` are the frame's
    /// placements in draw order, each with its draw order and the part of it
    /// that shows, in device pixels.
    fn over(
        blur: &BackdropBlur,
        natives: &[(NativeFramePlacement, DrawOrder, Bounds<ScaledPixels>)],
        scale_factor: f32,
    ) -> Option<Self> {
        let shown = blur.clipped_bounds();
        let natives_below = natives
            .iter()
            .take_while(|(_, order, _)| *order < blur.order)
            .count();
        if !natives[..natives_below]
            .iter()
            .any(|(_, _, visible)| visible.intersects(&shown))
        {
            return None;
        }
        Some(Self {
            bounds: unscale(shown, scale_factor),
            corner_radii: shown_corners(blur, shown).map(|radius| px(radius.0 / scale_factor)),
            blur_radius: px(blur.blur_radius.0 / scale_factor),
            opacity: blur.opacity,
            natives_below,
        })
    }
}

/// The corners of `blur`'s region where they show in `shown`, the part of it
/// its content mask leaves: each clamped to half the region's shorter side,
/// as GPUI's quads clamp theirs, and square where the mask cut it off.
fn shown_corners(blur: &BackdropBlur, shown: Bounds<ScaledPixels>) -> Corners<ScaledPixels> {
    let bounds = blur.bounds;
    let max = bounds.size.width.0.min(bounds.size.height.0) / 2.;
    let corner = |radius: ScaledPixels, kept: bool| {
        ScaledPixels(if kept { radius.0.min(max).max(0.) } else { 0. })
    };
    let radii = blur.corner_radii;
    Corners {
        top_left: corner(radii.top_left, shown.origin == bounds.origin),
        top_right: corner(radii.top_right, shown.top_right() == bounds.top_right()),
        bottom_right: corner(
            radii.bottom_right,
            shown.bottom_right() == bounds.bottom_right(),
        ),
        bottom_left: corner(
            radii.bottom_left,
            shown.bottom_left() == bounds.bottom_left(),
        ),
    }
}

impl NativeFrame {
    /// The natives `scene` places, which is finished.
    pub fn new(scene: &Scene, scale_factor: f32, viewport: Size<Pixels>) -> Self {
        let composition = &scene.composition;
        let mut placed: Vec<(NativeFramePlacement, DrawOrder, Bounds<ScaledPixels>)> =
            Vec::with_capacity(composition.placements.len());
        let mut top_order = None;
        for placement in &composition.placements {
            let shown = placement.clipped_bounds();
            let entry = NativeFramePlacement {
                id: placement.id,
                bounds: unscale(placement.bounds, scale_factor),
                visible: unscale(cover(shown), scale_factor),
                opacity: placement.opacity,
                rank: 0,
            };
            // A native placed twice in a frame shows once, where it was drawn last.
            placed.retain(|(kept, _, _)| kept.id != placement.id);
            placed.push((entry, placement.order, shown));
            top_order = Some(placement.order);
        }
        for (rank, (placement, _, _)) in placed.iter_mut().enumerate() {
            placement.rank = rank;
        }
        let blurs = if placed.is_empty() {
            Vec::new()
        } else {
            composition
                .blurs
                .iter()
                .filter_map(|blur| NativeFrameBlur::over(blur, &placed, scale_factor))
                .collect()
        };
        let placements: Vec<NativeFramePlacement> = placed
            .into_iter()
            .map(|(placement, _, _)| placement)
            .collect();
        let window = Bounds::new(point(px(0.), px(0.)), viewport);
        let covers_window = placements
            .last()
            .zip(top_order)
            .is_some_and(|(top, order)| {
                top.opacity >= 1.
                    && top.visible.intersect(&window) == window
                    && !scene.draws_above(order)
            });
        Self {
            any_hole: !placements.is_empty(),
            placements,
            blurs,
            covers_window,
        }
    }

    /// The placement of `id`, if this frame shows it.
    pub fn placement(&self, id: NativeId) -> Option<&NativeFramePlacement> {
        self.placements.iter().find(|placement| placement.id == id)
    }

    /// What changed since `previous` was presented: the placements that are
    /// new or moved, restacked or refaded, the natives no longer placed, and
    /// the blurs over natives if any of them changed.
    pub fn changes_since<'a>(&'a self, previous: &'a NativeFrame) -> NativeChanges<'a> {
        NativeChanges {
            placed: self
                .placements
                .iter()
                .filter(|placement| previous.placement(placement.id) != Some(*placement))
                .collect(),
            hidden: previous
                .placements
                .iter()
                .filter(|placement| self.placement(placement.id).is_none())
                .map(|placement| placement.id)
                .collect(),
            layer_changed: self.any_hole != previous.any_hole
                || self.covers_window != previous.covers_window,
            blurs: (self.blurs != previous.blurs).then_some(self.blurs.as_slice()),
        }
    }
}

/// The difference between two presented frames' natives.
#[derive(Debug, Default)]
pub struct NativeChanges<'a> {
    /// Placements new since the previous frame, or different in any way.
    pub placed: Vec<&'a NativeFramePlacement>,
    /// Natives the previous frame showed and this one does not.
    pub hidden: Vec<NativeId>,
    /// Whether GPUI's layer changes between opaque, composited and hidden.
    pub layer_changed: bool,
    /// The frame's blurs over natives, all of them, when they differ in any
    /// way from the previous frame's; `None` when they are the same.
    pub blurs: Option<&'a [NativeFrameBlur]>,
}

impl NativeChanges<'_> {
    /// Whether nothing has to change on the platform, so the frame can be
    /// presented as any other.
    pub fn is_empty(&self) -> bool {
        !self.moves_natives() && self.blurs.is_none()
    }

    /// Whether a native or GPUI's layer changes, leaving the blurs over
    /// natives aside: what a platform that does not blur natives acts on.
    pub fn moves_natives(&self) -> bool {
        !self.placed.is_empty() || !self.hidden.is_empty() || self.layer_changed
    }
}

/// Grows `bounds` to whole device pixels.
fn cover(bounds: Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
    let left = bounds.origin.x.0.floor();
    let top = bounds.origin.y.0.floor();
    let right = (bounds.origin.x.0 + bounds.size.width.0).ceil();
    let bottom = (bounds.origin.y.0 + bounds.size.height.0).ceil();
    Bounds::new(
        point(ScaledPixels(left), ScaledPixels(top)),
        size(ScaledPixels(right - left), ScaledPixels(bottom - top)),
    )
}

fn unscale(bounds: Bounds<ScaledPixels>, scale_factor: f32) -> Bounds<Pixels> {
    Bounds::new(
        point(
            px(bounds.origin.x.0 / scale_factor),
            px(bounds.origin.y.0 / scale_factor),
        ),
        size(
            px(bounds.size.width.0 / scale_factor),
            px(bounds.size.height.0 / scale_factor),
        ),
    )
}
