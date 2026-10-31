//! What the hang detector saw, so a hang that goes unreported says where it
//! was lost.
//!
//! A hang reaches a report through several hands: the foreground times the
//! work and journals it, unless the system slept or woke during it; the
//! monitor's thread drains the journal; the sealer closes the interval
//! around the work; the detector judges the closed interval. Each step can
//! drop it, and without a trace the result of all of them is the same
//! silence.
//!
//! At debug level under [`TARGET`], this logs the monitor starting, every
//! task poll of [`LONG_POLL`] or more as the foreground journals or drops
//! it, and every detector poll that drained, sealed or still holds anything:
//! what it drained (events, the longest of them, boundaries, power
//! boundaries among them, the markers that do no work, entries lost; a
//! storm of frame signals laps the ring and loses the work between two
//! drains), what it sealed and by what, and
//! what stays open. A hang that never reached a report then shows whether
//! its work was never timed, dropped as a power interruption, never drained,
//! or drained and never sealed; a monitor that stopped shows as its polls
//! stopping. Nothing is counted or formatted unless the target is on.

use std::fmt;
use std::time::Duration;

use scheduler::Instant;

use crate::profiler::TaskTiming;
use crate::profiler::journal::{
    ForegroundEvent, ForegroundJournalEntry, FrameSnapshot, IntervalBoundary, IntervalSealer,
};

/// The log target the trace is written under.
pub(crate) const TARGET: &str = "gpui::hang";

/// A task poll at least this long is traced as the foreground times it: far
/// below any hang threshold worth reporting, far above ordinary polls.
const LONG_POLL: Duration = Duration::from_millis(100);

fn enabled() -> bool {
    log::log_enabled!(target: TARGET, log::Level::Debug)
}

/// Notes that the monitor's thread started, polling every `interval`.
pub(crate) fn monitor_started(interval: Duration) {
    log::debug!(target: TARGET, "the monitor drains the journal every {}", Ms(interval));
}

/// Notes a task poll the foreground timed, `dropped` when it spanned a sleep
/// or a wake and so was not journaled.
pub(crate) fn task_poll(timing: &TaskTiming, dropped: bool) {
    // Every foreground poll passes here: the length comes first, as it costs
    // nothing next to asking the logger.
    if timing.poll_duration() >= LONG_POLL
        && enabled()
        && let Some(line) = task_poll_line(timing, dropped)
    {
        log::debug!(target: TARGET, "{line}");
    }
}

fn task_poll_line(timing: &TaskTiming, dropped: bool) -> Option<String> {
    let event = ForegroundEvent::TaskPoll(*timing);
    if event.duration() < LONG_POLL {
        return None;
    }
    let fate = if dropped {
        "dropped: the system slept or woke during it"
    } else {
        "journaled"
    };
    Some(format!("timed {}, {fate}", Event(&event)))
}

/// What one detector poll drained, counted before the sealer takes it.
#[derive(Debug, Default)]
pub(crate) struct Drained {
    entries: usize,
    events: usize,
    longest: Option<ForegroundEvent>,
    presented: usize,
    skipped: usize,
    idle: usize,
    power: usize,
    signals: usize,
    frame_states: usize,
    reentrant: usize,
    lost: u64,
}

impl Drained {
    /// Counts `entries` while the trace is on.
    pub(crate) fn count(entries: &[ForegroundJournalEntry]) -> Option<Self> {
        enabled().then(|| Self::of(entries))
    }

    fn of(entries: &[ForegroundJournalEntry]) -> Self {
        let mut drained = Self {
            entries: entries.len(),
            ..Self::default()
        };
        for entry in entries {
            match entry {
                ForegroundJournalEntry::Event(event) => {
                    drained.events += 1;
                    drained.longest = longer(drained.longest, *event);
                }
                ForegroundJournalEntry::Boundary(boundary) => match boundary {
                    IntervalBoundary::Presented(_) => drained.presented += 1,
                    IntervalBoundary::FrameSkipped(_) => drained.skipped += 1,
                    IntervalBoundary::Idle { .. } => drained.idle += 1,
                    IntervalBoundary::PowerTransition { .. } => drained.power += 1,
                },
                ForegroundJournalEntry::Discontinuity { lost } => drained.lost += lost,
                ForegroundJournalEntry::PlatformSignal(_) => drained.signals += 1,
                ForegroundJournalEntry::FrameState(_) => drained.frame_states += 1,
                ForegroundJournalEntry::ReentrantFrameSkipped { .. } => drained.reentrant += 1,
            }
        }
        drained
    }
}

/// Notes a detector poll at `now` that drained `drained`, sealed `sealed`,
/// and left `open` open.
pub(crate) fn polled(
    drained: Option<Drained>,
    sealed: &[FrameSnapshot],
    open: &IntervalSealer,
    now: Instant,
) {
    if let Some(line) = drained.and_then(|drained| poll_line(&drained, sealed, open, now)) {
        log::debug!(target: TARGET, "{line}");
    }
}

