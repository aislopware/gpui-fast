//! Checking that what a frame drew again from the last frame is what
//! building it afresh draws: Chromium's under-invalidation check
//! (`PaintController`'s `checkUnderInvalidation`) for gpui-fast's retained
//! drawing.
//!
//! A frame draws a view again from what it drew last frame while nothing the
//! view read has changed ([`crate::fast::retained`]), copies a stretch an
//! element painted under a key while the key is the same
//! ([`crate::fast::keyed`]), and composites a scroll layer's tiles, or keeps a
//! list's rows in its layer, while only the scroll changed
//! ([`crate::fast::layers`]). Each is sound only while what it stands for
//! would be built the same way: a view that read something it is not told
//! of, a `Cell` or the wall clock, shows what it showed before.
//!
//! With the check on ([`Window::set_check_retained`], or
//! `GPUI_CHECK_RETAINED=panic` or `=record` in the environment), every frame
//! that drew anything again is drawn a second time right after it, from
//! scratch, at the same moment ([`Window::repeat_frame_time`]), before any
//! effect runs or any event comes, so both see the same state and every
//! animation stands where it stood. What the two paint is compared, each
//! composited layer counted as the content its tiles were rasterized from.
//! Where they differ inside something the first frame drew again — the
//! innermost view drawn from last frame around what differs, a stretch
//! drawn again under a key, a layer part kept from the last paint — the
//! check reports it: the view's type, its entity, its element id path, and
//! the primitives each frame painted there. Differences in what the first
//! frame built are not judged: a build reads what the frame before it wrote
//! as it drew (a height measured while it was laid out, a hover found once
//! its hitboxes were in), which a frame from scratch after it reads anew,
//! and the frame that wrote it asks for the next one.
//!
//! A frame can find too late that it drew something again it should have
//! built: one that asks for the next frame (a hover over a view drawn
//! again, the focus moving while it drew), and a place that read what the
//! frame's own drawing wrote after it (a row rendering widens the column the
//! rows before it were drawn at, as upstream draws them that frame too). What
//! differs there waits for the next frame, and is reported only if that frame
//! draws the place again and it still differs.
//!
//! The frame from scratch is drawn in a shadow and leaves no trace: the
//! window shows the frame it drew, as it would with the check off, and every
//! frame after is drawn from that one. A checked app is the app unchecked,
//! frame for frame, so what an end-to-end test sees and compares holds with
//! the check on. The frame from scratch builds every view again (their
//! `render`s run) and lays them out on a layout engine of its own, with
//! scroll layers off, and is never presented; then everything drawing it
//! changed is put back: the element states it moved on are handed back to
//! the frame shown, the window's state of the frame shown (its hit test,
//! focus, layers, retained bookkeeping, accessibility tree and invalidation)
//! is restored, the effects, next-frame callbacks and change stamps its
//! views made are dropped, and the state of every `list`, scroll handle and
//! versioned state its views scrolled or bumped is set back to where the
//! frame shown left it. What a view's own `render` does to its own state
//! beyond notifying, a model it mutates in place, stays: no framework can
//! take that back.

use crate::{App, ArenaClearNeeded, Window};
#[cfg(any(test, feature = "test-support"))]
use crate::{
    Bounds, EntityId, GlobalElementId, LayerKey, ScaledPixels, Scene,
    fast::layers::verify::{Source, Sourced},
};
#[cfg(any(test, feature = "test-support"))]
use collections::{FxHashMap, FxHashSet};
#[cfg(any(test, feature = "test-support"))]
use std::{fmt::Write as _, ops::Range, rc::Rc};

/// What a window does with a frame that drew something again from the last
/// frame that a frame built afresh at the same moment draws otherwise. See
/// [`crate::fast::check_retained`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedCheck {
    /// Panics with the report, for tests.
    Panic,
    /// Logs the report as an error and keeps it, for
    /// [`Window::take_retained_check_reports`].
    Record,
}

/// A window's state for [`RetainedCheck`].
#[derive(Default)]
pub(crate) struct CheckState {
    #[cfg(any(test, feature = "test-support"))]
    mode: Option<RetainedCheck>,
    /// Whether the frame being drawn is the one from scratch.
    #[cfg(any(test, feature = "test-support"))]
    scratch: bool,
    #[cfg(any(test, feature = "test-support"))]
    reports: Vec<String>,
    /// The type of each view drawn, by its entity, for the reports.
    #[cfg(any(test, feature = "test-support"))]
    view_types: FxHashMap<EntityId, &'static str>,
    /// The scenes of every layer part the last frame painted: a part the
    /// frame being drawn composites that is one of them was kept.
    #[cfg(any(test, feature = "test-support"))]
    painted_parts: Vec<Rc<Scene>>,
    /// The stretches of paint operations the frame being drawn drew again
    /// under keys, with their keys.
    #[cfg(any(test, feature = "test-support"))]
    keyed: Vec<(Range<usize>, u64)>,
    /// The stretches of paint operations the frame being drawn replayed a
    /// list layer's rows into, each with its layer and the row's scene.
    #[cfg(any(test, feature = "test-support"))]
    replayed: Vec<(Range<usize>, LayerKey, *const Scene)>,
    /// What the last frame checked drew again otherwise, while it asked for
    /// the next frame: reported if that frame draws it again too.
    #[cfg(any(test, feature = "test-support"))]
    pending: Vec<Found>,
    /// The state of every `list` the window laid out while checked. A view
    /// may scroll its list as it renders, before the list is laid out, so
    /// the frame from scratch keeps them all as the frame shown left them,
    /// to put them back once drawn.
    #[cfg(any(test, feature = "test-support"))]
    lists: Vec<std::rc::Weak<std::cell::RefCell<crate::StateInner>>>,
    /// Where the writes made while the window drew stood as the last frame
    /// checked ended: those past it were made while drawing this one.
    #[cfg(any(test, feature = "test-support"))]
    writes_seen: u64,
}

