//! The three discontinuities of one value over frames, which the region
//! rules read cell by cell:
//!
//! - `snap`: it steps in one frame between frames that hold (nearly) still.
//! - `flash`: it steps out of a still state and comes at least halfway back
//!   within the horizon, or drifts away and snaps back: a one-frame blink, or
//!   a surface that drops out and fades back in.
//! - `changes-twice`: it changes, rests a moment, then carries on the same
//!   way, where one change was meant.
//!
//! A cross-fade or an eased transition moves a little each frame and has no
//! isolated step, so none of them fires on it.
//!
//! The rules are rune's (anomalyco/rune,
//! `crates/rune/src/motion_gate/check/series.rs`, from `e6371c48`, #71; MIT),
//! written again here for gpui's frames.

/// A value at one frame, embedded so that Euclidean distance is how
/// different it looks: a cell's colour in OKLab, in steps of ΔE 0.08.
pub(crate) type V = [f32; 3];

pub(crate) fn dist(a: &V, b: &V) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// How far `p` is from the straight path `a` → `b`.
fn off_path(p: &V, a: &V, b: &V) -> f32 {
    let ab: [f32; 3] = std::array::from_fn(|i| b[i] - a[i]);
    let len2: f32 = ab.iter().map(|x| x * x).sum();
    if len2 < 1e-12 {
        return dist(p, a);
    }
    let t = ((0..3).map(|i| (p[i] - a[i]) * ab[i]).sum::<f32>() / len2).clamp(0., 1.);
    let q: V = std::array::from_fn(|i| a[i] + ab[i] * t);
    dist(p, &q)
}