fn poll_line(
    drained: &Drained,
    sealed: &[FrameSnapshot],
    open: &IntervalSealer,
    now: Instant,
) -> Option<String> {
    let open_empty = open.events.is_empty() && open.small_polls.is_empty();
    if drained.entries == 0 && sealed.is_empty() && open_empty {
        return None;
    }
    Some(format!(
        "poll: drained {}; sealed {}; open {}",
        DrainedLine(drained),
        Sealed(sealed),
        Open { open, now },
    ))
}

fn longer(longest: Option<ForegroundEvent>, event: ForegroundEvent) -> Option<ForegroundEvent> {
    match longest {
        Some(longest) if longest.duration() >= event.duration() => Some(longest),
        _ => Some(event),
    }
}

fn longest<'a>(events: impl IntoIterator<Item = &'a ForegroundEvent>) -> Option<ForegroundEvent> {
    events.into_iter().copied().fold(None, longer)
}

fn boundary_name(boundary: &IntervalBoundary) -> &'static str {
    match boundary {
        IntervalBoundary::Presented(_) => "presented",
        IntervalBoundary::FrameSkipped(_) => "skipped",
        IntervalBoundary::Idle { .. } => "idle",
        IntervalBoundary::PowerTransition { .. } => "power",
    }
}

/// Writes `items`, the counts that aren't zero, joined by ", ".
fn counts(f: &mut fmt::Formatter<'_>, items: &[(usize, &str)]) -> fmt::Result {
    let mut first = true;
    for (count, name) in items.iter().filter(|(count, _)| *count > 0) {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        write!(f, "{count} {name}")?;
    }
    Ok(())
}

struct DrainedLine<'a>(&'a Drained);

impl fmt::Display for DrainedLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let drained = self.0;
        write!(f, "{} entries (", drained.entries)?;
        match &drained.longest {
            Some(longest) => write!(
                f,
                "{} events, the longest {}",
                drained.events,
                Event(longest)
            )?,
            None => f.write_str("no events")?,
        }
        let boundaries = [
            (drained.presented, "presented"),
            (drained.skipped, "skipped"),
            (drained.idle, "idle"),
            (drained.power, "power"),
        ];
        if boundaries.iter().any(|(count, _)| *count > 0) {
            f.write_str("; boundaries ")?;
            counts(f, &boundaries)?;
        }
        let markers = [
            (drained.signals, "frame signals"),
            (drained.frame_states, "frame states"),
            (drained.reentrant, "re-entrant skips"),
        ];
        if markers.iter().any(|(count, _)| *count > 0) {
            f.write_str("; markers ")?;
            counts(f, &markers)?;
        }
        if drained.lost > 0 {
            write!(f, "; {} lost", drained.lost)?;
        }
        f.write_str(")")
    }
}

struct Sealed<'a>(&'a [FrameSnapshot]);

impl fmt::Display for Sealed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sealed = self.0;
        if sealed.is_empty() {
            return f.write_str("none");
        }
        let by = |name: &str| {
            sealed
                .iter()
                .filter(|snapshot| boundary_name(&snapshot.boundary) == name)
                .count()
        };
        write!(f, "{} (", sealed.len())?;
        counts(
            f,
            &[
                (by("presented"), "presented"),
                (by("skipped"), "skipped"),
                (by("idle"), "idle"),
                (by("power"), "power"),
            ],
        )?;
        let longest = sealed
            .iter()
            .filter_map(|snapshot| Some((longest(&snapshot.events)?, &snapshot.boundary)))
            .max_by_key(|(event, _)| event.duration());
        if let Some((event, boundary)) = longest {
            write!(
                f,
                "; the longest {}, sealed {}",
                Event(&event),
                boundary_name(boundary)
            )?;
        }
        let dropped: u64 = sealed.iter().map(|snapshot| snapshot.dropped_events).sum();
        if dropped > 0 {
            write!(f, "; {dropped} events dropped")?;
        }
        if sealed.iter().any(|snapshot| snapshot.journal_discontinuous) {
            f.write_str("; discontinuous")?;
        }
        f.write_str(")")
    }
}

struct Open<'a> {
    open: &'a IntervalSealer,
    now: Instant,
}

impl fmt::Display for Open<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let open = self.open;
        let (Some(first), Some(longest)) = (open.events.first(), longest(&open.events)) else {
            return match open.small_polls.len() {
                0 => f.write_str("none"),
                flushes => write!(f, "{flushes} folded flushes only"),
            };
        };
        write!(
            f,
            "{} events, the first ended {} ago, the longest {}",
            open.events.len(),
            Ms(self.now.saturating_duration_since(first.end_time())),
            Event(&longest)
        )
    }
}