impl CheckState {
    /// The check as the environment asks for it: `GPUI_CHECK_RETAINED` set
    /// to `panic` or `record`.
    pub(crate) fn new() -> Self {
        #[cfg(any(test, feature = "test-support"))]
        {
            let mode = match std::env::var("GPUI_CHECK_RETAINED").as_deref() {
                Ok("panic") => Some(RetainedCheck::Panic),
                Ok("record") | Ok("1") => Some(RetainedCheck::Record),
                _ => None,
            };
            Self {
                mode,
                ..Self::default()
            }
        }
        #[cfg(not(any(test, feature = "test-support")))]
        Self::default()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Window {
    /// Checks every frame that draws something again from the last frame
    /// against the same frame drawn from scratch at the same moment, and
    /// does `check` with what differs; `None` turns the check off. See
    /// [`crate::fast::check_retained`].
    pub fn set_check_retained(&mut self, check: Option<RetainedCheck>) {
        self.retained_state.check.mode = check;
    }

    /// What [`Window::set_check_retained`] asked for.
    pub fn check_retained(&self) -> Option<RetainedCheck> {
        self.retained_state.check.mode
    }

    /// The reports [`RetainedCheck::Record`] kept since this was last asked.
    pub fn take_retained_check_reports(&mut self) -> Vec<String> {
        std::mem::take(&mut self.retained_state.check.reports)
    }
}

/// Whether the frame being drawn is the check's frame from scratch.
#[inline(always)]
pub(crate) fn scratch(window: &Window) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    return window.retained_state.check.scratch;
    #[cfg(not(any(test, feature = "test-support")))]
    {
        let _ = &window.retained_state.check;
        false
    }
}

/// Notes that the frame being drawn lays out the `list` of `state`, for
/// the check's frame from scratch to put it back as the frame shown left it.
#[inline(always)]
pub(crate) fn note_list(window: &mut Window, state: &crate::ListState) {
    #[cfg(any(test, feature = "test-support"))]
    if window.retained_state.check.mode.is_some() {
        let lists = &mut window.retained_state.check.lists;
        if !lists
            .iter()
            .any(|kept| std::ptr::eq(kept.as_ptr(), Rc::as_ptr(&state.0)))
        {
            lists.retain(|kept| kept.strong_count() > 0);
            lists.push(Rc::downgrade(&state.0));
        }
    }
    #[cfg(not(any(test, feature = "test-support")))]
    let _ = (window, state);
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// The scroll handles any window laid out a container for, which a view
    /// may scroll as it renders: the check's frame from scratch keeps them as
    /// the frame shown left them, to put them back once drawn.
    static SCROLL_HANDLES: std::cell::RefCell<Vec<std::rc::Weak<std::cell::RefCell<crate::ScrollHandleState>>>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// While the check's frame from scratch is drawn, each versioned state it
    /// changed, with the version it had before: put back once it is drawn.
    static VERSIONS: std::cell::RefCell<Option<Vec<(crate::fast::dependencies::StateVersion, u64)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Notes that a container was laid out for the scroll handle `handle`. See
/// [`SCROLL_HANDLES`].
#[inline(always)]
pub(crate) fn note_scroll_handle(handle: &crate::ScrollHandle) {
    #[cfg(any(test, feature = "test-support"))]
    SCROLL_HANDLES.with_borrow_mut(|handles| {
        if !handles
            .iter()
            .any(|kept| std::ptr::eq(kept.as_ptr(), Rc::as_ptr(&handle.0)))
        {
            handles.retain(|kept| kept.strong_count() > 0);
            handles.push(Rc::downgrade(&handle.0));
        }
    });
    #[cfg(not(any(test, feature = "test-support")))]
    let _ = handle;
}

/// Notes that the state `version` counts changes of is about to change: the
/// check's frame from scratch puts its version back. See [`VERSIONS`].
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn note_version_bump(version: &crate::fast::dependencies::StateVersion) {
    VERSIONS.with_borrow_mut(|versions| {
        if let Some(versions) = versions
            && !versions.iter().any(|(kept, _)| kept.same(version))
        {
            versions.push((version.clone(), version.get()));
        }
    });
}

/// Notes the type of the view of `entity` as it is laid out.
#[inline(always)]
pub(crate) fn note_view(window: &mut Window, entity: crate::EntityId, name: &'static str) {
    #[cfg(any(test, feature = "test-support"))]
    if window.retained_state.check.mode.is_some() {
        window.retained_state.check.view_types.insert(entity, name);
    }
    #[cfg(not(any(test, feature = "test-support")))]
    let _ = (window, entity, name);
}

/// Notes that the stretch under `key` was drawn again, from paint
/// operation `start` on to the last one.
#[inline(always)]
pub(crate) fn note_keyed(window: &mut Window, key: u64, start: usize) {
    #[cfg(any(test, feature = "test-support"))]
    if window.retained_state.check.mode.is_some() && !window.retained_state.check.scratch {
        let end = window.next_frame.scene.paint_operations.len();
        window.retained_state.check.keyed.push((start..end, key));
    }
    #[cfg(not(any(test, feature = "test-support")))]
    let _ = (window, key, start);
}

/// Replays the rows of the list layer `key` holds over `in_content` into
/// the frame, moved by `translation`, one row at a time, noting where each
/// went, when the check is on: returns whether it did.
pub(crate) fn replay_rows(
    window: &mut Window,
    key: crate::LayerKey,
    content: &crate::LayerContent,
    in_content: crate::Bounds<crate::ScaledPixels>,
    rows_over: crate::Bounds<crate::ScaledPixels>,
    translation: crate::Point<crate::ScaledPixels>,
) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if window.retained_state.check.mode.is_some() && !window.retained_state.check.scratch {
        for part in content.part_scenes_over(in_content, rows_over) {
            let start = window.next_frame.scene.paint_operations.len();
            crate::fast::layers::paint::draw_into_frame(
                window,
                crate::fast::scene::operations(part),
                translation,
            );
            let end = window.next_frame.scene.paint_operations.len();
            window
                .retained_state
                .check
                .replayed
                .push((start..end, key, Rc::as_ptr(part)));
        }
        return true;
    }
    let _ = (window, key, content, in_content, rows_over, translation);
    false
}

/// Ends a draw that `drawn` owes the arena of, checking it when the check
/// is on: returns what the frame shown owes.
#[inline(always)]
pub(crate) fn after_draw(
    window: &mut Window,
    cx: &mut App,
    drawn: ArenaClearNeeded,
) -> ArenaClearNeeded {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(mode) = window.retained_state.check.mode
        && !window.retained_state.check.scratch
    {
        return check(window, cx, mode, drawn);
    }
    let _ = cx;
    let _ = &window;
    drawn
}

/// How many primitives a report lists for each place, of each frame.
#[cfg(any(test, feature = "test-support"))]
const LISTED: usize = 6;

#[cfg(any(test, feature = "test-support"))]
fn check(
    window: &mut Window,
    cx: &mut App,
    mode: RetainedCheck,
    drawn: ArenaClearNeeded,
) -> ArenaClearNeeded {
    // What was written while this frame was drawn: views, keyed stretches
    // and layers drawn again that read it may have read it before it was.
    let writes_seen = std::mem::replace(
        &mut window.retained_state.check.writes_seen,
        cx.entities.write_generation(),
    );
    let written = cx.entities.written_after(writes_seen);
    let keyed = std::mem::take(&mut window.retained_state.check.keyed);
    let replayed = std::mem::take(&mut window.retained_state.check.replayed);
    let painted_before = std::mem::take(&mut window.retained_state.check.painted_parts);
    let pending = std::mem::take(&mut window.retained_state.check.pending);
    let shown = Shown::capture(window, &painted_before, keyed, replayed);
    drop(painted_before);
    let Some(shown) = shown else {
        // Nothing was drawn again: what the last frame found is built.
        window.retained_state.check.painted_parts = layer_parts(window);
        return drawn;
    };
    drawn.clear(cx);

    let (fresh, drawn) = draw_from_scratch(window, cx);
    window.retained_state.check.painted_parts = layer_parts(window);
    let found = shown.judge(fresh, &window.retained_state.check.view_types);
    // A frame may find too late what it drew again, and build it on the
    // next: one that asked for the next frame (a hover over a view drawn
    // again, the focus moving while it drew), and a place that read what
    // was written while the frame was drawn, after it read it, as a row
    // rendering widens a column the rows before it were drawn at. What it
    // found there waits for the next frame.
    let asked_for_next = !window.next_frame_callbacks.borrow().is_empty()
        || window.invalidator.is_dirty()
        || !window.retained_state.dirty_subtrees.is_empty();
    let (mut now, mut later): (Vec<Found>, Vec<Found>) = found
        .into_iter()
        .partition(|found| !asked_for_next && !read_written(window, &found.place, &written));
    // What the last frame found to wait for, this one draws again, rather
    // than building it, and still otherwise: it was never going to be.
    let confirmed: Vec<Found> = pending
        .into_iter()
        .filter(|found| {
            shown.draws_again(&found.place)
                && now
                    .iter()
                    .chain(&later)
                    .any(|again| again.place == found.place)
        })
        .collect();
    let unconfirmed = |found: &Found| !confirmed.iter().any(|again| again.place == found.place);
    now.retain(unconfirmed);
    later.retain(unconfirmed);
    window.retained_state.check.pending = later;
    report(window, mode, confirmed, true);
    report(window, mode, now, false);
    drawn
}

/// Whether what was drawn again at `place` read one of the entities
/// `written` while the frame was drawn: views and layers record what they
/// read, keyed stretches do not.
#[cfg(any(test, feature = "test-support"))]
fn read_written(
    window: &Window,
    place: &PlaceKey,
    written: &collections::FxHashSet<EntityId>,
) -> bool {
    if written.is_empty() {
        return false;
    }
    let was_written = |entity: &EntityId| written.contains(entity);
    match place {
        PlaceKey::View(id) => {
            let retained = &window.rendered_frame.retained;
            retained.find(id).is_some_and(|index| {
                retained.records[index]
                    .dependencies
                    .entities
                    .iter()
                    .any(was_written)
            })
        }
        PlaceKey::Layer(id) => window.fast_layers.layers.get(id).is_some_and(|layer| {
            layer
                .record
                .as_ref()
                .is_some_and(|record| record.dependencies.entities.iter().any(was_written))
                || crate::fast::layers::lists::a_held_row_read(&layer.rows, was_written)
        }),
        PlaceKey::Keyed(_) => false,
    }
}

/// Draws the frame `window` shows again from scratch, at its moment, and
/// returns what it painted, leaving the window, the frame it shows and the
/// app as they were (see [`crate::fast::check_retained`]), with what the
/// draw owes the arena.
#[cfg(any(test, feature = "test-support"))]
fn draw_from_scratch(window: &mut Window, cx: &mut App) -> (Vec<Sourced>, ArenaClearNeeded) {
    let shadow = Shadow::enter(window, cx);
    let arena = crate::ElementArenaScope::enter(&cx.element_arena);
    window.repeat_frame_time();
    crate::fast::frame_time::begin_frame(window, cx);
    window.refreshing = true;
    window.draw_roots(cx);
    window.refreshing = false;
    crate::fast::frame_time::end_frame();
    // The fades its primitives index: a frame shown gets them as it ends.
    crate::fast::edge_fade::copy_table(window);
    let fresh = crate::fast::layers::verify::sourced(&window.next_frame.scene);
    // What the elements kept from one frame to the next, the frame from
    // scratch took from the frame shown as it drew them: they go back.
    let states = std::mem::take(&mut window.next_frame.element_states);
    window.rendered_frame.element_states.extend(states);
    window.next_frame.clear();
    shadow.leave(window, cx);
    (fresh, arena.exit(&cx.element_arena))
}

/// What drawing a frame changes besides the frame it draws, as it stood
/// before the check's frame from scratch was drawn, or set aside for it.
#[cfg(any(test, feature = "test-support"))]
struct Shadow {
    /// The window's retained bookkeeping: the frame from scratch keeps its
    /// own, which notes nothing for the next frame.
    retained: crate::fast::retained::RetainedState,
    /// The window's scroll layers: the frame from scratch draws with none,
    /// every container painting its content directly.
    layers: crate::fast::layers::WindowLayers,
    /// The layout engine and what matches elements to its nodes: the frame
    /// from scratch lays out on nodes of its own, so the nodes the frame
    /// shown keeps for the next one stay as they were.
    layout_engine: Option<crate::TaffyLayoutEngine>,
    fast_layout: crate::fast::layout_key::WindowLayout,
    /// The accessibility tree: the frame from scratch builds none and sends
    /// the platform nothing.
    a11y: crate::window::a11y::A11y,
    focus_tooltip: crate::fast::focus_tooltip::WindowFocusTooltip,
    tooltip_bounds: Option<crate::TooltipBounds>,
    mouse_hit_test: crate::HitTest,
    requested_autoscroll: Option<Bounds<crate::Pixels>>,
    /// The focus, which a view may move as it renders.
    focus: Option<crate::FocusId>,
    focus_generation: u64,
    dirty_views: FxHashSet<EntityId>,
    /// The views notified for the next frame, and whether the window asked
    /// for one.
    invalidated: FxHashSet<EntityId>,
    dirty: bool,
    next_frame_callbacks: usize,
    pending_effects: usize,
    pending_notifications: FxHashSet<EntityId>,
    pending_global_notifications: collections::TypeIdHashSet,
    accessed_entities: FxHashSet<EntityId>,
    stamps: crate::fast::dependencies::ChangeStamps,
    thread: crate::fast::layers::invalidate::ThreadNotes,
    lists: Vec<ListSnapshot>,
    scroll_handles: Vec<ScrollHandleSnapshot>,
}

#[cfg(any(test, feature = "test-support"))]
impl Shadow {
    /// Sets aside what drawing the frame from scratch would change.
    fn enter(window: &mut Window, cx: &mut App) -> Self {
        let mut retained = crate::fast::retained::RetainedState::new(cx);
        retained.view_retention = window.retained_state.view_retention;
        retained.element_retention = window.retained_state.element_retention;
        retained.font_generation = window.retained_state.font_generation;
        retained.check.scratch = true;
        let lists = window
            .retained_state
            .check
            .lists
            .iter()
            .filter_map(|list| list.upgrade())
            .map(ListSnapshot::take)
            .collect();
        let layers = crate::fast::layers::WindowLayers {
            enabled: false,
            frame: window.fast_layers.frame,
            window_size: window.fast_layers.window_size,
            hover_freeze: window.fast_layers.hover_freeze.clone(),
            ..Default::default()
        };
        VERSIONS.set(Some(Vec::new()));
        let a11y = crate::window::a11y::A11y::new(
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            true,
            None,
        );
        Shadow {
            retained: std::mem::replace(&mut window.retained_state, retained),
            layers: std::mem::replace(&mut window.fast_layers, layers),
            layout_engine: window
                .layout_engine
                .replace(crate::TaffyLayoutEngine::new()),
            fast_layout: std::mem::take(&mut window.fast_layout),
            a11y: std::mem::replace(&mut window.a11y, a11y),
            focus_tooltip: window.fast_focus_tooltip.clone(),
            tooltip_bounds: window.tooltip_bounds.take(),
            mouse_hit_test: std::mem::take(&mut window.mouse_hit_test),
            requested_autoscroll: window.requested_autoscroll,
            focus: window.focus,
            focus_generation: window.focus_generation,
            dirty_views: std::mem::take(&mut window.dirty_views),
            invalidated: window.invalidator.take_views(),
            dirty: window.invalidator.is_dirty(),
            next_frame_callbacks: window.next_frame_callbacks.borrow().len(),
            pending_effects: cx.pending_effects.len(),
            pending_notifications: cx.pending_notifications.clone(),
            pending_global_notifications: cx.pending_global_notifications.clone(),
            accessed_entities: std::mem::take(cx.entities.accessed_entities.get_mut()),
            stamps: crate::fast::dependencies::ChangeStamps::take(cx),
            thread: crate::fast::layers::invalidate::ThreadNotes::take(),
            lists,
            scroll_handles: SCROLL_HANDLES.with_borrow(|handles| {
                handles
                    .iter()
                    .filter_map(|handle| handle.upgrade())
                    .map(ScrollHandleSnapshot::take)
                    .collect()
            }),
        }
    }

