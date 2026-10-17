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
//! The frame from scratch is the one the window shows, so the frame shown
//! is correct whatever the check finds, and it is what the next frame draws
//! again from. A frame built afresh element by element in a shadow, as
//! Chromium builds a display item again, would have to take back what
//! building an element leaves behind in GPUI (layout nodes, element state,
//! hitboxes, dispatch nodes); drawing the whole frame again sees the same
//! state just as well, and the records of the first frame say which of its
//! parts were drawn again.
//!
//! A frame from scratch paints every scroll layer that holds content afresh
//! into it, without deciding anything about it, as the frame it repeats did:
//! the next frame finds the layer as current as the last, and composites it
//! just as it would have.

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

/// Whether the frame being drawn is the check's frame from scratch, which
/// paints every scroll layer that holds content afresh.
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

/// What the check's frame from scratch does with the scroll container `id`:
/// its layer, if it holds content, is painted afresh, and nothing about the
/// layer is decided or counted. `None` on any other frame.
#[inline(always)]
pub(crate) fn scratch_decision(
    window: &Window,
    id: &crate::GlobalElementId,
) -> Option<crate::fast::layers::policy::Decision> {
    use crate::fast::layers::policy::Decision;
    scratch(window).then(|| match window.fast_layers.layers.get(id) {
        Some(layer) if layer.record.is_some() => Decision::Repaint,
        _ => Decision::Bypass,
    })
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
    translation: crate::Point<crate::ScaledPixels>,
) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if window.retained_state.check.mode.is_some() && !window.retained_state.check.scratch {
        for part in content.part_scenes_over(in_content) {
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
    let _ = (window, key, content, in_content, translation);
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
    // What the last frame drew otherwise while it asked for this one, this
    // one draws again rather than building it: it was never going to be.
    let confirmed: Vec<Found> = pending
        .into_iter()
        .filter(|found| shown.draws_again(&found.place))
        .collect();
    drawn.clear(cx);

    window.retained_state.check.scratch = true;
    // The frame from scratch repeats the frame shown: its layers are painted
    // as of that frame's number, for the next frame to find them current.
    window.fast_layers.frame = window.fast_layers.frame.saturating_sub(1);
    window.repeat_frame_time();
    window.refreshing = true;
    let drawn = window.draw(cx);
    window.retained_state.check.scratch = false;
    window.retained_state.check.keyed.clear();
    window.retained_state.check.replayed.clear();

    window.retained_state.check.painted_parts = layer_parts(window);
    let fresh = crate::fast::layers::verify::sourced(&window.rendered_frame.scene);
    let found = shown.judge(fresh, &window.retained_state.check.view_types);
    // A frame that asked for the next one may have found too late what it
    // drew again, and builds it then (a hover over a view drawn again, the
    // focus moving while it drew): what it found waits for that frame.
    let asked_for_next = !window.next_frame_callbacks.borrow().is_empty()
        || window.invalidator.is_dirty()
        || !window.retained_state.dirty_subtrees.is_empty();
    let (now, later) = if asked_for_next {
        (Vec::new(), found)
    } else {
        (found, Vec::new())
    };
    window.retained_state.check.pending = later;
    report(window, mode, confirmed, true);
    report(window, mode, now, false);
    drawn
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