/// An event in words: what it was, how long, and for a task poll where the
/// task was spawned.
struct Event<'a>(&'a ForegroundEvent);

impl fmt::Display for Event<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let took = Ms(self.0.duration());
        match self.0 {
            ForegroundEvent::TaskPoll(timing) => write!(
                f,
                "{took} task poll at {}:{}",
                timing.location.file(),
                timing.location.line()
            ),
            ForegroundEvent::Action(timing) => write!(f, "{took} action {}", timing.name),
            ForegroundEvent::Input(timing) => write!(f, "{took} {} input", timing.kind),
            ForegroundEvent::Draw(_) => write!(f, "{took} draw"),
            ForegroundEvent::View(timing) => {
                write!(f, "{took} {:?} of {}", timing.phase, timing.name)
            }
            ForegroundEvent::Present(_) => write!(f, "{took} present"),
            ForegroundEvent::SmallPolls(flush) => {
                write!(f, "{took} of {} folded polls", flush.summary.count)
            }
        }
    }
}

/// A duration in milliseconds, to a tenth.
struct Ms(Duration);

impl fmt::Display for Ms {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1} ms", self.0.as_secs_f64() * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use scheduler::{Instant, SpawnTime};

    use crate::profiler::journal::{
        ForegroundEvent, ForegroundJournalEntry, IntervalBoundary, IntervalSealer, PlatformSignal,
    };
    use crate::profiler::{TaskTiming, YieldTime};
    use crate::{FrameRequestSource, WindowId};

    use super::{Drained, LONG_POLL, poll_line, task_poll_line};

    fn poll(start: Instant, took: Duration) -> TaskTiming {
        TaskTiming {
            location: std::panic::Location::caller(),
            spawned: SpawnTime(start),
            start,
            end: YieldTime(start + took),
        }
    }

    /// Each step a hang passes on its way to a report says what it did with
    /// it, at explicit times: the foreground timing it (or dropping it across
    /// a wake), the detector draining it and holding it open, and the
    /// detector sealing it. A poll that saw nothing says nothing.
    #[test]
    fn the_trace_says_where_a_hang_went() {
        let start = Instant::now();
        let hang = poll(start, Duration::from_millis(600));
        let site = format!("{}:{}", hang.location.file(), hang.location.line());

        assert_eq!(
            task_poll_line(&hang, false).as_deref(),
            Some(format!("timed 600.0 ms task poll at {site}, journaled").as_str())
        );
        assert_eq!(
            task_poll_line(&hang, true).as_deref(),
            Some(
                format!("timed 600.0 ms task poll at {site}, dropped: the system slept or woke during it")
                    .as_str()
            )
        );
        let short = poll(start, LONG_POLL - Duration::from_micros(1));
        assert_eq!(
            task_poll_line(&short, false),
            None,
            "ordinary polls say nothing"
        );

        let mut sealer = IntervalSealer::new(start);
        let quiet = Drained::of(&[]);
        assert_eq!(
            poll_line(&quiet, &[], &sealer, start),
            None,
            "nothing to say"
        );

        let hang_end = hang.end.0;
        let entries = [
            ForegroundJournalEntry::Boundary(IntervalBoundary::PowerTransition { ended_at: start }),
            ForegroundJournalEntry::Event(ForegroundEvent::TaskPoll(hang)),
            ForegroundJournalEntry::PlatformSignal(PlatformSignal {
                window_id: WindowId::from(1),
                signal_at: None,
                handled_at: hang_end,
                source: FrameRequestSource::NativeCallback,
            }),
            ForegroundJournalEntry::Discontinuity { lost: 2 },
        ];
        let drained = Drained::of(&entries);
        let sealed = sealer.push_entries(entries);
        let now = hang_end + Duration::from_millis(400);
        assert_eq!(
            poll_line(&drained, &sealed, &sealer, now).as_deref(),
            Some(
                format!(
                    "poll: drained 4 entries (1 events, the longest 600.0 ms task poll at {site}; \
                     boundaries 1 power; markers 1 frame signals; 2 lost); sealed none; open 1 events, the first ended \
                     400.0 ms ago, the longest 600.0 ms task poll at {site}"
                )
                .as_str()
            ),
            "the power boundary came before the hang, so it sealed nothing"
        );

        let entries = [ForegroundJournalEntry::Boundary(IntervalBoundary::Idle {
            ended_at: now,
        })];
        let drained = Drained::of(&entries);
        let sealed = sealer.push_entries(entries);
        assert_eq!(
            poll_line(&drained, &sealed, &sealer, now).as_deref(),
            Some(
                format!(
                    "poll: drained 1 entries (no events; boundaries 1 idle); sealed 1 (1 idle; \
                     the longest 600.0 ms task poll at {site}, sealed idle; 2 events dropped; \
                     discontinuous); open none"
                )
                .as_str()
            )
        );
    }
}
