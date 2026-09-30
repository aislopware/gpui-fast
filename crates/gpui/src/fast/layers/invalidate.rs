//! Telling scrolls and offset reads apart from other changes, and deciding
//! whether a frame only scrolled a layer (M4).
//!
//! A wheel scroll mutates a container's offset in place and notifies the view
//! that painted it, which looks like any other change. Here it is noted for
//! the container it moved ([`note_scrolled`]), and reads of an offset through
//! the scroll getters ([`note_offset_read`]) are recorded with what a view
//! read, so that a view whose output depends on an offset is told apart from
//! one that was only built again because it holds a scroll container.

#![allow(
    dead_code,
    reason = "the paint stream's hook around a scroll container's children calls policy::decide; remove once it is merged"
)]

use std::{cell::RefCell, ops::Range, rc::Rc};

use collections::{FxHashMap, FxHashSet};

use crate::fast::dependencies::{RenderDependencies, StateVersion};
use crate::fast::layers::record::LayerRecord;
use crate::{App, GlobalElementId, Interactivity, Window};

/// Where a scroll container's offset lives, which scrolls and reads of it are
/// noted under.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ScrollSource {
    /// Scroll state shared outside the element — a [`crate::ScrollHandle`], a list's
    /// state — by the address of its version counter.
    Handle(usize),
    /// A container's own element state, which nothing outside it reads.
    Container(GlobalElementId),
}

impl ScrollSource {
    /// The source of the shared scroll state `version` counts changes of.
    pub(crate) fn of_state(version: &StateVersion) -> Self {
        ScrollSource::Handle(version.id())
    }
}

/// A scroll container as its wheel listener knows it: its id and where its
/// offset lives. See [`painted_container`].
#[derive(Clone)]
pub(crate) struct ScrollContainer {
    id: GlobalElementId,
    source: ScrollSource,
    version: Option<StateVersion>,
}

/// The scrolls of the frame being drawn, and the scroll containers painted.
#[derive(Default)]
pub(crate) struct ScrollLog {
    /// The scroll containers a wheel scrolled since the last frame was drawn.
    pub(crate) scrolled: FxHashSet<GlobalElementId>,
    /// Where the offsets a wheel moved since the last frame live: those of
    /// `scrolled`, and of lists, which have no id.
    scrolled_sources: FxHashSet<ScrollSource>,
    /// The scroll containers painted lately, by id.
    containers: FxHashMap<GlobalElementId, PaintedContainer>,
}

/// What [`ScrollLog`] keeps of a scroll container it saw painted.
struct PaintedContainer {
    source: ScrollSource,
    /// Its shared scroll state's version, and the version it was at when it
    /// was painted, if it has shared state.
    version: Option<(StateVersion, u64)>,
    /// The frame it was last painted in.
    frame: u64,
}

/// How long a scroll container that is not painted is remembered, in frames:
/// one inside a view drawn again from last frame is not painted either.
const FORGET_AFTER_FRAMES: u64 = 120;

impl ScrollLog {
    /// The scroll containers painted lately.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn containers(&self) -> impl Iterator<Item = &GlobalElementId> {
        self.containers.keys()
    }

    /// Where the offset of the scroll container `id` lives, if it was
    /// painted lately.
    pub(crate) fn source(&self, id: &GlobalElementId) -> Option<ScrollSource> {
        self.containers
            .get(id)
            .map(|container| container.source.clone())
    }

    fn remember(&mut self, container: &ScrollContainer, frame: u64) {
        let version = container
            .version
            .as_ref()
            .map(|version| (version.clone(), version.get()));
        self.containers.insert(
            container.id.clone(),
            PaintedContainer {
                source: container.source.clone(),
                version,
                frame,
            },
        );
    }

    /// Ends the frame `frame`: the scrolls before it are taken in, and scroll
    /// containers neither painted lately nor holding a layer are forgotten.
    pub(crate) fn finish_frame(&mut self, frame: u64, keep: impl Fn(&GlobalElementId) -> bool) {
        self.scrolled.clear();
        self.scrolled_sources.clear();
        self.containers
            .retain(|id, container| container.frame + FORGET_AFTER_FRAMES > frame || keep(id));
    }
}

