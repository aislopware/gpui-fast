//! gpui-fast's `TabStopMap`, which `gpui.rs` puts in place of upstream's
//! `tab_stop.rs`.
//!
//! Upstream keeps the tab order as a sum tree and inserts every focus handle
//! a frame paints into it as it is painted, so each tracked focus handle costs
//! a tree insert on every frame. The order is only ever read when the keyboard
//! moves the focus (`Window::focus_next`, `focus_prev`) or accessibility counts
//! the tab stops, far more rarely than frames are drawn. This map records what
//! was painted, and sorts it into the tab order the first time a frame's order
//! is asked for.
//!
//! The order is upstream's: by path (the tab indices of the groups a handle
//! was painted in, then its own), and among equal paths by the order they were
//! painted in. A focus handle painted more than once is found where it was
//! painted last.

use std::cell::OnceCell;

use collections::FxHashMap;

use crate::{FocusHandle, FocusId};

/// Represents a collection of focus handles using the tab-index APIs.
#[derive(Debug, Default)]
pub(crate) struct TabStopMap {
    pub(crate) insertion_history: Vec<TabStopOperation>,
    /// The tab order of `insertion_history`, sorted when first read.
    order: OnceCell<TabOrder>,
}

#[derive(Debug, Clone)]
pub enum TabStopOperation {
    Insert(FocusHandle),
    Group(TabIndex),
    GroupEnd,
}

type TabIndex = isize;

type TabStopPath = smallvec::SmallVec<[TabIndex; 6]>;

/// One focus handle painted, where it sorts in the tab order.
#[derive(Debug)]
struct TabStopNode {
    /// The tab indices of the groups it was painted in, then its own.
    path: TabStopPath,
    /// Its operation in `insertion_history`.
    node_insertion_index: usize,
    /// Whether this node is a tab stop.
    tab_stop: bool,
}

/// `insertion_history`'s focus handles in tab order.
#[derive(Debug, Default)]
struct TabOrder {
    nodes: Vec<TabStopNode>,
    /// Where in `nodes` each focus handle was painted last.
    by_id: FxHashMap<FocusId, usize>,
}

impl TabOrder {
    fn of(history: &[TabStopOperation]) -> Self {
        let mut path = TabStopPath::new();
        let mut nodes = Vec::new();
        for (node_insertion_index, operation) in history.iter().enumerate() {
            match operation {
                TabStopOperation::Insert(focus_handle) => {
                    let mut node_path = path.clone();
                    node_path.push(focus_handle.tab_index);
                    nodes.push(TabStopNode {
                        path: node_path,
                        node_insertion_index,
                        tab_stop: focus_handle.tab_stop,
                    });
                }
                TabStopOperation::Group(tab_index) => path.push(*tab_index),
                TabStopOperation::GroupEnd => {
                    path.pop();
                }
            }
        }
        // Painted in insertion order, so a stable sort by path keeps equal
        // paths in the order they were painted.
        nodes.sort_by(|a, b| a.path.cmp(&b.path));
        let mut by_id = FxHashMap::default();
        let mut last_painted = FxHashMap::<FocusId, usize>::default();
        for (position, node) in nodes.iter().enumerate() {
            let TabStopOperation::Insert(focus_handle) = &history[node.node_insertion_index] else {
                continue;
            };
            let painted = last_painted
                .entry(focus_handle.id)
                .or_insert(node.node_insertion_index);
            if node.node_insertion_index >= *painted {
                *painted = node.node_insertion_index;
                by_id.insert(focus_handle.id, position);
            }
        }
        Self { nodes, by_id }
    }

    /// The first tab stop after `position`, or before it when `forward` is
    /// false.
    fn step(&self, position: usize, forward: bool) -> Option<usize> {
        if forward {
            (position + 1..self.nodes.len()).find(|&next| self.nodes[next].tab_stop)
        } else {
            (0..position).rev().find(|&prev| self.nodes[prev].tab_stop)
        }
    }
}

impl TabStopMap {
    pub fn insert(&mut self, focus_handle: &FocusHandle) {
        self.order.take();
        self.insertion_history
            .push(TabStopOperation::Insert(focus_handle.clone()));
    }

    pub fn begin_group(&mut self, tab_index: isize) {
        self.order.take();
        self.insertion_history
            .push(TabStopOperation::Group(tab_index));
    }

    pub fn end_group(&mut self) {
        self.order.take();
        self.insertion_history.push(TabStopOperation::GroupEnd);
    }

    pub fn clear(&mut self) {
        self.order.take();
        self.insertion_history.clear();
    }

    fn order(&self) -> &TabOrder {
        self.order
            .get_or_init(|| TabOrder::of(&self.insertion_history))
    }

    pub fn next(&self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        self.step(focused_id, true)
    }

    pub fn prev(&self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        self.step(focused_id, false)
    }

    /// The tab stop after the focused handle, or before it; from the first
    /// (or last) one when nothing is focused, the focused handle was not
    /// painted, or it is the last (or first) tab stop.
    fn step(&self, focused_id: Option<&FocusId>, forward: bool) -> Option<FocusHandle> {
        let order = self.order();
        let from_focused = focused_id
            .and_then(|id| order.by_id.get(id))
            .and_then(|&position| order.step(position, forward));
        let position = match from_focused {
            Some(position) => position,
            None => {
                let end = if forward {
                    0
                } else {
                    order.nodes.len().checked_sub(1)?
                };
                let node = order.nodes.get(end)?;
                if node.tab_stop {
                    end
                } else {
                    order.step(end, forward)?
                }
            }
        };
        self.focus_handle_for_order(&order.nodes[position])
    }

    pub fn replay(&mut self, nodes: &[TabStopOperation]) {
        self.order.take();
        self.insertion_history.extend_from_slice(nodes);
    }

    pub fn paint_index(&self) -> usize {
        self.insertion_history.len()
    }

    pub(crate) fn tab_stop_count(&self) -> usize {
        let order = self.order();
        order
            .by_id
            .values()
            .filter(|&&position| order.nodes[position].tab_stop)
            .count()
    }

    fn focus_handle_for_order(&self, order: &TabStopNode) -> Option<FocusHandle> {
        match &self.insertion_history[order.node_insertion_index] {
            TabStopOperation::Insert(focus_handle) => Some(focus_handle.clone()),
            _ => {
                debug_assert!(
                    false,
                    "The order node did not correspond to an element, this is a GPUI bug"
                );
                None
            }
        }
    }
}
