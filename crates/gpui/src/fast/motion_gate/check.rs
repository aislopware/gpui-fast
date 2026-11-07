//! Reading a recording back: what a person would see go wrong over time.
//!
//! The rules and their thresholds are in [`RULES`], one row per finding
//! kind. The region rules (`flicker`, `snap`, `changes-twice`) read the
//! window's cells frame by frame with [`super::series`]; the track rules
//! (`pop`, `stale`) read the values a test follows ([`super::MotionRun::track`])
//! against the frames drawn.

use std::collections::HashSet;

use super::raster::{CELL, Grid};
use super::series::{self, Tune, V};
use super::{Recording, TrackKind};

/// One finding kind, its threshold and what it means.
pub struct Rule {
    /// The finding's name.
    pub kind: &'static str,
    /// The number the rule compares with.
    pub threshold: f32,
    /// What a finding of this kind means.
    pub means: &'static str,
}

/// The rules [`check`] applies. Region thresholds are in OKLab ΔE steps of
/// 0.08, as rune's; track thresholds in the track's units.
pub const RULES: &[Rule] = &[
    Rule {
        kind: "flicker",
        threshold: 0.5,
        means: "a region of the window steps out by at least this and comes at least halfway back \
                within 0.4 s, or drifts out and snaps back, with no input and no animation \
                running (rune's region `flash`)",
    },
    Rule {
        kind: "snap",
        threshold: 1.0,
        means: "a region steps by at least this in one frame between frames that hold still, \
                with no input and no animation asking for it (rune's region `snap`)",
    },
    Rule {
        kind: "changes-twice",
        threshold: 0.25,
        means: "a region changes, rests from 2 frames to this many seconds, then carries on the \
                same way with no input between: one change landed as two, a frame of it stale \
                (rune's `changes-twice`)",
    },
    Rule {
        kind: "pop",
        threshold: 2.0,
        means: "an animated value jumps: a position steps at least this many points in one frame \
                between frames that hold still, or its per-frame step changes by more than five \
                times this from one frame to the next (rune's `jump` and `jerk`); an opacity \
                steps by at least 0.5 in one frame (rune's `fade-snap`)",
    },
    Rule {
        kind: "stale",
        threshold: 0.0,
        means: "content that should have updated did not: a followed value moved on a frame \
                whose pixels did not change at all, or held for exactly one frame in the middle \
                of its motion",
    },
];

/// A rule's threshold.
pub fn rule(kind: &str) -> f32 {
    RULES
        .iter()
        .find(|rule| rule.kind == kind)
        .map_or(f32::INFINITY, |rule| rule.threshold)
}

/// What [`check`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// The rule's kind.
    pub kind: &'static str,
    /// The frame it shows on.
    pub frame: usize,
    /// Where: a region of the window, or a followed value's name.
    pub place: String,
    /// What it saw.
    pub detail: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at frame {}, {}: {}",
            self.kind, self.frame, self.place, self.detail
        )
    }
}

/// Everything wrong in `recording`, in frame order.
pub fn check(recording: &Recording) -> Vec<Finding> {
    let mut out = regions(recording);
    out.extend(tracks(recording));
    out.sort_by_key(|finding| finding.frame);
    out
}

fn tune(hz: f64) -> Tune {
    Tune {
        snap: rule("snap"),
        flash: rule("flicker"),
        still: 0.06,
        horizon: (0.4 * hz).round() as usize,
        rest: (2, (f64::from(rule("changes-twice")) * hz).round() as usize),
    }
}

