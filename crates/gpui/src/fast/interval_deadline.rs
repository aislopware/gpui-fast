//! Retained foreground work waits a bounded time for its interval to seal.
//!
//! The foreground journal's writer seals an interval at a presented frame, a
//! frame callback that drew nothing, or a turn end that leaves the foreground
//! idle: no turn open, no runnable queued, no frame pending. A pending frame
//! stops blocking that boundary after [`FRAME_DEADLINE`], but a queued
//! runnable blocked it for as long as the foreground stayed busy. A busy
//! foreground is exactly when one turn ends with the next runnable already
//! queued, turn after turn: tasks woken from other threads keep arriving, and
//! a covered window that keeps drawing (test builds keep a covered window's
//! frames coming) only adds presentations, which aren't measured frames and
//! seal nothing. The interval holding a hang then never sealed, and the hang
//! detector, which reports sealed intervals only, never saw the hang.
//!
//! So work the interval retained waits at most [`FRAME_DEADLINE`] for one of
//! those boundaries, as a pending frame does: the first turn to end past it
//! seals the interval idle. The time counts from when the first retained
//! event ended, not from when it began, so a long hang still waits out the
//! deadline after it ends for the frame it starved, and the two stay in one
//! interval. Folded polls retain nothing and start no clock: a foreground busy
//! with only sub-floor polls still seals nothing.

use scheduler::Instant;

use crate::profiler::journal::{
    FRAME_DEADLINE, ForegroundJournalEntry, ForegroundJournalWriter, IntervalBoundary,
};

/// When the writer's open interval first retained work, if it has.
#[derive(Debug, Default)]
pub(crate) struct OpenInterval {
    retained_at: Option<Instant>,
}

impl OpenInterval {
    /// Notes that the open interval retained an event that ended at `at`.
    pub(crate) fn retained(interval: &mut Self, at: Instant) {
        interval.retained_at.get_or_insert(at);
    }

    /// Notes that a boundary sealed the open interval.
    pub(crate) fn sealed(interval: &mut Self) {
        interval.retained_at = None;
    }

    fn overdue(&self, now: Instant) -> bool {
        self.retained_at
            .is_some_and(|at| now.saturating_duration_since(at) >= FRAME_DEADLINE)
    }
}

/// Seals the writer's open interval idle at a turn end `ended_at` that leaves
/// no turn open, once its retained work has waited [`FRAME_DEADLINE`],
/// whatever is still queued or pending. Returns whether it sealed.
pub(crate) fn seal_overdue(writer: &mut ForegroundJournalWriter, ended_at: Instant) -> bool {
    if writer.turn_depth > 0 || !writer.fast_interval.overdue(ended_at) {
        return false;
    }
    writer.record_entry(ForegroundJournalEntry::Boundary(IntervalBoundary::Idle {
        ended_at,
    }));
    true
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use scheduler::{Instant, SpawnTime};

    use crate::WindowId;
    use crate::profiler::hang::{HangDetector, HangTrigger};
    use crate::profiler::journal::{
        FRAME_DEADLINE, ForegroundEvent, ForegroundJournalEntry, TASK_POLL_FLOOR,
        begin_foreground_turn, foreground_runnable_counter, install_test_foreground_journal,
        record_present, record_task_poll,
    };
    use crate::profiler::{PresentTiming, TaskTiming, YieldTime};

    const THRESHOLD: Duration = Duration::from_millis(250);
    /// One refresh of a 60 Hz display.
    const FRAME: Duration = Duration::from_micros(16_667);

    /// One task poll from `start` to `end`, as the platform's dispatcher runs
    /// it: a turn of its own, and the runnable it was counted as finished.
    fn task_poll(start: Instant, end: Instant) {
        begin_foreground_turn();
        record_task_poll(TaskTiming {
            location: std::panic::Location::caller(),
            spawned: SpawnTime(start),
            start,
            end: YieldTime(end),
        });
    }

    /// A presentation of a frame that is no measured sample, as a covered
    /// window's are.
    fn unmeasured_present(window_id: WindowId, at: Instant) {
        let timing = PresentTiming {
            window_id,
            present_start: at,
            present_end: at + Duration::from_millis(1),
            animation_interval: None,
        };
        record_present(timing, None);
    }

    /// A hang followed by a foreground that never drains (every poll ends
    /// with the next runnable queued) and a covered window that presents
    /// every refresh. Nothing in that stream is a boundary the writer would
    /// wait for, so the hang is reported only through the deadline.
    #[test]
    fn a_hang_in_a_foreground_that_never_drains_is_reported_within_the_deadline() {
        let (journal, _journal_guard) = install_test_foreground_journal(4096, 64);
        let mut detector = HangDetector::new(journal, THRESHOLD, THRESHOLD);
        let runnables = foreground_runnable_counter();
        let window_id = WindowId::from(7);
        let hang_start = Instant::now();
        let hang_end = hang_start + Duration::from_millis(600);

        // The hang's runnable, and the one already queued behind it.
        runnables.queued();
        runnables.queued();
        task_poll(hang_start, hang_end);
        let mut at = hang_end;
        while at < hang_end + FRAME_DEADLINE * 3 {
            runnables.queued();
            unmeasured_present(window_id, at);
            task_poll(at + Duration::from_millis(2), at + Duration::from_millis(3));
            at += FRAME;
        }

        let incidents = detector.poll();
        assert_eq!(incidents.len(), 1, "the hang is reported once");
        let incident = &incidents[0];
        assert_eq!(incident.trigger, HangTrigger::Threshold);
        assert!(
            matches!(
                incident.contributors.as_slice(),
                [ForegroundEvent::TaskPoll(poll)]
                    if poll.start == hang_start && poll.end.0 == hang_end
            ),
            "the hang is the one contributor: {:?}",
            incident.contributors
        );
        let sealed_at = incident.snapshot.boundary.end_time();
        assert!(
            sealed_at >= hang_end + FRAME_DEADLINE
                && sealed_at <= hang_end + FRAME_DEADLINE + FRAME,
            "sealed at the first turn end past the deadline, {:?} after the hang",
            sealed_at.saturating_duration_since(hang_end)
        );
    }

    /// The deadline is for retained work: polls folded below the floor keep
    /// sealing nothing however long the foreground stays busy with them, so
    /// they never re-enter the ring as intervals of their own.
    #[test]
    fn a_foreground_busy_with_folded_polls_only_seals_nothing() {
        let (journal, _journal_guard) = install_test_foreground_journal(4096, 64);
        let mut collector = journal.collector();
        let runnables = foreground_runnable_counter();
        let start = Instant::now();

        runnables.queued();
        let mut at = start;
        while at < start + FRAME_DEADLINE * 3 {
            runnables.queued();
            task_poll(at, at + TASK_POLL_FLOOR / 2);
            at += FRAME;
        }

        let entries = collector.collect_unseen().entries;
        assert!(
            entries
                .iter()
                .all(|entry| !matches!(entry, ForegroundJournalEntry::Boundary(_))),
            "no boundary: {entries:?}"
        );
    }
}