/// The scroll container whose scroll listener is being painted: the element
/// at the top of the element id stack, `element` its interactivity, with
/// the scroll handle it tracks, if any.
/// It is remembered as painted, for scrolls and reads of it to be told apart.
pub(crate) fn painted_container(window: &mut Window, element: &Interactivity) -> ScrollContainer {
    let id = crate::fast::global_id::current(window);
    let (source, version) = match element.tracked_scroll_handle.as_ref() {
        Some(handle) => {
            let version = handle.0.borrow().version.clone();
            (ScrollSource::of_state(&version), Some(version))
        }
        None => (ScrollSource::Container(id.clone()), None),
    };
    let container = ScrollContainer {
        id,
        source,
        version,
    };
    let frame = window.fast_layers.frame;
    window.fast_layers.scrolls.remember(&container, frame);
    container
}

/// Notes that a wheel moved `container`'s offset, as its scroll listener
/// does, for the next frame to tell the scroll apart from other changes.
pub(crate) fn note_scrolled(window: &mut Window, container: &ScrollContainer) {
    let scrolls = &mut window.fast_layers.scrolls;
    scrolls.scrolled.insert(container.id.clone());
    scrolls.scrolled_sources.insert(container.source.clone());
    if !scrolls.containers.contains_key(&container.id) {
        let frame = window.fast_layers.frame;
        window.fast_layers.scrolls.remember(container, frame);
    }
}

/// Notes that a wheel moved the list whose state `version` counts changes
/// of. A list has no id; its offset is known by its state.
pub(crate) fn note_list_scrolled(window: &mut Window, version: &StateVersion) {
    window
        .fast_layers
        .scrolls
        .scrolled_sources
        .insert(ScrollSource::of_state(version));
}

/// Whether the scroll container `id` scrolled since the last frame was
/// drawn: a wheel moved it, or its shared scroll state changed, as
/// [`crate::ScrollHandle::set_offset`] and the `scroll_to_…` methods change it.
pub(crate) fn scrolled(window: &Window, id: &GlobalElementId) -> bool {
    let scrolls = &window.fast_layers.scrolls;
    scrolls.scrolled.contains(id)
        || scrolls.containers.get(id).is_some_and(|container| {
            container
                .version
                .as_ref()
                .is_some_and(|(version, painted_at)| version.get() != *painted_at)
        })
}

/// The offsets read through the scroll getters while dependencies are being
/// recorded, with the version of the state each was read at.
///
/// The getters take no context to record into, so the log is kept by the
/// thread, which is the one thread windows are drawn on, and opened and
/// closed with the app's recordings. See
/// [`crate::App::begin_recording_dependencies`].
#[derive(Default)]
struct OffsetReadLog {
    recordings: usize,
    reads: Vec<(StateVersion, u64)>,
}

thread_local! {
    static OFFSET_READS: RefCell<OffsetReadLog> = RefCell::new(OffsetReadLog::default());
}

/// Records, for any recording that is open, that the offset of the scroll
/// state `version` counts changes of was read.
#[inline]
pub(crate) fn note_offset_read(version: &StateVersion) {
    OFFSET_READS.with_borrow_mut(|log| {
        if log.recordings > 0 {
            log.reads.push((version.clone(), version.get()));
        }
    });
}

/// Opens a recording of offset reads, returning where in the log it starts.
pub(crate) fn begin_offset_reads() -> usize {
    OFFSET_READS.with_borrow_mut(|log| {
        log.recordings += 1;
        log.reads.len()
    })
}

/// Where the offset read log ends.
pub(crate) fn offset_reads_len() -> usize {
    OFFSET_READS.with_borrow(|log| log.reads.len())
}

/// The offsets read in `range` of the log, and those read in it outside the
/// stretches in `nested`, which lie within it, in order.
pub(crate) fn offset_reads_in<'a>(
    range: &Range<usize>,
    nested: impl Iterator<Item = &'a Range<usize>>,
) -> (OffsetReads, OffsetReads) {
    if range.is_empty() {
        return (OffsetReads::default(), OffsetReads::default());
    }
    OFFSET_READS.with_borrow(|log| {
        let all = OffsetReads::of(&log.reads[range.clone()]);
        let mut own = Vec::new();
        let mut cursor = range.start;
        for nested in nested {
            if nested.start > cursor {
                own.extend_from_slice(&log.reads[cursor..nested.start]);
            }
            cursor = cursor.max(nested.end);
        }
        if range.end > cursor {
            own.extend_from_slice(&log.reads[cursor..range.end]);
        }
        (all, OffsetReads::of(&own))
    })
}

