//! The tables [`TaffyLayoutEngine::layout_bounds`] caches absolute bounds in,
//! indexed by the node's Taffy slot instead of hashed.
//!
//! Every element asks for its bounds in prepaint, thousands of them a frame,
//! and each first query of a node looks its parent up, then stores the node's
//! origin and bounds. Upstream keeps both in `FxHashMap<LayoutId, _>`s and
//! clears them at the end of every frame. [`LayoutIdMap`] answers the same
//! calls from a `Vec` indexed by the slot half of the node id, and forgets
//! everything at the end of a frame by starting a new pass instead of
//! clearing. What `layout_bounds` computes, pixel snapping included, is left
//! exactly as upstream wrote it.
//!
//! [`TaffyLayoutEngine::layout_bounds`]: crate::TaffyLayoutEngine::layout_bounds

use crate::{LayoutId, Point, TaffyLayoutEngine};

/// A place in device pixels snapped to a device pixel: to the nearest one,
/// ties toward the pixel below.
///
/// Upstream snaps a node's bounds half toward zero, which is the same on
/// places at or past the window's top and left edges, and the other way past
/// them. A view drawn cached is laid out on its own and placed at its bounds,
/// where what sticks out above or left of it lies at a negative place, and a
/// box half a device pixel past its top would snap a pixel lower than it
/// does laid out in place, at a positive place in the window. Ties toward
/// the pixel below snap a place alike wherever it is measured from.
#[inline]
pub(crate) fn snap(value: f32) -> f32 {
    // Adding zero turns the negative zero `ceil` gives for a half into zero.
    (value - 0.5).ceil() + 0.
}

/// Where layout places a node without a parent, at `location`: as far into
/// its device pixel as [`TaffyLayoutEngine::place_root`] placed it, or not.
pub(crate) fn root_origin(
    engine: &TaffyLayoutEngine,
    id: LayoutId,
    location: taffy::Point<f32>,
) -> Point<f32> {
    let phase = engine.retention.root_phases.get(&id).copied();
    Point::from(location) + phase.unwrap_or_default()
}

impl TaffyLayoutEngine {
    /// Places `root`, laid out on its own for a view drawn at its bounds, at
    /// `phase` in its device pixel, where the view lies laid out in place
    /// (see [`Self::layout_phase`]), so that what it holds snaps as it does
    /// there.
    pub(crate) fn place_root(&mut self, root: LayoutId, phase: Point<f32>) {
        self.retention.root_phases.insert(root, phase);
    }

    /// How far past the device pixel its bounds snap to layout placed the
    /// node, in device pixels, once [`TaffyLayoutEngine::layout_bounds`] has
    /// placed it this frame.
    ///
    /// Bounds are snapped from where layout places a node in the window, not
    /// from where it lies in its parent, so what a node holds snaps by where
    /// the node itself lies within its pixel: a node moved by part of a
    /// device pixel can snap where it did while a box it centres, half a
    /// device pixel off a pixel's edge, snaps to the next pixel. Two places
    /// with the same phase snap everything inside alike, moved by the
    /// difference of the two.
    pub(crate) fn layout_phase(&self, id: LayoutId) -> Point<f32> {
        self.absolute_outer_origins
            .get(&id)
            .map_or_else(Point::default, |origin| origin.map(|c| c - snap(c)))
    }
}

/// One slot of a [`LayoutIdMap`].
#[derive(Clone, Copy, Default)]
struct Entry<V> {
    /// The whole node id, slot and version, since Taffy gives a removed node's
    /// slot to the next node it creates.
    node: u64,
    /// The pass the value was stored in. It stands only in the current one.
    pass: u32,
    value: V,
}

/// A map from layout node to `V`, for the few calls `layout_bounds` and
/// `compute_layout` make of their caches: `get`, `insert`, `remove` and
/// `clear`, as on the `FxHashMap` it replaces.
pub(crate) struct LayoutIdMap<V> {
    /// Indexed by the slot of the node's id. Slots are dense, as Taffy reuses
    /// those of removed nodes.
    entries: Vec<Entry<V>>,
    /// Entries of any other pass are absent. Never 0, the pass of an entry that
    /// was never stored or has been removed.
    pass: u32,
}

impl<V> Default for LayoutIdMap<V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            pass: 1,
        }
    }
}

/// The id Taffy gave the node: a slotmap key's `as_ffi` value, the version in
/// the high half and the slot in the low.
#[inline]
fn raw(id: &LayoutId) -> u64 {
    u64::from(taffy::NodeId::from(*id))
}

#[inline]
fn slot(raw: u64) -> usize {
    raw as u32 as usize
}

impl<V: Copy + Default> LayoutIdMap<V> {
    #[inline]
    pub(crate) fn get(&self, id: &LayoutId) -> Option<&V> {
        let raw = raw(id);
        self.entries
            .get(slot(raw))
            .filter(|entry| entry.pass == self.pass && entry.node == raw)
            .map(|entry| &entry.value)
    }

    #[inline]
    pub(crate) fn insert(&mut self, id: LayoutId, value: V) {
        let raw = raw(&id);
        let slot = slot(raw);
        if slot >= self.entries.len() {
            self.entries.resize(slot + 1, Entry::default());
        }
        self.entries[slot] = Entry {
            node: raw,
            pass: self.pass,
            value,
        };
    }

    #[inline]
    pub(crate) fn remove(&mut self, id: &LayoutId) {
        let raw = raw(id);
        if let Some(entry) = self.entries.get_mut(slot(raw))
            && entry.node == raw
        {
            entry.pass = 0;
        }
    }

    /// Forgets every entry, at the end of a frame, without touching them.
    pub(crate) fn clear(&mut self) {
        self.pass = self.pass.wrapping_add(1);
        if self.pass == 0 {
            // Four billion frames on, entries stored in pass 1 would be back.
            self.entries.fill(Entry::default());
            self.pass = 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LayoutIdMap;
    use crate::LayoutId;
    use taffy::{TaffyTree, style::Style};

    #[test]
    fn entries_last_one_pass_and_belong_to_one_node_of_a_slot() {
        let mut taffy: TaffyTree<()> = TaffyTree::new();
        let first: LayoutId = taffy.new_leaf(Style::default()).unwrap().into();
        let mut map = LayoutIdMap::<u32>::default();
        map.insert(first, 7);
        assert_eq!(map.get(&first), Some(&7));

        // A node given the removed node's slot does not see its entry.
        taffy.remove(first.into()).unwrap();
        let second: LayoutId = taffy.new_leaf(Style::default()).unwrap().into();
        assert_ne!(first, second);
        assert_eq!(map.get(&second), None);
        map.remove(&second);
        assert_eq!(map.get(&first), Some(&7));

        map.insert(second, 8);
        assert_eq!(map.get(&second), Some(&8));
        assert_eq!(map.get(&first), None);
        map.remove(&second);
        assert_eq!(map.get(&second), None);

        map.insert(second, 9);
        map.clear();
        assert_eq!(map.get(&second), None);
        map.insert(second, 10);
        assert_eq!(map.get(&second), Some(&10));
    }
}
