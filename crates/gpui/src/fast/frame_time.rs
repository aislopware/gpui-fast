//! The moment a frame shows: read once as the frame starts drawing, and the
//! time every animation in it stands at, so the animations of one frame move
//! in step however long the frame takes to lay out.
//!
//! A frame can be asked to show the moment the last one showed again
//! ([`Window::repeat_frame_time`]): a check that draws the frame shown once
//! more from scratch, to compare the two, draws it at the same moment, and an
//! animation a hair short of its end in the one is not at its end in the
//! other.
//!
//! A frame can be late because of what it did itself: one that took 50 ms to
//! lay out the motion it started shows that motion 50 ms after its moment.
//! That motion then starts from when the frame finished drawing ([`shown`]),
//! as Core Animation starts what a transaction adds when the transaction
//! commits, so the next frame moves it about one frame on rather than by how
//! long the frame took. Motion already running keeps real time.

use std::time::Duration;

use scheduler::Instant;

use crate::{App, Window};

/// A window's frame moment.
#[derive(Default)]
pub(crate) struct FrameTime {
    /// The moment the last frame begun shows; `None` before the first.
    now: Option<Instant>,
    /// Whether the next frame shows that moment again.
    repeat: bool,
    /// The last frame the window drew to show: its moment and how long after
    /// that moment it finished drawing, on the same clock.
    finished: Option<(Instant, Duration)>,
}

thread_local! {
    /// The moment of the frame a window is drawing on this thread, while it
    /// draws.
    static DRAWING: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Starts a frame: it shows the moment it starts at, or the last frame's when
/// that was asked for.
pub(crate) fn begin_frame(window: &mut Window, cx: &App) {
    let time = &mut window.fast_frame_time;
    if !std::mem::take(&mut time.repeat) || time.now.is_none() {
        time.now = Some(cx.background_executor().now());
    }
    DRAWING.set(time.now);
}

/// Ends the frame begun by [`begin_frame`], one the window shows, recording
/// that it finished drawing now, for [`shown`]. A frame drawn again only to
/// check the one shown ends with [`end_frame`] and records nothing, so it
/// never hides how late that one was.
pub(crate) fn end_shown_frame(window: &mut Window, cx: &App) {
    let time = &mut window.fast_frame_time;
    time.finished = time.now.map(|moment| {
        (
            moment,
            cx.background_executor()
                .now()
                .saturating_duration_since(moment),
        )
    });
    end_frame();
}

/// Where motion that started at `start` starts once the frame that started it
/// has shown: a start at the moment of the last frame drawn moves to when that
/// frame finished drawing. Any other start is kept.
pub(crate) fn shown(window: &Window, start: Instant) -> Instant {
    match window.fast_frame_time.finished {
        Some((moment, took)) if moment == start => start + took,
        _ => start,
    }
}

/// Ends the frame begun by [`begin_frame`]: what reads the time from here on
/// reads now.
pub(crate) fn end_frame() {
    DRAWING.set(None);
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

impl App {
    /// The moment the frame a window is drawing shows ([`Window::frame_time`])
    /// while it draws, and now between frames: for code that animates on its
    /// own clock without a window at hand, to stand where the frame's
    /// animations stand. A start stamped from an event, between frames, is
    /// now.
    pub fn frame_time(&self) -> Instant {
        DRAWING
            .get()
            .unwrap_or_else(|| self.background_executor().now())
    }
}

impl Window {
    /// The moment the frame being drawn shows, which every animation in it
    /// stands at: read once as the frame starts drawing, or the last frame's
    /// again after [`Window::repeat_frame_time`]. A view that animates on its
    /// own clock reads this rather than the wall clock to move in step with
    /// GPUI's animations, and to be drawn again where it stood. Read outside
    /// a frame, it is the last frame's.
    pub fn frame_time(&self) -> Instant {
        self.fast_frame_time.now.unwrap_or_else(Instant::now)
    }

    /// Has the next frame drawn show the moment the last frame showed, so
    /// every animation stands where it stood then: for a check that draws the
    /// frame shown again, from scratch, and compares the two.
    pub fn repeat_frame_time(&mut self) {
        self.fast_frame_time.repeat = true;
    }
}

/// When an animation started. A start stamped on a frame moves to when that
/// frame finished drawing once a later frame draws the animation ([`shown`]).
pub(crate) struct Start {
    at: Instant,
    /// Whether no frame since the one that stamped it has drawn the animation.
    fresh: bool,
}

impl Start {
    /// An animation the frame being drawn starts.
    pub(crate) fn new(window: &Window, cx: &App) -> Self {
        Self {
            at: now(window, cx),
            fresh: true,
        }
    }

    /// The next step of a sequence already running, which keeps real time.
    pub(crate) fn running(window: &Window, cx: &App) -> Self {
        Self {
            at: now(window, cx),
            fresh: false,
        }
    }

    /// When the animation started, as the frame being drawn sees it.
    pub(crate) fn at(&mut self, window: &Window, cx: &App) -> Instant {
        if self.fresh && now(window, cx) != self.at {
            self.at = shown(window, self.at);
            self.fresh = false;
        }
        self.at
    }
}

/// Whether a spring rests and whether the last frame set it moving from rest,
/// which moves the time it was last stepped to when that frame showed.
#[derive(Default)]
pub(crate) struct SpringStart {
    /// Whether a frame has stepped the spring; one just mounted rests.
    stepped: bool,
    resting: bool,
    starting: bool,
}

impl SpringStart {
    /// As a frame steps the spring, before its target changes: `updated_at`
    /// moves when the last frame started the spring, and `resting` is whether
    /// it rests.
    pub(crate) fn step(&mut self, updated_at: &mut Instant, resting: bool, window: &Window) {
        if std::mem::take(&mut self.starting) {
            *updated_at = shown(window, *updated_at);
        }
        self.resting = resting || !std::mem::replace(&mut self.stepped, true);
    }

    /// Once the frame has set the spring's target: `moving` is whether the
    /// spring now has somewhere to go.
    pub(crate) fn set(&mut self, moving: bool) {
        self.starting = self.resting && moving;
    }
}
