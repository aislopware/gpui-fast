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
//! follow keep their vsync pacing.
//!
//! The immediate frame runs at most once between two ticks, and a run that drew nothing keeps
//! the slot for the draw after it, as a wake that only ran next-frame callbacks leaves it for
//! a keystroke's echo a moment later. A run that left GPUI asking for another frame gives the
//! slot back instead: GPUI asks again when it defers a throttled window's frame (an inactive
//! window draws at most 30 times a second), and the waker that ask also runs would start the
//! immediate frame again at once. Kept armed, a throttled window's main queue ran its frame
//! callback over and over until the throttle let one draw, tens of thousands of times a second
//! in an inactive window with frames to draw. A window the link was stopped for otherwise (hidden, or
//! between two of AppKit's own draws) is not parked, and draws only as the link brings it
//! back.
//!
//! A window takes its frames from a `CVDisplayLink` until the process's windows move to their
//! `CAMetalDisplayLink` (macOS 14 and later), as `gpui_apple::fast::frame_source` decides: when
//! `nextDrawable` blocks. The window moves at its next tick, or as its link next starts. A
//! `CAMetalDisplayLink` pauses itself after an update that drew nothing (`MetalFrames`), so it
//! is never parked here; what stays is the immediate frame. The link hands each update its
//! drawable and the layer gives none while a link drives it, so the immediate frame sets the
//! link aside, draws into the layer's next drawable, and attaches a new link after it: an idle
//! window's first frame reaches the glass as soon as it does with no link, and the frames
//! after it are the link's.

use std::{
    ffi::c_void,
    mem,
    rc::{Rc, Weak},
    time::Duration,
};

use dispatch2::DispatchQueue;
use objc2::{msg_send, runtime::AnyObject};
use parking_lot::Mutex;
use scheduler::Instant;

use crate::{
    display_link::{MetalFrames, WindowFrameSource},
    window::{FrameSource, MacWindowState},
};

/// How long a window's ticks draw nothing and nothing asks for a frame before its link stops:
/// long enough that a pause between keystrokes or a short gap in an animation keeps it, short
/// enough that a window at rest stops waking the app within a few refreshes.
pub(crate) const PARK_AFTER: Duration = Duration::from_millis(100);

/// What a window's link has heard since its last tick, and whether it is parked.
#[derive(Default)]
pub(crate) struct FramePark {
    /// GPUI asked for a frame since the last tick.
    wanted: bool,
    /// GPUI asked for a frame while the immediate frame ran.
    asked_in_immediate: bool,
    /// The tick since which every tick has been idle.
    idle_since: Option<Instant>,
    /// The link was stopped because the window was idle, not for any other reason.
    parked: bool,
    /// The window's `CAMetalDisplayLink`, for when the process's windows move to it (macOS 14
    /// and later).
    metal_frames: Option<Rc<MetalFrames>>,
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

    /// GPUI asked for a frame.
    fn asked(&mut self) {
        self.wanted = true;
        self.asked_in_immediate = true;
    }

    /// The immediate frame is about to run GPUI's frame callback.
    fn immediate_frame_began(&mut self) {
        self.asked_in_immediate = false;
    }

    /// The immediate frame ran GPUI's frame callback, and `drew` whether a frame began within
    /// the last refresh: whether the immediate frame may run again before the next tick. Only a
    /// run that drew nothing and left nothing asked keeps its slot.
    fn immediate_frame_ended(&mut self, drew: bool) -> bool {
        let asked = mem::take(&mut self.asked_in_immediate);
        self.wanted = true;
        !drew && !asked
    }
}

/// A new window's park, which holds its `CAMetalDisplayLink` for later.
pub(crate) fn new(metal_frames: &Option<Rc<MetalFrames>>) -> FramePark {
    FramePark {
        metal_frames: metal_frames.clone(),
        ..FramePark::default()
    }
}