/// Closes the innermost recording of offset reads, emptying the log once
/// none is open.
pub(crate) fn end_offset_reads() {
    OFFSET_READS.with_borrow_mut(|log| {
        log.recordings = log.recordings.saturating_sub(1);
        if log.recordings == 0 {
            log.reads.clear();
        }
    });
}

/// Tells any recording that is open that `reads` were read again, as they
/// are when a subtree built from them is reused, returning the stretch of
/// the log they took up.
pub(crate) fn replay_offset_reads(reads: &OffsetReads) -> Range<usize> {
    OFFSET_READS.with_borrow_mut(|log| {
        let start = log.reads.len();
        if log.recordings > 0
            && let Some(reads) = &reads.0
        {
            log.reads.extend(reads.iter().cloned());
        }
        start..log.reads.len()
    })
}

/// The scroll offsets a retained subtree read, once each, at the earliest
/// version read. Most subtrees read none, which takes no allocation.
#[derive(Clone, Default)]
pub(crate) struct OffsetReads(Option<Rc<[(StateVersion, u64)]>>);

impl OffsetReads {
    fn of(reads: &[(StateVersion, u64)]) -> Self {
        if reads.is_empty() {
            return OffsetReads(None);
        }
        let mut unique: Vec<(StateVersion, u64)> = Vec::with_capacity(reads.len());
        for read in reads {
            if !unique
                .iter()
                .any(|(version, _)| version.id() == read.0.id())
            {
                unique.push(read.clone());
            }
        }
        OffsetReads(Some(unique.into()))
    }

    fn iter(&self) -> impl Iterator<Item = &(StateVersion, u64)> {
        self.0.iter().flat_map(|reads| reads.iter())
    }

    /// Both sets of reads.
    pub(crate) fn union(&self, other: &Self) -> Self {
        match (&self.0, &other.0) {
            (_, None) => self.clone(),
            (None, _) => other.clone(),
            (Some(a), Some(b)) => {
                let mut reads = a.to_vec();
                reads.extend(b.iter().cloned());
                OffsetReads::of(&reads)
            }
        }
    }
}

/// Whether `dependencies` include a read of the offset that lives at
/// `source`.
pub(crate) fn render_read_offset(dependencies: &RenderDependencies, source: &ScrollSource) -> bool {
    match source {
        ScrollSource::Handle(id) => dependencies
            .offset_reads
            .iter()
            .any(|(version, _)| version.id() == *id),
        ScrollSource::Container(_) => false,
    }
}

/// Whether an offset `dependencies` include a read of has moved since: a
/// wheel scrolled it since the last frame, or its shared state changed.
/// A view that read an offset is built again when it scrolls, as it would
/// be for any other state it read.
pub(crate) fn offset_read_changed(window: &Window, dependencies: &RenderDependencies) -> bool {
    let scrolled = &window.fast_layers.scrolls.scrolled_sources;
    dependencies.offset_reads.iter().any(|(version, read_at)| {
        version.get() != *read_at
            || (!scrolled.is_empty() && scrolled.contains(&ScrollSource::of_state(version)))
    })
}

/// `dependencies` without the version of the scroll state at `source`, if
/// they hold it: a scroll of the container is what a layer is composited
/// for, not a change.
fn without_scroll_state(
    dependencies: &RenderDependencies,
    source: Option<&ScrollSource>,
) -> Option<RenderDependencies> {
    let Some(ScrollSource::Handle(id)) = source else {
        return None;
    };
    if !dependencies
        .states
        .iter()
        .any(|(version, _)| version.id() == *id)
    {
        return None;
    }
    let states: Vec<_> = dependencies
        .states
        .iter()
        .filter(|(version, _)| version.id() != *id)
        .cloned()
        .collect();
    Some(RenderDependencies {
        states: states.into(),
        ..dependencies.clone()
    })
}

/// The view whose element is being prepainted: the one holding the scroll
/// container asking.
fn owner(window: &Window) -> Option<&GlobalElementId> {
    window.retained_state.subtree_stack.last()
}