    /// Puts back what [`Shadow::enter`] set aside, dropping what drawing the
    /// frame from scratch made.
    fn leave(self, window: &mut Window, cx: &mut App) {
        window.retained_state = self.retained;
        for list in self.lists {
            list.restore();
        }
        for handle in self.scroll_handles {
            handle.restore();
        }
        for (version, at) in VERSIONS.take().unwrap_or_default() {
            version.set(at);
        }
        window.fast_layers = self.layers;
        window.layout_engine = self.layout_engine;
        window.fast_layout = self.fast_layout;
        window.a11y = self.a11y;
        window.fast_focus_tooltip = self.focus_tooltip;
        window.tooltip_bounds = self.tooltip_bounds;
        window.mouse_hit_test = self.mouse_hit_test;
        window.requested_autoscroll = self.requested_autoscroll;
        window.focus = self.focus;
        window.focus_generation = self.focus_generation;
        window.dirty_views = self.dirty_views;
        window.invalidator.replace_views(self.invalidated);
        if !self.dirty {
            window.invalidator.set_dirty(false);
        }
        window.invalidator.set_phase(crate::window::DrawPhase::None);
        window
            .next_frame_callbacks
            .borrow_mut()
            .truncate(self.next_frame_callbacks);
        cx.pending_effects.truncate(self.pending_effects);
        cx.pending_notifications = self.pending_notifications;
        cx.pending_global_notifications = self.pending_global_notifications;
        *cx.entities.accessed_entities.get_mut() = self.accessed_entities;
        self.stamps.restore(cx);
        self.thread.restore();
    }
}

/// A scroll handle's state as the frame shown left it. See
/// [`SCROLL_HANDLES`].
#[cfg(any(test, feature = "test-support"))]
struct ScrollHandleSnapshot {
    state: Rc<std::cell::RefCell<crate::ScrollHandleState>>,
    offset: crate::Point<crate::Pixels>,
    bounds: Bounds<crate::Pixels>,
    max_offset: crate::Point<crate::Pixels>,
    scroll_to_bottom: bool,
    overflow: crate::Point<crate::Overflow>,
}

#[cfg(any(test, feature = "test-support"))]
impl ScrollHandleSnapshot {
    fn take(handle: Rc<std::cell::RefCell<crate::ScrollHandleState>>) -> Self {
        let state = handle.borrow();
        let snapshot = ScrollHandleSnapshot {
            state: handle.clone(),
            offset: *state.offset.borrow(),
            bounds: state.bounds,
            max_offset: state.max_offset,
            scroll_to_bottom: state.scroll_to_bottom,
            overflow: state.overflow,
        };
        drop(state);
        snapshot
    }