/// What a new window takes its frames from: its `CAMetalDisplayLink` once the process's windows
/// take theirs (`gpui_apple::fast::frame_source`), else a `CVDisplayLink`.
pub(crate) fn frame_source(metal_frames: &Option<Rc<MetalFrames>>) -> FrameSource {
    match metal_frames {
        Some(metal_frames) if gpui_apple::fast::frame_source::metal_link() => {
            FrameSource::Metal(metal_frames.clone())
        }
        _ => FrameSource::DisplayLink(None),
    }
}

/// After a vsync tick's frame request: a window moves to its `CAMetalDisplayLink` once the
/// process's windows take theirs, and else parks once it has been idle for [`PARK_AFTER`].
pub(crate) fn after_tick(window_state: &Rc<Mutex<MacWindowState>>) {
    let mut state = window_state.lock();
    if moves_to_metal(&state) {
        state.start_display_link(window_state);
        return;
    }
    let drew = !state.renderer.idle_for_a_refresh();
    if state.fast_frame_park.tick(Instant::now(), drew) {
        state.stop_display_link();
        state.fast_frame_park.parked = true;
    }
}

/// GPUI wants a frame (`PlatformWindow::schedule_frame`). A parked window gets the immediate
/// frame on the next main-queue turn, as a dirty one does from its waker, which also covers a
/// frame wanted only for next-frame callbacks or to present.
pub(crate) fn schedule_frame(window_state: &Rc<Mutex<MacWindowState>>) {
    let mut state = window_state.lock();
    state.fast_frame_park.asked();
    if !state.fast_frame_park.parked {
        return;
    }
    drop(state);
    let weak = Weak::into_raw(Rc::downgrade(window_state));
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
///
/// A window a `CAMetalDisplayLink` paces draws its immediate frame whenever it has a link,
/// running or paused ([`before_immediate_frame`]); one without (hidden) draws as the link
/// brings it back.
pub(crate) fn ticking(
    state: &mut MacWindowState,
    armed: bool,
    window_state: &Rc<Mutex<MacWindowState>>,
) -> bool {
    match &state.frame_source {
        FrameSource::Metal(metal_frames) => return metal_frames.is_attached(),
        FrameSource::DisplayLink(source) if !state.fast_frame_park.parked => {
            return source.as_ref().is_some_and(WindowFrameSource::is_running);
        }
        FrameSource::DisplayLink(_) => {}
    }
    if armed && state.renderer.idle_for_a_refresh() {
        return true;
    }
    state.start_display_link(window_state);
    false
}

/// The immediate frame is about to draw: a `CAMetalDisplayLink` is set aside, so the frame
/// takes the layer's next drawable now rather than wait for the link's next update.
pub(crate) fn before_immediate_frame(state: &mut MacWindowState) {
    state.fast_frame_park.immediate_frame_began();
    if matches!(state.frame_source, FrameSource::Metal(_)) {
        state.stop_display_link();
    }
}

/// The immediate frame was drawn: a parked window's link starts again, and a
/// `CAMetalDisplayLink` set aside for it is attached again, for the frames that follow to keep
/// their vsync pacing. A run that drew nothing and left nothing asked keeps the immediate frame
/// armed for the draw after it.
pub(crate) fn after_immediate_frame(
    state: &mut MacWindowState,
    window_state: &Rc<Mutex<MacWindowState>>,
) {
    let drew = !state.renderer.idle_for_a_refresh();
    if state.fast_frame_park.immediate_frame_ended(drew) {
        state.immediate_frame_armed = true;
    }
    if state.fast_frame_park.parked || matches!(state.frame_source, FrameSource::Metal(_)) {
        state.start_display_link(window_state);
    }
}

/// A `CAMetalDisplayLink` update began: it arms the immediate frame as a vsync tick does, and
/// takes in what the display reported. The link times the frames itself, so no update is held
/// back to drain a queue.
pub(crate) fn link_update(state: &mut MacWindowState) {
    state.immediate_frame_armed = true;
    state.renderer.take_presentations();
}

/// What GPUI's frame waker holds of a window's `CAMetalDisplayLink`: weak, as the window
/// state owns the waker.
pub(crate) fn link_waker(metal_frames: &Option<Rc<MetalFrames>>) -> Option<Weak<MetalFrames>> {
    metal_frames.as_ref().map(Rc::downgrade)
}

/// GPUI's frame waker ran: a paused `CAMetalDisplayLink` runs again, so its next update draws
/// the frame if the immediate frame does not.
pub(crate) fn wake_link(metal_frames: &Option<Weak<MetalFrames>>) {
    if let Some(metal_frames) = metal_frames.as_ref().and_then(Weak::upgrade) {
        metal_frames.wake();
    }
}

/// The window's link is about to start: it takes the display's refresh, and its
/// `CAMetalDisplayLink` in place of the `CVDisplayLink` once the process's windows take theirs.
pub(crate) fn before_start(state: &mut MacWindowState) {
    note_refresh(state);
    if moves_to_metal(state)
        && let Some(metal_frames) = &state.fast_frame_park.metal_frames
    {
        state.frame_source = FrameSource::Metal(metal_frames.clone());
    }
}

/// Whether a window on a `CVDisplayLink` takes its `CAMetalDisplayLink` now.
fn moves_to_metal(state: &MacWindowState) -> bool {
    matches!(state.frame_source, FrameSource::DisplayLink(_))
        && state.fast_frame_park.metal_frames.is_some()
        && gpui_apple::fast::frame_source::metal_link()
}

/// The window's display reaches its fastest refresh this often (AppKit:
/// `NSScreen.minimumRefreshInterval`, a variable-rate panel's fastest), which paces the
/// renderer's presentation.
fn note_refresh(state: &mut MacWindowState) {
    // SAFETY: `native_window` is the window this state belongs to, alive while it is.
    let screen: *mut AnyObject = unsafe { msg_send![state.native_window, screen] };
    if screen.is_null() {
        // AppKit can report no screen while displays are being reconfigured.
        return;
    }
    // SAFETY: `screen` is a live NSScreen.
    let refresh: f64 = unsafe { msg_send![screen, minimumRefreshInterval] };
    if refresh > 0.0 {
        state
            .renderer
            .set_refresh_interval(Duration::from_secs_f64(refresh));
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

    /// A throttled window: GPUI defers its frame until the throttle's interval has passed, and
    /// each deferral draws nothing, asks for another frame, and wakes the waker, which queues
    /// another immediate frame on the main queue. A tick arms one run; the wakes it causes find
    /// the slot given back, so the main queue runs the callback once between two ticks, where it
    /// ran it until the interval passed.
    #[test]
    fn a_deferred_frame_gives_the_immediate_frame_back_to_the_next_tick() {
        let mut park = FramePark::default();
        let mut armed = true;
        let mut queued = 1;
        let mut runs = 0;
        while queued > 0 && runs < 1000 {
            queued -= 1;
            if !std::mem::take(&mut armed) {
                continue;
            }
            runs += 1;
            park.immediate_frame_began();
            park.asked();
            queued += 1;
            armed = park.immediate_frame_ended(false);
        }
        assert_eq!(runs, 1, "one run between two ticks");
    }

    /// A run that drew nothing and left nothing asked (a wake that only ran next-frame
    /// callbacks) keeps the slot for the draw after it; one that drew takes it. A request heard
    /// before the run began is the run's own, and gives nothing back.
    #[test]
    fn only_an_idle_run_keeps_the_immediate_frame() {
        let mut park = FramePark::default();
        park.asked();
        park.immediate_frame_began();
        assert!(
            park.immediate_frame_ended(false),
            "drew nothing, asked nothing"
        );
        assert!(park.wanted, "the run counts as a request for the parking");
        park.immediate_frame_began();
        assert!(!park.immediate_frame_ended(true), "drew");
        park.immediate_frame_began();
        park.asked();
        assert!(
            !park.immediate_frame_ended(false),
            "asked for another frame"
        );
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