/// Whether `dependencies`, the scroll state at `source` aside, changed since
/// they were recorded, or name an entity notified since the last frame
/// other than the view holding the container, whose notification
/// [`owner_notified_otherwise`] accounts for.
fn changed(
    window: &Window,
    cx: &App,
    dependencies: &RenderDependencies,
    source: Option<&ScrollSource>,
) -> bool {
    let trimmed = without_scroll_state(dependencies, source);
    let dependencies = trimmed.as_ref().unwrap_or(dependencies);
    let notified = &window.retained_state.notified_entities;
    let owner = owner(window).and_then(crate::fast::splice::view_entity);
    cx.dependencies_changed(dependencies, window.inside_notified_view())
        || offset_read_changed(window, dependencies)
        || (!notified.is_empty()
            && dependencies
                .entities
                .iter()
                .any(|entity| Some(*entity) != owner && notified.contains(entity)))
}

/// Whether the view holding the scroll container `id` was notified since
/// the last frame for anything other than a scroll of `id`: notified while
/// `id` did not scroll, as its wheel listener notifies it only when it does.
fn owner_notified_otherwise(window: &Window, id: &GlobalElementId) -> bool {
    owner(window)
        .and_then(crate::fast::splice::view_entity)
        .is_some_and(|owner| {
            window.retained_state.notified_entities.contains(&owner) && !scrolled(window, id)
        })
}

/// Whether the frame being drawn only scrolled the layer of the scroll
/// container `id`, whose content `record` holds (spec §6.4):
///
/// - nothing the content read changed — no view nested in it is notified or
///   read anything that changed — and no hover it was painted by did;
/// - the view holding the container is clean, or dirty only because `id`
///   scrolled: nothing it read itself changed, it was notified only by the
///   scroll, and its render did not read `id`'s offset.
///
/// What the container itself is painted with — its bounds, content mask,
/// text style, opacity — is checked by [`crate::fast::layers::policy::decide`].
pub(crate) fn scroll_only(
    window: &Window,
    cx: &App,
    id: &GlobalElementId,
    record: &LayerRecord,
) -> bool {
    let source = window.fast_layers.scrolls.source(id);
    !changed(window, cx, &record.dependencies, source.as_ref())
        && window.hovers_unchanged(&record.hovers)
        && owner_scrolled_only(window, cx, id, source.as_ref())
}

/// Whether the view holding the scroll container `id`, whose offset lives
/// at `source`, and the views drawn inside the container are unchanged
/// since the last frame, but for scrolls of `id` its render did not read.
fn owner_scrolled_only(
    window: &Window,
    cx: &App,
    id: &GlobalElementId,
    source: Option<&ScrollSource>,
) -> bool {
    if owner_notified_otherwise(window, id) {
        return false;
    }
    let Some(owner) = owner(window) else {
        return false;
    };
    let Some(index) = window.rendered_frame.retained.find(owner) else {
        // Not drawn last frame as a retained view: nothing tells what it read.
        return false;
    };
    let records = &window.rendered_frame.retained.records;
    let owner = &records[index];
    // A view drawn inside the content renders while the owner lays out,
    // before the content is recorded: what it read is its own record's.
    // Such records follow the owner's; one whose view is dirty, notified or
    // around a view that is, is built again.
    let nested = &records[index + 1..(index + 1 + owner.nested).min(records.len())];
    let content_view_dirty = !window.dirty_views.is_empty()
        && nested.iter().any(|record| {
            record.id.len() > id.len()
                && record.id.starts_with(id)
                && crate::fast::splice::view_entity(&record.id)
                    .is_some_and(|view| window.dirty_views.contains(&view))
        });
    !content_view_dirty
        && !source.is_some_and(|source| render_read_offset(&owner.own_dependencies, source))
        && !changed(window, cx, &owner.own_dependencies, source)
}

/// Whether what the content of the scroll container `id` is built from
/// changed since the last frame, as far as it can be told without a layer
/// recording the content: the container is on today's path, and a frame
/// that did not only scroll it keeps a demoted layer waiting.
pub(crate) fn changed_without_layer(window: &Window, cx: &App, id: &GlobalElementId) -> bool {
    let source = window.fast_layers.scrolls.source(id);
    !owner_scrolled_only(window, cx, id, source.as_ref())
}
