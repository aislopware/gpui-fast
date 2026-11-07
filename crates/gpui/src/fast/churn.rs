//! Debug builds' warnings for views that are built more than they need to be.
//!
//! Retention draws a view again from last frame while nothing it read
//! changed, so a view's `render` runs only when it has to. Two patterns
//! defeat that without breaking anything, and so go unseen:
//!
//! - **Built for nothing**: the view is rendered on frames whose scene comes
//!   out exactly as the frame before's ([`crate::fast::same_frame`]). What
//!   notified it, or what it read, changes nothing it shows: a timer that
//!   notifies every second for a label that changes every minute, or a model
//!   notified for a field this view doesn't draw. After [`UNCHANGED`] such
//!   frames it is named.
//! - **Built every frame**: the view is built on each of [`STREAK`] frames in
//!   a row. During its own animation that is expected, but then the whole
//!   view is rebuilt for the part that moves; outside one, something tells it
//!   to on every frame. Either way it is named once, with why it was built.
//!
//! Each warning goes to `log::warn!` once per view and names its type, its
//! entity and why it was last built: it was notified (or asked for an
//! animation frame), a view inside it was and it could not be drawn around
//! it, or something it read changed.
//!
//! Off unless `GPUI_CHURN_WARN=1` is set when the window opens (or a test
//! turns it on with [`Window::set_churn_warnings`]), so a debug build pays one
//! branch per built view while it is off. Release builds hold none of it: the
//! state is empty and every hook compiles to nothing.
//!
//! The idea is rune's debug-only perf warnings (anomalyco/rune,
//! `crates/rune/src/tooling/perf_warn.rs` and `perf_warn/idle.rs`, at
//! `4cb8f788`; MIT): a `poll` that says it changed what shows when nothing
//! did, and a window that keeps drawing the same frame while nothing asks it
//! to, each warned once per site and only in debug builds.

#[cfg(debug_assertions)]
use collections::FxHashMap;

use crate::{EntityId, Window};

/// Frames a view is built on that come out the same as the frame before,
/// before it is named.
#[cfg(debug_assertions)]
pub(crate) const UNCHANGED: u32 = 30;

/// Frames in a row a view is built on before it is named.
#[cfg(debug_assertions)]
pub(crate) const STREAK: u32 = 120;

/// A window's churn meter: nothing in release builds, nothing while off.
#[derive(Default)]
pub(crate) struct Churn {
    #[cfg(debug_assertions)]
    meter: Option<Box<Meter>>,
}

impl Churn {
    /// The meter `GPUI_CHURN_WARN=1` asks for.
    pub(crate) fn from_env() -> Self {
        Churn {
            #[cfg(debug_assertions)]
            meter: std::env::var_os("GPUI_CHURN_WARN")
                .is_some_and(|value| value == "1")
                .then(Box::default),
        }
    }
}

/// Why a view was built this frame.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Why {
    /// It was notified, or asked for an animation frame.
    Notified,
    /// A view inside it was, and it could not be drawn again around it.
    Inside,
    /// Something it read changed.
    Read,
}

#[cfg(debug_assertions)]
impl Why {
    fn says(self) -> &'static str {
        match self {
            Why::Notified => "it was notified or asked for an animation frame",
            Why::Inside => "a view inside it was notified and it could not be drawn around it",
            Why::Read => "something it read changed",
        }
    }
}

