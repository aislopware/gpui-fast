//! What one presented frame shows of its natives, for the platform to
//! reconcile its containers with.

use crate::{Bounds, Pixels, ScaledPixels, Scene, Size, point, px, size};

use super::NativeId;

/// Where each native shows in a frame, bottom to top.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeFrame {
    /// One entry per native placed, in draw order: the first is the lowest.
    pub placements: Vec<NativeFramePlacement>,
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

impl NativeFrame {
    /// The natives `scene` places, which is finished.
    pub fn new(scene: &Scene, scale_factor: f32, viewport: Size<Pixels>) -> Self {
        let composition = &scene.composition;
        let mut placements: Vec<NativeFramePlacement> =
            Vec::with_capacity(composition.placements.len());
        let mut top_order = None;
        for placement in &composition.placements {
            let entry = NativeFramePlacement {
                id: placement.id,
                bounds: unscale(placement.bounds, scale_factor),
                visible: unscale(cover(placement.clipped_bounds()), scale_factor),
                opacity: placement.opacity,
                rank: 0,
            };
            // A native placed twice in a frame shows once, where it was drawn last.
            placements.retain(|placed| placed.id != placement.id);
            placements.push(entry);
            top_order = Some(placement.order);
        }
        for (rank, placement) in placements.iter_mut().enumerate() {
            placement.rank = rank;
        }
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
            covers_window,
        }
    }

    /// The placement of `id`, if this frame shows it.
    pub fn placement(&self, id: NativeId) -> Option<&NativeFramePlacement> {
        self.placements.iter().find(|placement| placement.id == id)
    }

    /// What changed since `previous` was presented: the placements that are
    /// new or moved, restacked or refaded, and the natives no longer placed.
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
}

impl NativeChanges<'_> {
    /// Whether nothing has to change on the platform, so the frame can be
    /// presented as any other.
    pub fn is_empty(&self) -> bool {
        self.placed.is_empty() && self.hidden.is_empty() && !self.layer_changed
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
