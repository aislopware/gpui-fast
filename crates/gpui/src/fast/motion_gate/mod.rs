//! A motion gate for tests: draw a window frame by frame on the test
//! executor's virtual clock, keep a coarse picture of every frame and the
//! values a test follows, and list what a person would see go wrong over
//! time ([`check::RULES`]):
//!
//! - **flicker**: a region that changes and comes back within 0.4 s while no
//!   input lands and nothing animates;
//! - **snap**: a region that steps in one frame with nothing asking for it;
//! - **changes-twice**: one change that lands in two steps with a rest
//!   between, a frame of it stale;
//! - **pop**: a followed value (a position, an opacity) that jumps;
//! - **stale**: a followed value that moved on a frame whose pixels did not
//!   change, or held for one frame in the middle of its motion.
//!
//! `GPUI_CHECK_RETAINED` compares each retained frame with one drawn from
//! scratch; it cannot see a frame that is wrong in time: a blink, a pop, a
//! frame that came late. This can.
//!
//! ```ignore
//! let mut run = MotionRun::new(cx, window, 60.);
//! run.track("panel x", TrackKind::Position, |window| panel_x(window));
//! run.play(cx, 0.2); // the settled start
//! run.act(cx, "open the panel", |cx| click(cx));
//! run.play(cx, 1.0);
//! run.assert_clean(&[]);
//! ```
//!
//! A frame is the window's scene rasterized on the CPU into 12-point cells
//! ([`raster`]), so it runs in any `#[gpui::test]`; [`MotionRun::pixels`]
//! reads rendered frames instead where a test has a renderer (`gpui_macos`'s
//! pixel tests), which also catches what the renderer gets wrong. An input
//! marked with [`MotionRun::act`] keeps the change it asks for from counting,
//! as does an animation frame (one a next-frame callback asked for) for
//! `flicker` and `snap`.
//!
//! The design and the rules are rune's motion gate (anomalyco/rune,
//! `crates/rune/src/motion_gate/`, its region rules from `e6371c48`, #71,
//! and `8b7e8cad`; MIT). Rune also follows each element's geometry and paint;
//! gpui has no element identity across frames to follow, so a test names the
//! values it cares about instead.

pub mod check;
pub mod raster;
pub(crate) mod series;

use std::time::Duration;

use image::RgbaImage;

use crate::{AnyWindowHandle, AppContext as _, TestAppContext, Window};

pub use check::{Finding, RULES, Rule, check};
pub use raster::Grid;

/// What a followed value measures, which sets its thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    /// A position or size, in points.
    Position,
    /// An opacity, 0 to 1.
    Opacity,
}

/// A followed value.
pub struct Track {
    /// What a finding calls it.
    pub name: String,
    /// What it measures.
    pub kind: TrackKind,
}

/// One recorded frame.
pub struct Frame {
    /// The window's picture.
    pub grid: Grid,
    /// An animation asked for this frame.
    pub animating: bool,
    /// The input that landed on this frame, if one did.
    pub input: Option<String>,
    /// Each followed value, in the order of [`Recording::tracks`].
    pub values: Vec<Option<f32>>,
}

/// The frames of a run.
pub struct Recording {
    /// Frames a second.
    pub hz: f64,
    /// The values followed.
    pub tracks: Vec<Track>,
    /// The frames recorded, oldest first.
    pub frames: Vec<Frame>,
}

type Read = Box<dyn Fn(&Window) -> Option<f32>>;
type RenderFrame = Box<dyn FnMut(&mut Window) -> Option<RgbaImage>>;

/// A window being recorded: every [`step`](MotionRun::step) advances the
/// virtual clock a frame, delivers the frame as a platform would (its
/// next-frame callbacks, then a draw if they or anything else left the window
/// dirty) and samples it.
pub struct MotionRun {
    window: AnyWindowHandle,
    dt: Duration,
    reads: Vec<Read>,
    render: Option<RenderFrame>,
    input: Option<String>,
    /// What was recorded so far.
    pub recording: Recording,
}

