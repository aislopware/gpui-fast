//! The moment a frame shows: read once as the frame starts drawing, and the
//! time every animation in it stands at, so the animations of one frame move
//! in step however long the frame takes to lay out.
//!
//! A frame can be asked to show the moment the last one showed again
//! ([`Window::repeat_frame_time`]): a check that draws the frame shown once
//! more from scratch, to compare the two, draws it at the same moment, and an
//! animation a hair short of its end in the one is not at its end in the
//! other.

use scheduler::Instant;

use crate::{App, Window};

/// A window's frame moment.
#[derive(Default)]
pub(crate) struct FrameTime {
    /// The moment the last frame begun shows; `None` before the first.
    now: Option<Instant>,
    /// Whether the next frame shows that moment again.
    repeat: bool,
}

/// Starts a frame: it shows the moment it starts at, or the last frame's when
/// that was asked for.
pub(crate) fn begin_frame(window: &mut Window, cx: &App) {
    let time = &mut window.fast_frame_time;
    if !std::mem::take(&mut time.repeat) || time.now.is_none() {
        time.now = Some(cx.background_executor().now());
    }
}

/// The moment the frame being drawn shows; before any frame, now.
pub(crate) fn now(window: &Window, cx: &App) -> Instant {
    window
        .fast_frame_time
        .now
        .unwrap_or_else(|| cx.background_executor().now())
}

/// How long it has been since `start` at the moment the frame being drawn
/// shows; nothing for a `start` after it.
pub(crate) fn since(window: &Window, cx: &App, start: Instant) -> std::time::Duration {
    now(window, cx).saturating_duration_since(start)
}

impl Window {
    /// Has the next frame drawn show the moment the last frame showed, so
    /// every animation stands where it stood then: for a check that draws the
    /// frame shown again, from scratch, and compares the two.
    pub fn repeat_frame_time(&mut self) {
        self.fast_frame_time.repeat = true;
    }
}