fn regions(recording: &Recording) -> Vec<Finding> {
    let frames = &recording.frames;
    let n = frames.len();
    let Some(first) = frames.first() else {
        return Vec::new();
    };
    let (w, h) = (first.grid.width, first.grid.height);
    if frames
        .iter()
        .any(|frame| (frame.grid.width, frame.grid.height) != (w, h))
    {
        return Vec::new();
    }
    let tune = tune(recording.hz);
    let input_in = |a: usize, b: usize| (a..=b.min(n - 1)).any(|k| frames[k].input.is_some());
    let animating_in = |a: usize, b: usize| (a..=b.min(n - 1)).any(|k| frames[k].animating);
    let cell = |k: usize, i: usize| -> V { frames[k].grid.lab(i).map(|c| c / 0.08) };
    // (kind, frame) → the cells that showed it.
    let mut hits: Vec<((&'static str, usize), usize, series::Event)> = Vec::new();
    for i in 0..w * h {
        let values: Vec<V> = (0..n).map(|k| cell(k, i)).collect();
        if values
            .windows(2)
            .all(|pair| series::dist(&pair[0], &pair[1]) < tune.still)
        {
            continue;
        }
        for event in series::events(&values, &tune) {
            let (a, b) = event.span;
            let k = event.frame;
            let vetoed = match event.kind {
                "flash" => input_in(a, b) || animating_in(a, b),
                "snap" => input_in(k, k) || animating_in(k, k),
                _ => input_in(a, b),
            };
            if !vetoed {
                hits.push(((event.kind, k), i, event));
            }
        }
    }
    hits.sort_by_key(|(key, cell, _)| (key.1, key.0, *cell));
    let mut out = Vec::new();
    let mut start = 0;
    while start < hits.len() {
        let key = hits[start].0;
        let end = start + hits[start..].iter().take_while(|hit| hit.0 == key).count();
        let cells: HashSet<usize> = hits[start..end].iter().map(|hit| hit.1).collect();
        for region in connected(&cells, w) {
            // A lone cell is a glyph's antialiasing or a sub-cell move.
            if region.len() < 2 {
                continue;
            }
            let (x0, y0) = (
                region.iter().map(|c| c % w).min().unwrap_or(0),
                region.iter().map(|c| c / w).min().unwrap_or(0),
            );
            let (x1, y1) = (
                region.iter().map(|c| c % w).max().unwrap_or(0) + 1,
                region.iter().map(|c| c / w).max().unwrap_or(0) + 1,
            );
            let worst = hits[start..end]
                .iter()
                .filter(|hit| region.contains(&hit.1))
                .max_by(|a, b| a.2.size.total_cmp(&b.2.size));
            let Some((_, at, event)) = worst else {
                continue;
            };
            let grid = |k: usize| -> &Grid { &frames[k].grid };
            let colors = match key.0 {
                "snap" => format!("{} → {}", grid(key.1 - 1).hex(*at), grid(key.1).hex(*at)),
                _ => format!(
                    "{} → {} → {}",
                    grid(event.span.0).hex(*at),
                    grid(key.1).hex(*at),
                    grid(event.span.1).hex(*at)
                ),
            };
            out.push(Finding {
                kind: match key.0 {
                    "flash" => "flicker",
                    other => other,
                },
                frame: key.1,
                place: format!(
                    "region {:.0}×{:.0} pt at {:.0},{:.0}",
                    (x1 - x0) as f32 * CELL,
                    (y1 - y0) as f32 * CELL,
                    x0 as f32 * CELL,
                    y0 as f32 * CELL
                ),
                detail: format!(
                    "{} cells, {colors} over frames {}–{}",
                    region.len(),
                    event.span.0,
                    event.span.1
                ),
            });
        }
        start = end;
    }
    out
}

/// The 4-connected regions of `cells` in a grid `w` cells across.
fn connected(cells: &HashSet<usize>, w: usize) -> Vec<Vec<usize>> {
    let mut seen = HashSet::new();
    let mut starts: Vec<usize> = cells.iter().copied().collect();
    starts.sort_unstable();
    let mut regions = Vec::new();
    for start in starts {
        if !seen.insert(start) {
            continue;
        }
        let mut region = vec![start];
        let mut queue = vec![start];
        while let Some(c) = queue.pop() {
            let (x, y) = (c % w, c / w);
            let near = [
                (x > 0).then(|| c - 1),
                (x + 1 < w).then(|| c + 1),
                (y > 0).then(|| c - w),
                Some(c + w),
            ];
            for neighbour in near.into_iter().flatten() {
                if cells.contains(&neighbour) && seen.insert(neighbour) {
                    region.push(neighbour);
                    queue.push(neighbour);
                }
            }
        }
        regions.push(region);
    }
    regions
}

fn tracks(recording: &Recording) -> Vec<Finding> {
    let frames = &recording.frames;
    let mut out = Vec::new();
    for (index, track) in recording.tracks.iter().enumerate() {
        let value = |k: usize| {
            frames
                .get(k)
                .and_then(|frame| frame.values.get(index).copied().flatten())
        };
        let still = match track.kind {
            TrackKind::Position => 0.25,
            TrackKind::Opacity => 0.01,
        };
        let step = |k: usize| -> Option<f32> { Some(value(k)? - value(k.checked_sub(1)?)?) };
        let input = |k: usize| {
            (k.saturating_sub(1)..=k).any(|j| frames.get(j).is_some_and(|f| f.input.is_some()))
        };
        let mut stale_at = HashSet::new();
        for k in 1..frames.len() {
            let Some(d) = step(k) else { continue };
            // Moved, and nothing on screen changed.
            if d.abs() >= still && frames[k].grid == frames[k - 1].grid {
                stale_at.insert(k);
                out.push(Finding {
                    kind: "stale",
                    frame: k,
                    place: track.name.clone(),
                    detail: format!(
                        "moved {:.3} → {:.3}, but the frame drew the same pixels",
                        value(k - 1).unwrap_or(0.),
                        value(k).unwrap_or(0.)
                    ),
                });
                continue;
            }
            // Held one frame inside its motion, then caught up.
            if let (Some(before), Some(after)) = (step(k - 1), step(k + 1))
                && d.abs() < still
                && before.abs() >= still
                && after.abs() >= still
                && before.signum() == after.signum()
                && !input(k)
                && !input(k + 1)
            {
                stale_at.insert(k);
                stale_at.insert(k + 1);
                out.push(Finding {
                    kind: "stale",
                    frame: k,
                    place: track.name.clone(),
                    detail: format!(
                        "held at {:.3} for one frame between steps of {before:.3} and {after:.3}",
                        value(k).unwrap_or(0.)
                    ),
                });
            }
        }
        for k in 1..frames.len() {
            if stale_at.contains(&k) || input(k) {
                continue;
            }
            let Some(d) = step(k) else { continue };
            let (before, after) = (step(k - 1).unwrap_or(0.), step(k + 1).unwrap_or(0.));
            let popped = match track.kind {
                TrackKind::Opacity => d.abs() >= 0.5,
                TrackKind::Position => {
                    let jump = rule("pop");
                    let between_still = before.abs() < still && after.abs() < still;
                    (between_still && d.abs() >= jump)
                        || (before.abs() >= still && (d - before).abs() > 5. * jump)
                }
            };
            if popped {
                out.push(Finding {
                    kind: "pop",
                    frame: k,
                    place: track.name.clone(),
                    detail: format!(
                        "{:.3} → {:.3} in one frame (steps around it {before:.3}, {after:.3})",
                        value(k - 1).unwrap_or(0.),
                        value(k).unwrap_or(0.)
                    ),
                });
            }
        }
    }
    out
}