impl MotionRun {
    /// Records `window` at `hz` frames a second, after drawing it once.
    pub fn new(cx: &mut TestAppContext, window: impl Into<AnyWindowHandle>, hz: f64) -> Self {
        let window = window.into();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .expect("the window is open");
        MotionRun {
            window,
            dt: Duration::from_secs_f64(1. / hz),
            reads: Vec::new(),
            render: None,
            input: None,
            recording: Recording {
                hz,
                tracks: Vec::new(),
                frames: Vec::new(),
            },
        }
    }

    /// Follows a value `read` takes from each frame drawn (`None` where it
    /// isn't there), as a [`TrackKind`].
    pub fn track(
        &mut self,
        name: impl Into<String>,
        kind: TrackKind,
        read: impl Fn(&Window) -> Option<f32> + 'static,
    ) {
        self.recording.tracks.push(Track {
            name: name.into(),
            kind,
        });
        self.reads.push(Box::new(read));
    }

    /// Takes each frame's picture from `render`, a rendered frame
    /// (`Window::render_to_image`), in place of rasterizing its scene.
    pub fn pixels(&mut self, render: impl FnMut(&mut Window) -> Option<RgbaImage> + 'static) {
        self.render = Some(Box::new(render));
    }

    /// Advances one frame without recording it.
    pub fn idle_step(&mut self, cx: &mut TestAppContext) {
        self.frame(cx, false);
    }

    /// Advances `secs` without recording, to set a scene up.
    pub fn idle(&mut self, cx: &mut TestAppContext, secs: f64) {
        for _ in 0..self.frames_in(secs) {
            self.idle_step(cx);
        }
    }

    /// Records one frame.
    pub fn step(&mut self, cx: &mut TestAppContext) {
        self.frame(cx, true);
    }

    /// Records `secs` of frames.
    pub fn play(&mut self, cx: &mut TestAppContext, secs: f64) {
        for _ in 0..self.frames_in(secs) {
            self.step(cx);
        }
    }

    /// An input on the next frame: `f` runs, then the frame it lands on is
    /// recorded. What it changes there is asked for.
    pub fn act(
        &mut self,
        cx: &mut TestAppContext,
        what: &str,
        f: impl FnOnce(&mut TestAppContext),
    ) {
        f(cx);
        self.input = Some(what.to_owned());
        self.step(cx);
    }

    /// What the rules find in the frames so far.
    pub fn findings(&self) -> Vec<Finding> {
        check(&self.recording)
    }

    /// Fails with every finding whose kind `allowed` doesn't list, each with
    /// the reason it would be allowed: `&[("snap", "the badge swaps at once, by design")]`.
    #[track_caller]
    pub fn assert_clean(&self, allowed: &[(&str, &str)]) {
        let found: Vec<String> = self
            .findings()
            .into_iter()
            .filter(|finding| !allowed.iter().any(|(kind, _)| *kind == finding.kind))
            .map(|finding| finding.to_string())
            .collect();
        assert!(
            found.is_empty(),
            "the motion gate found {} artefact(s) over {} frames:\n  {}",
            found.len(),
            self.recording.frames.len(),
            found.join("\n  ")
        );
    }

    fn frames_in(&self, secs: f64) -> usize {
        (secs / self.dt.as_secs_f64()).round() as usize
    }

    fn frame(&mut self, cx: &mut TestAppContext, record: bool) {
        cx.executor().advance_clock(self.dt);
        cx.run_until_parked();
        let reads = &self.reads;
        let render = &mut self.render;
        let sampled = cx
            .update_window(self.window, |_, window, cx| {
                let animating = window.simulate_next_frame(cx) > 0;
                if window.invalidator.is_dirty() {
                    window.draw(cx).clear(cx);
                }
                if !record {
                    return None;
                }
                let viewport = window.viewport_size;
                let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
                let grid = render
                    .as_mut()
                    .and_then(|render| render(window))
                    .map(|image| Grid::of_image(&image, width, height))
                    .unwrap_or_else(|| {
                        Grid::of_scene(
                            &window.rendered_frame.scene,
                            width,
                            height,
                            window.scale_factor(),
                        )
                    });
                let values = reads.iter().map(|read| read(window)).collect();
                Some((grid, animating, values))
            })
            .expect("the window is open");
        if let Some((grid, animating, values)) = sampled {
            self.recording.frames.push(Frame {
                grid,
                animating,
                input: self.input.take(),
                values,
            });
        }
    }
}