    fn restore(self) {
        let mut state = self.state.borrow_mut();
        *state.offset.borrow_mut() = self.offset;
        state.bounds = self.bounds;
        state.max_offset = self.max_offset;
        state.scroll_to_bottom = self.scroll_to_bottom;
        state.overflow = self.overflow;
    }
}

/// A `list`'s state as the frame shown left it. See [`CheckState::lists`].
#[cfg(any(test, feature = "test-support"))]
struct ListSnapshot {
    state: Rc<std::cell::RefCell<crate::StateInner>>,
    last_layout_bounds: Option<Bounds<crate::Pixels>>,
    last_padding: Option<crate::Edges<crate::Pixels>>,
    items: sum_tree::SumTree<crate::ListItem>,
    logical_scroll_top: Option<crate::ListOffset>,
    pending_scroll: Option<crate::PendingScroll>,
    follow_state: crate::FollowState,
    version: u64,
    tail: crate::fast::list_tail::TailReservation,
}

#[cfg(any(test, feature = "test-support"))]
impl ListSnapshot {
    fn take(list: Rc<std::cell::RefCell<crate::StateInner>>) -> Self {
        let state = list.borrow();
        let snapshot = ListSnapshot {
            state: list.clone(),
            last_layout_bounds: state.last_layout_bounds,
            last_padding: state.last_padding,
            items: state.items.clone(),
            logical_scroll_top: state.logical_scroll_top,
            pending_scroll: state.pending_scroll.clone(),
            follow_state: state.follow_state,
            version: state.version.get(),
            tail: state.fast_tail,
        };
        drop(state);
        snapshot
    }