#[cfg(debug_assertions)]
#[derive(Default)]
struct Meter {
    /// Frames drawn since the meter started.
    frames: u64,
    /// The views built in the frame being drawn.
    built: Vec<(EntityId, &'static str, Why)>,
    views: FxHashMap<EntityId, ViewCount>,
    /// The warnings given, for tests.
    #[cfg(any(test, feature = "test-support"))]
    warnings: Vec<String>,
}

#[cfg(debug_assertions)]
#[derive(Default)]
struct ViewCount {
    /// The last frame it was built on.
    last: u64,
    /// Frames in a row it was built on, up to `last`.
    streak: u32,
    /// Frames it was built on that drew what the frame before drew.
    unchanged: u32,
    warned_streak: bool,
    warned_unchanged: bool,
}

/// Whether `window` measures churn: whether frames are compared for it.
#[inline(always)]
pub(crate) fn enabled(window: &Window) -> bool {
    #[cfg(debug_assertions)]
    {
        window.retained_state.churn.meter.is_some()
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = window;
        false
    }
}

/// Notes that the view `entity`, of type `name`, is being built this frame.
#[inline(always)]
pub(crate) fn built(window: &mut Window, entity: EntityId, name: &'static str) {
    #[cfg(debug_assertions)]
    {
        if window.retained_state.churn.meter.is_none() {
            return;
        }
        let why = if window.retained_state.notified_entities.contains(&entity) {
            Why::Notified
        } else if window.dirty_views.contains(&entity) {
            Why::Inside
        } else {
            Why::Read
        };
        if let Some(meter) = window.retained_state.churn.meter.as_mut() {
            meter.built.push((entity, name, why));
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = (window, entity, name);
}

/// Ends a frame `Window::draw` drew: `unchanged` when its scene came out as
/// the frame before's.
#[inline(always)]
pub(crate) fn frame_drawn(window: &mut Window, unchanged: bool) {
    #[cfg(debug_assertions)]
    if let Some(meter) = window.retained_state.churn.meter.as_mut() {
        meter.frame_drawn(unchanged);
    }
    #[cfg(not(debug_assertions))]
    let _ = (window, unchanged);
}

#[cfg(debug_assertions)]
impl Meter {
    fn frame_drawn(&mut self, unchanged: bool) {
        self.frames += 1;
        let frame = self.frames;
        let mut built = std::mem::take(&mut self.built);
        for (entity, name, why) in built.drain(..) {
            let count = self.views.entry(entity).or_default();
            // A view built twice in a frame (laid out, then built for a view
            // around it) counts once.
            if count.last == frame {
                continue;
            }
            count.streak = if count.last + 1 == frame {
                count.streak + 1
            } else {
                1
            };
            count.last = frame;
            count.unchanged += u32::from(unchanged);
            let mut warn = None;
            if count.unchanged >= UNCHANGED && !count.warned_unchanged {
                count.warned_unchanged = true;
                warn = Some(format!(
                    "view `{name}` ({entity:?}) was built on {} frames that drew exactly what \
                     the frame before drew: {}, but nothing it shows changed. Notify it only when \
                     what it renders changes",
                    count.unchanged,
                    why.says()
                ));
            } else if count.streak >= STREAK && !count.warned_streak {
                count.warned_streak = true;
                warn = Some(format!(
                    "view `{name}` ({entity:?}) was built on each of the last {} frames, last \
                     because {}: if only part of it changes from frame to frame, hold that part \
                     in a view of its own",
                    count.streak,
                    why.says()
                ));
            }
            if let Some(warning) = warn {
                log::warn!("gpui: {warning}. (debug builds only; GPUI_CHURN_WARN=1 asks for it)");
                #[cfg(any(test, feature = "test-support"))]
                self.warnings.push(warning);
            }
        }
        self.built = built;
        // Views built long ago no longer count towards a streak; forget them
        // so a window that builds many views over its life stays small.
        if frame.is_multiple_of(u64::from(STREAK)) {
            self.views.retain(|_, count| {
                frame - count.last <= u64::from(STREAK)
                    || count.warned_streak
                    || count.warned_unchanged
            });
        }
    }
}

#[cfg(all(debug_assertions, any(test, feature = "test-support")))]
impl Window {
    /// Turns this window's churn warnings ([`crate::fast::churn`]) on or off,
    /// as `GPUI_CHURN_WARN=1` does when the window opens. Debug builds only.
    pub fn set_churn_warnings(&mut self, on: bool) {
        self.retained_state.churn.meter = on.then(Box::default);
    }

    /// The churn warnings this window gave since the last call.
    pub fn take_churn_warnings(&mut self) -> Vec<String> {
        self.retained_state
            .churn
            .meter
            .as_mut()
            .map(|meter| std::mem::take(&mut meter.warnings))
            .unwrap_or_default()
    }
}