/// Thresholds, in the units of the embedding, and windows in frames.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tune {
    /// A one-frame step at least this big is a `snap`.
    pub snap: f32,
    /// An excursion at least this big is a `flash`, and each half of a
    /// `changes-twice`.
    pub flash: f32,
    /// A change under this between frames is holding still.
    pub still: f32,
    /// Frames a flash comes back within.
    pub horizon: usize,
    /// Frames of rest between the two halves of a `changes-twice`.
    pub rest: (usize, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Event {
    pub kind: &'static str,
    /// The frame it shows on: the step, or where the second change starts.
    pub frame: usize,
    /// The frames it spans, first to last.
    pub span: (usize, usize),
    /// Its size in the embedding.
    pub size: f32,
}

/// Every discontinuity in `values`.
pub(crate) fn events(values: &[V], tune: &Tune) -> Vec<Event> {
    let n = values.len();
    let d = |k: usize| -> f32 {
        if k == 0 || k >= n {
            return 0.;
        }
        dist(&values[k], &values[k - 1])
    };
    let still = |k: usize| d(k) < tune.still;
    let mut out = Vec::new();
    let mut taken = vec![false; n];
    for k in 1..n {
        let step = d(k);
        if step < tune.flash.min(tune.snap) {
            continue;
        }
        let (prev, next) = (d(k - 1), d(k + 1));
        let (a, b) = (values[k - 1], values[k]);
        // Out of a still state: where it rests within the horizon.
        if prev < step / 4. {
            let mut m = k + 1;
            while m < n.min(k + tune.horizon + 1) && !(still(m) && still(m + 1)) {
                m += 1;
            }
            let rest = values[m.min(n - 1)];
            let off = off_path(&b, &a, &rest);
            if off >= 0.5 * step && off >= tune.flash {
                out.push(Event {
                    kind: "flash",
                    frame: k,
                    span: (k - 1, m.min(n - 1)),
                    size: off,
                });
                taken[k] = true;
                if let Some(back) = (k + 1..=m.min(n - 1)).find(|&j| d(j) >= step / 2.) {
                    taken[back] = true;
                }
                continue;
            }
        }
        // Into a still state after drifting from one: a snap back.
        if next < step / 4. && !taken[k] && k >= 3 {
            let from = k.saturating_sub(tune.horizon).max(2);
            if let Some(j) = (from..k - 1).rev().find(|&j| still(j) && still(j - 1))
                && (j + 1..k).all(|i| d(i) < step / 2.)
            {
                let off = off_path(&a, &values[j], &b);
                if off >= 0.5 * step && off >= tune.flash {
                    out.push(Event {
                        kind: "flash",
                        frame: k,
                        span: (j, k),
                        size: off,
                    });
                    taken[k] = true;
                    continue;
                }
            }
        }
        if !taken[k] && step >= tune.snap && prev < step / 4. && next < step / 4. {
            out.push(Event {
                kind: "snap",
                frame: k,
                span: (k - 1, k),
                size: step,
            });
        }
    }
    // Motion segments: runs of frames that change, and the rests between them.
    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut k = 1;
    while k < n {
        if still(k) {
            k += 1;
            continue;
        }
        let start = k;
        while k < n && !still(k) {
            k += 1;
        }
        segments.push((start, k - 1));
    }
    for pair in segments.windows(2) {
        let ((a1, b1), (a2, b2)) = (pair[0], pair[1]);
        let rest = a2 - b1 - 1;
        if rest < tune.rest.0 || rest > tune.rest.1 {
            continue;
        }
        let (v0, v1, r, v2) = (values[a1 - 1], values[b1], values[a2 - 1], values[b2]);
        let (first, second) = (dist(&v0, &v1), dist(&r, &v2));
        if first < tune.flash || second < tune.flash {
            continue;
        }
        let dot: f32 = (0..3).map(|i| (v1[i] - v0[i]) * (v2[i] - r[i])).sum();
        // Each half one step, so neither is a transition: those are `snap`'s.
        if dot > 0.7 * first * second
            && !(taken[a2] || out.iter().any(|e| e.frame == a1 || e.frame == a2))
        {
            out.push(Event {
                kind: "changes-twice",
                frame: a2,
                span: (a1 - 1, b2),
                size: first + second,
            });
        }
    }
    out.sort_by_key(|event| event.frame);
    out
}

#[cfg(test)]
mod tests {
    use super::{Tune, V, events};

    const TUNE: Tune = Tune {
        snap: 1.0,
        flash: 0.5,
        still: 0.01,
        horizon: 24,
        rest: (2, 15),
    };

    fn run(xs: &[f32]) -> Vec<&'static str> {
        let values: Vec<V> = xs.iter().map(|&x| [x, 0., 0.]).collect();
        events(&values, &TUNE)
            .into_iter()
            .map(|event| event.kind)
            .collect()
    }

    fn ease(from: f32, to: f32, frames: usize) -> Vec<f32> {
        (1..=frames)
            .map(|i| {
                let t = i as f32 / frames as f32;
                from + (to - from) * t * t * (3. - 2. * t)
            })
            .collect()
    }

    #[test]
    fn a_step_between_still_frames_snaps() {
        assert_eq!(run(&[0., 0., 0., 2., 2., 2.]), ["snap"]);
    }

    #[test]
    fn a_transition_neither_snaps_nor_flashes() {
        let mut xs = vec![0.; 4];
        xs.extend(ease(0., 2., 12));
        xs.extend([2.; 6]);
        assert!(run(&xs).is_empty(), "{:?}", run(&xs));
    }

    #[test]
    fn a_one_frame_blink_flashes_once() {
        assert_eq!(run(&[1., 1., 1., 0., 1., 1., 1.]), ["flash"]);
    }

    #[test]
    fn dropping_out_and_fading_back_flashes() {
        let mut xs = vec![2.; 4];
        xs.push(0.);
        xs.extend(ease(0., 2., 12));
        xs.extend([2.; 6]);
        assert_eq!(run(&xs), ["flash"]);
    }

    #[test]
    fn a_transition_that_rests_and_carries_on_changes_twice() {
        let mut xs = vec![0.; 3];
        xs.extend(ease(0., 1., 8));
        xs.extend([1.; 4]);
        xs.extend(ease(1., 2., 8));
        xs.extend([2.; 4]);
        assert_eq!(run(&xs), ["changes-twice"]);
    }

    #[test]
    fn going_there_and_back_later_changes_once_each_way() {
        let mut xs = vec![0.; 3];
        xs.extend(ease(0., 1., 8));
        xs.extend([1.; 4]);
        xs.extend(ease(1., 0., 8));
        xs.extend([0.; 4]);
        assert!(run(&xs).is_empty(), "{:?}", run(&xs));
    }
}