    fn restore(self) {
        let mut state = self.state.borrow_mut();
        state.last_layout_bounds = self.last_layout_bounds;
        state.last_padding = self.last_padding;
        state.items = self.items;
        state.logical_scroll_top = self.logical_scroll_top;
        state.pending_scroll = self.pending_scroll;
        state.follow_state = self.follow_state;
        state.version.set(self.version);
        state.fast_tail = self.tail;
    }
}

/// Reports `found`, if anything was, as `mode` asks: `again` for what the
/// frame before found and this one drew again.
#[cfg(any(test, feature = "test-support"))]
fn report(window: &mut Window, mode: RetainedCheck, found: Vec<Found>, again: bool) {
    if found.is_empty() {
        return;
    }
    let mut report = format!(
        "a frame drew again from the last frame what a frame built afresh at the same moment \
         draws otherwise, in {} place{}{}:",
        found.len(),
        if found.len() == 1 { "" } else { "s" },
        if again {
            ", and the frame it asked for drew it again too"
        } else {
            ""
        }
    );
    for found in &found {
        report.push('\n');
        report.push_str(&found.text);
    }
    match mode {
        RetainedCheck::Panic => panic!("{report}"),
        RetainedCheck::Record => {
            log::error!("{report}");
            window.retained_state.check.reports.push(report);
        }
    }
}

/// The scenes of every part of the content the layers of `window` hold.
#[cfg(any(test, feature = "test-support"))]
fn layer_parts(window: &Window) -> Vec<Rc<Scene>> {
    window
        .fast_layers
        .layers
        .values()
        .filter_map(|layer| layer.record.as_ref())
        .flat_map(|record| record.content.part_scenes().cloned())
        .collect()
}

/// A view the frame shown drew: where its paint operations lie, whether it
/// was drawn from last frame, and where it was drawn.
#[cfg(any(test, feature = "test-support"))]
struct ShownView {
    id: GlobalElementId,
    operations: Range<usize>,
    drawn_again: bool,
    reach: Bounds<ScaledPixels>,
}

/// A layer the frame shown composited a part of as the last frame painted
/// it.
#[cfg(any(test, feature = "test-support"))]
struct KeptLayer {
    key: LayerKey,
    container: GlobalElementId,
    viewport: Bounds<ScaledPixels>,
}

/// What the check keeps of the frame shown while it draws one from scratch.
#[cfg(any(test, feature = "test-support"))]
struct Shown {
    painted: Vec<Sourced>,
    views: Vec<ShownView>,
    /// Views drawn again whose hovers changed under them as the frame was
    /// painted, too late to build them, which the next frame builds.
    late: FxHashSet<GlobalElementId>,
    keyed: Vec<(Range<usize>, u64)>,
    /// The rows replayed from list layers that the last frame painted:
    /// where each went, and its layer's place in `layers`.
    kept_rows: Vec<(Range<usize>, usize)>,
    kept_parts: FxHashSet<*const Scene>,
    layers: Vec<KeptLayer>,
}

/// A place a frame drew again otherwise than a frame built afresh draws it,
/// and the report's lines about it.
#[cfg(any(test, feature = "test-support"))]
pub(crate) struct Found {
    place: PlaceKey,
    text: String,
}

/// A place drawn again, named so that the next frame can find it.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, PartialEq, Eq)]
enum PlaceKey {
    View(GlobalElementId),
    Keyed(u64),
    Layer(GlobalElementId),
}

/// What a primitive one frame painted and the other did not lies in.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Place {
    View(usize),
    Keyed(usize),
    Layer(usize),
}

