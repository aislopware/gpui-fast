//! A window's display link stops while GPUI wants no frames, and starts again the moment it
//! wants one.
//!
//! The link ticks every refresh while the window is on screen, and each tick wakes the app,
//! 60 to 120 times a second for a window that shows nothing new. GPUI says when it wants a
//! frame: the window's invalidator wakes the platform as the window becomes dirty
//! (`frame_waker`, which runs `immediate_frame`), and `schedule_frame` follows every effect
//! flush and every frame that leaves a window dirty, with a frame to present or with
//! next-frame callbacks (an animation's). So a window whose ticks have drawn nothing and asked
//! for nothing for [`PARK_AFTER`] stops its link: it is parked.
//!
//! Waking adds no latency. A parked window's next frame is the immediate frame, drawn as soon
//! as GPUI wants it, as it is for a window whose link kept running; the link starts again only
//! after that frame is drawn, so starting it is off the path to the glass, and the frames that
//! follow keep their vsync pacing. A window the link was stopped for otherwise (hidden, or
//! between two of AppKit's own draws) is not parked, and draws only as the link brings it
//! back.

use std::{
    ffi::c_void,
    mem,
    sync::{Arc, Weak},
    time::Duration,
};

use dispatch2::DispatchQueue;
use parking_lot::Mutex;
use scheduler::Instant;

use crate::{display_link::WindowFrameSource, window::MacWindowState};

/// How long a window's ticks draw nothing and nothing asks for a frame before its link stops:
/// long enough that a pause between keystrokes or a short gap in an animation keeps it, short
/// enough that a window at rest stops waking the app within a few refreshes.
pub(crate) const PARK_AFTER: Duration = Duration::from_millis(100);

/// What a window's link has heard since its last tick, and whether it is parked.
#[derive(Default)]
pub(crate) struct FramePark {
    /// GPUI asked for a frame since the last tick.
    wanted: bool,
    /// The tick since which every tick has been idle.
    idle_since: Option<Instant>,
    /// The link was stopped because the window was idle, not for any other reason.
    parked: bool,
}

impl FramePark {
    /// A tick has run its frame request: whether the link stops now. `drew` is whether a frame
    /// began within the last refresh.
    fn tick(&mut self, now: Instant, drew: bool) -> bool {
        let wanted = mem::take(&mut self.wanted);
        if wanted || drew {
            self.idle_since = None;
            return false;
        }
        let since = *self.idle_since.get_or_insert(now);
        let park = now.saturating_duration_since(since) >= PARK_AFTER;
        if park {
            self.idle_since = None;
        }
        park
    }

    /// The link stopped, for whatever reason (`stop_display_link`): the window is parked only
    /// if [`after_tick`] says so next.
    pub(crate) fn stopped(park: &mut Self) {
        park.parked = false;
    }
}

/// After a vsync tick's frame request: park the window once it has been idle for
/// [`PARK_AFTER`].
pub(crate) fn after_tick(state: &mut MacWindowState) {
    let drew = !state.renderer.idle_for_a_refresh();
    if state.fast_frame_park.tick(Instant::now(), drew) {
        state.stop_display_link();
        state.fast_frame_park.parked = true;
    }
}

/// GPUI wants a frame (`PlatformWindow::schedule_frame`). A parked window gets the immediate
/// frame on the next main-queue turn, as a dirty one does from its waker, which also covers a
/// frame wanted only for next-frame callbacks or to present.
pub(crate) fn schedule_frame(window_state: &Arc<Mutex<MacWindowState>>) {
    let mut state = window_state.lock();
    state.fast_frame_park.wanted = true;
    if !state.fast_frame_park.parked {
        return;
    }
    drop(state);
    let weak = Weak::into_raw(Arc::downgrade(window_state));
    // SAFETY: `immediate_frame` runs on the main queue, where window state is used, and takes
    // over the `Weak` that `Weak::into_raw` handed out, as it does from `frame_waker`.
    unsafe {
        DispatchQueue::main().exec_async_f(
            weak.cast_mut().cast::<c_void>(),
            crate::window::immediate_frame,
        );
    }
}

/// Whether the immediate frame may draw now (`armed`: a tick passed since the last one). A
/// running link says yes, as before. A parked window draws now if it can, and the link starts
/// after the frame ([`after_immediate_frame`]); if it cannot draw yet (it drew within the last
/// refresh, or no tick passed), the link starts now and its next tick draws.
pub(crate) fn ticking(state: &mut MacWindowState, armed: bool) -> bool {
    if !state.fast_frame_park.parked {
        return state
            .frame_source
            .as_ref()
            .is_some_and(WindowFrameSource::is_running);
    }
    if armed && state.renderer.idle_for_a_refresh() {
        return true;
    }
    state.start_display_link();
    false
}

/// The immediate frame was drawn: a parked window's link starts again, for the frames that
/// follow to keep their vsync pacing.
pub(crate) fn after_immediate_frame(state: &mut MacWindowState) {
    state.fast_frame_park.wanted = true;
    if state.fast_frame_park.parked {
        state.start_display_link();
    }
}

#[cfg(test)]
mod tests {
    use super::{FramePark, PARK_AFTER};
    use scheduler::Instant;
    use std::time::Duration;

    const TICK: Duration = Duration::from_micros(8_333);

    /// Ticks that draw nothing and hear no request stop the link once they have gone on for
    /// [`PARK_AFTER`], and not before.
    #[test]
    fn idle_ticks_stop_the_link_after_a_while() {
        let mut park = FramePark::default();
        let start = Instant::now();
        let mut at = start;
        let mut parked_at = None;
        for _ in 0..100 {
            if park.tick(at, false) {
                parked_at = Some(at);
                break;
            }
            at += TICK;
        }
        let parked_at = parked_at.expect("an idle window stops its link");
        let idle = parked_at.saturating_duration_since(start);
        assert!(idle >= PARK_AFTER && idle < PARK_AFTER + TICK, "{idle:?}");
    }

    /// A tick that drew, or a request heard since the last tick, starts the idle time over.
    #[test]
    fn a_frame_or_a_request_keeps_the_link() {
        let mut park = FramePark::default();
        let mut at = Instant::now();
        for tick in 0..200 {
            let drew = tick % 10 == 0;
            if tick % 10 == 5 {
                park.wanted = true;
            }
            assert!(!park.tick(at, drew), "tick {tick}");
            at += TICK;
        }
    }
}
