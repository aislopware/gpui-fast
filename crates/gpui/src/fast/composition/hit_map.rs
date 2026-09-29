//! Where each native takes the pointer: where its hitbox is the topmost one,
//! as GPUI's own hit test would find it.

use smallvec::SmallVec;

use crate::{Bounds, Hitbox, Pixels, Point};

use super::{NativeId, SceneComposition};

/// Where the natives of a presented frame take the pointer. Built once per
/// presented frame from its hitboxes, and consulted by the platform's hit
/// testing without going back into GPUI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeHitMap {
    /// The natives that take the pointer, topmost first.
    pub entries: Vec<NativeHitEntry>,
}

/// Where one native takes the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeHitEntry {
    /// The native.
    pub id: NativeId,
    /// Its hitbox, clipped to the hitbox's content mask.
    pub visible: Bounds<Pixels>,
    /// The hitboxes above it within `visible`, clipped to it.
    pub occluders: SmallVec<[Bounds<Pixels>; 4]>,
}

impl NativeHitMap {
    /// The hit map of a frame whose natives are `composition` and whose
    /// hitboxes are `hitboxes`, in the order they were inserted.
    pub(crate) fn new(composition: &SceneComposition, hitboxes: &[Hitbox]) -> Self {
        let mut entries: Vec<(usize, NativeHitEntry)> = Vec::new();
        for placement in &composition.placements {
            let Some(hitbox_id) = placement.hitbox else {
                continue;
            };
            let Some(index) = hitboxes.iter().rposition(|hitbox| hitbox.id == hitbox_id) else {
                continue;
            };
            let hitbox = &hitboxes[index];
            let visible = hitbox.bounds.intersect(&hitbox.content_mask.bounds);
            if visible.is_empty() {
                continue;
            }
            let occluders = hitboxes[index + 1..]
                .iter()
                .map(|above| {
                    above
                        .bounds
                        .intersect(&above.content_mask.bounds)
                        .intersect(&visible)
                })
                .filter(|occluder| !occluder.is_empty())
                .collect();
            entries.retain(|(_, entry)| entry.id != placement.id);
            entries.push((
                index,
                NativeHitEntry {
                    id: placement.id,
                    visible,
                    occluders,
                },
            ));
        }
        entries.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
        Self {
            entries: entries.into_iter().map(|(_, entry)| entry).collect(),
        }
    }

    /// The native that takes the pointer at `position`, if any.
    pub fn native_at(&self, position: Point<Pixels>) -> Option<NativeId> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.visible.contains(&position))?;
        let occluded = entry
            .occluders
            .iter()
            .any(|occluder| occluder.contains(&position));
        (!occluded).then_some(entry.id)
    }

    /// Whether no native takes the pointer anywhere.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