#[cfg(any(test, feature = "test-support"))]
impl Shown {
    /// What the check needs of the frame `window` shows, or `None` when it
    /// drew nothing again.
    fn capture(
        window: &Window,
        painted_before: &[Rc<Scene>],
        keyed: Vec<(Range<usize>, u64)>,
        replayed: Vec<(Range<usize>, LayerKey, *const Scene)>,
    ) -> Option<Self> {
        let frame = &window.rendered_frame;
        let before: FxHashSet<*const Scene> = painted_before
            .iter()
            .map(|scene| Rc::as_ptr(scene))
            .collect();
        let mut kept_parts = FxHashSet::default();
        let mut kept_keys = FxHashSet::default();
        for layer in &frame.scene.layers.frames {
            for part in layer.content.part_scenes() {
                if before.contains(&Rc::as_ptr(part)) {
                    kept_parts.insert(Rc::as_ptr(part));
                    kept_keys.insert(layer.key);
                }
            }
        }
        let replayed: Vec<_> = replayed
            .into_iter()
            .filter(|(_, _, part)| before.contains(part))
            .collect();
        kept_keys.extend(replayed.iter().map(|(_, key, _)| *key));
        let painted = |record: &crate::fast::retained::RetainedSubtree| {
            matches!(
                record.paint,
                crate::fast::retained::PaintStatus::Painted { .. }
            )
        };
        let drew_again = frame
            .retained
            .records
            .iter()
            .any(|record| record.drawn_again && painted(record));
        if !drew_again && kept_parts.is_empty() && replayed.is_empty() && keyed.is_empty() {
            return None;
        }
        let scale = window.scale_factor();
        let views = frame
            .retained
            .records
            .iter()
            .filter(|record| painted(record))
            .map(|record| ShownView {
                id: record.id.clone(),
                operations: record.paint_range.start.scene_index
                    ..record.paint_range.end.scene_index,
                drawn_again: record.drawn_again,
                reach: record
                    .context
                    .bounds
                    .intersect(&record.context.content_mask.bounds)
                    .scale(scale),
            })
            .collect();
        let layers = window
            .fast_layers
            .layers
            .iter()
            .filter(|(_, layer)| kept_keys.contains(&layer.key))
            .filter_map(|(id, layer)| {
                Some(KeptLayer {
                    key: layer.key,
                    container: id.clone(),
                    viewport: layer.record.as_ref()?.viewport.scale(scale),
                })
            })
            .collect();
        let layers: Vec<KeptLayer> = layers;
        let kept_rows = replayed
            .into_iter()
            .filter_map(|(range, key, _)| {
                Some((range, layers.iter().position(|layer| layer.key == key)?))
            })
            .collect();
        Some(Shown {
            painted: crate::fast::layers::verify::sourced(&frame.scene),
            views,
            late: window.retained_state.dirty_subtrees.clone(),
            keyed,
            kept_rows,
            kept_parts,
            layers,
        })
    }

    /// The innermost view whose paint holds operation `index`.
    fn view_painting(&self, index: usize) -> Option<usize> {
        self.views
            .iter()
            .enumerate()
            .filter(|(_, view)| view.operations.contains(&index))
            .min_by_key(|(at, view)| (view.operations.len(), usize::MAX - at))
            .map(|(at, _)| at)
    }

    /// The innermost view or kept layer drawn over `reach`'s centre.
    fn place_over(&self, reach: &Bounds<ScaledPixels>) -> Option<Place> {
        let centre = reach.center();
        let area = |bounds: &Bounds<ScaledPixels>| bounds.size.width.0 * bounds.size.height.0;
        let views = self
            .views
            .iter()
            .enumerate()
            .filter(|(_, view)| view.reach.contains(&centre))
            .map(|(at, view)| (area(&view.reach), usize::MAX - at, Place::View(at)));
        let layers = self
            .layers
            .iter()
            .enumerate()
            .filter(|(_, layer)| layer.viewport.contains(&centre))
            .map(|(at, layer)| (area(&layer.viewport), 0, Place::Layer(at)));
        views
            .chain(layers)
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(.., place)| place)
    }

    /// Where the frame shown drew `painted` from: the place it was drawn
    /// again in, if it was.
    fn source_place(&self, painted: &Sourced) -> Option<Place> {
        match painted.source {
            Source::Operation(index) => {
                if let Some(at) = self
                    .keyed
                    .iter()
                    .position(|(range, _)| range.contains(&index))
                {
                    return Some(Place::Keyed(at));
                }
                if let Some((_, layer)) = self
                    .kept_rows
                    .iter()
                    .find(|(range, _)| range.contains(&index))
                {
                    return Some(Place::Layer(*layer));
                }
                self.view_painting(index).map(Place::View)
            }
            Source::Layer(key, part) => self
                .kept_parts
                .contains(&part)
                .then(|| self.layers.iter().position(|layer| layer.key == key))
                .flatten()
                .map(Place::Layer),
        }
    }

    /// Whether what differs in `place` was drawn again rather than built.
    fn drawn_again(&self, place: Place) -> bool {
        match place {
            Place::View(at) => {
                let view = &self.views[at];
                view.drawn_again && !self.late.contains(&view.id)
            }
            Place::Keyed(_) | Place::Layer(_) => true,
        }
    }

    /// The report of what the frame from scratch, painting `fresh`, found
    /// the frame shown drew again otherwise, if it found anything.
    fn judge(
        &self,
        mut fresh: Vec<Sourced>,
        view_types: &FxHashMap<EntityId, &'static str>,
    ) -> Vec<Found> {
        let mut shown: Vec<&Sourced> = self.painted.iter().collect();
        shown.sort_unstable_by(|a, b| a.text.cmp(&b.text));
        fresh.sort_unstable_by(|a, b| a.text.cmp(&b.text));
        let (only_shown, only_fresh) = differences(&shown, &fresh);
        if only_shown.is_empty() && only_fresh.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<(Place, &str, bool)> = Vec::new();
        for painted in &only_shown {
            if let Some(place) = self
                .source_place(painted)
                .or_else(|| self.place_over(&painted.reach))
                && self.drawn_again(place)
            {
                found.push((place, &painted.text, true));
            }
        }
        for painted in &only_fresh {
            if let Some(place) = self.place_over(&painted.reach)
                && self.drawn_again(place)
            {
                found.push((place, &painted.text, false));
            }
        }
        let mut places: Vec<(Place, Vec<&str>, Vec<&str>)> = Vec::new();
        for (place, text, was_shown) in found {
            let at = match places.iter().position(|(p, ..)| *p == place) {
                Some(at) => at,
                None => {
                    places.push((place, Vec::new(), Vec::new()));
                    places.len() - 1
                }
            };
            if was_shown {
                places[at].1.push(text);
            } else {
                places[at].2.push(text);
            }
        }
        places.sort_by_key(|(place, ..)| *place);
        places
            .into_iter()
            .map(|(place, drawn, afresh)| {
                let mut text = format!("- {}", self.describe(place, view_types));
                list(&mut text, "drawn", &drawn);
                list(&mut text, "afresh", &afresh);
                Found {
                    place: self.key(place),
                    text,
                }
            })
            .collect()
    }

    /// What names `place` from one frame to the next.
    fn key(&self, place: Place) -> PlaceKey {
        match place {
            Place::View(at) => PlaceKey::View(self.views[at].id.clone()),
            Place::Keyed(at) => PlaceKey::Keyed(self.keyed[at].1),
            Place::Layer(at) => PlaceKey::Layer(self.layers[at].container.clone()),
        }
    }

    /// Whether this frame drew `place` again from the last frame, as the
    /// frame before it did.
    fn draws_again(&self, place: &PlaceKey) -> bool {
        match place {
            PlaceKey::View(id) => self
                .views
                .iter()
                .any(|view| view.drawn_again && view.id == *id),
            PlaceKey::Keyed(key) => self.keyed.iter().any(|(_, k)| k == key),
            PlaceKey::Layer(id) => self.layers.iter().any(|layer| layer.container == *id),
        }
    }

    fn describe(&self, place: Place, view_types: &FxHashMap<EntityId, &'static str>) -> String {
        match place {
            Place::View(at) => {
                let view = &self.views[at];
                let entity = crate::fast::splice::view_entity(&view.id);
                let name = entity
                    .and_then(|entity| view_types.get(&entity).copied())
                    .unwrap_or("a view");
                let entity = entity.map_or_else(String::new, |entity| format!(" {entity:?}"));
                format!(
                    "the view {name}{entity}, drawn from the last frame without being notified, at {}",
                    view.id
                )
            }
            Place::Keyed(at) => format!(
                "the stretch painted under the key {:#x}, drawn again for its key",
                self.keyed[at].1
            ),
            Place::Layer(at) => format!(
                "the scroll layer of {}, composited from its last paint (layer {:?})",
                self.layers[at].container, self.layers[at].key
            ),
        }
    }
}

/// Lists at most [`LISTED`] of `lines` under `label`.
#[cfg(any(test, feature = "test-support"))]
fn list(report: &mut String, label: &str, lines: &[&str]) {
    if lines.is_empty() {
        return;
    }
    let _ = write!(report, "\n  {label} ({}):", lines.len());
    for line in lines.iter().take(LISTED) {
        let _ = write!(report, "\n    {line}");
    }
    if lines.len() > LISTED {
        report.push_str("\n    …");
    }
}

/// What each of two lists sorted by text holds that the other does not, a
/// repeat counting once per copy.
#[cfg(any(test, feature = "test-support"))]
fn differences<'a>(
    shown: &[&'a Sourced],
    fresh: &'a [Sourced],
) -> (Vec<&'a Sourced>, Vec<&'a Sourced>) {
    let (mut only_shown, mut only_fresh) = (Vec::new(), Vec::new());
    let (mut a, mut b) = (0, 0);
    while a < shown.len() || b < fresh.len() {
        match (shown.get(a), fresh.get(b)) {
            (Some(x), Some(y)) if x.text == y.text => {
                a += 1;
                b += 1;
            }
            (Some(x), Some(y)) if x.text < y.text => {
                only_shown.push(*x);
                a += 1;
            }
            (Some(_), Some(y)) => {
                only_fresh.push(y);
                b += 1;
            }
            (Some(x), None) => {
                only_shown.push(*x);
                a += 1;
            }
            (None, Some(y)) => {
                only_fresh.push(y);
                b += 1;
            }
            (None, None) => break,
        }
    }
    (only_shown, only_fresh)
}
