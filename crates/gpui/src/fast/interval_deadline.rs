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
//!
//! The writer only acts at a turn end, and a foreground can go quiet with the
//! interval still open: its window dirty, the frame never drawn, nothing left
//! to run. No turn ends, so the writer never sees its deadline pass. The hang
//! detector closes such an interval instead, at its first poll past the
//! deadline ([`push_entries`]), as idle where the foreground last recorded
//! work. It polls on its own thread whatever the foreground does, so an idle
//! app wakes for nothing more. The two sides measure the same deadline from
//! the same event, so while the foreground still has turns to end the writer
//! seals first; a turn ending just as the detector polls splits the interval
//! there, every event whole on one side or the other.

use scheduler::Instant;

use crate::profiler::journal::{
    FRAME_DEADLINE, ForegroundEvent, ForegroundJournalEntry, ForegroundJournalWriter,
    FrameSnapshot, IntervalBoundary, IntervalSealer,
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

/// Seals what `entries` complete, as [`IntervalSealer::push_entries`] does,
/// and then, at `now`, the interval still open once the first event it holds
/// ended [`FRAME_DEADLINE`] ago: idle, ended where its last work ended.
pub(crate) fn push_entries(
    sealer: &mut IntervalSealer,
    entries: impl IntoIterator<Item = ForegroundJournalEntry>,
    now: Instant,
) -> Vec<FrameSnapshot> {
    let mut snapshots = sealer.push_entries(entries);
    if let Some(ended_at) = quiet_since(sealer, now) {
        snapshots.push(sealer.seal(IntervalBoundary::Idle { ended_at }));
    }
    snapshots
}

/// When the sealer's open interval last recorded work, if its first event
/// ended [`FRAME_DEADLINE`] or more before `now`.
fn quiet_since(sealer: &IntervalSealer, now: Instant) -> Option<Instant> {
    let first = sealer.events.first()?;
    if now.saturating_duration_since(first.end_time()) < FRAME_DEADLINE {
        return None;
    }
    sealer
        .events
        .iter()
        .map(ForegroundEvent::end_time)
        .chain(sealer.small_polls.iter().map(|flush| flush.until))
        .max()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use scheduler::{Instant, SpawnTime};

    use crate::WindowId;
    use crate::profiler::hang::{HangDetector, HangIncident, HangTrigger};
    use crate::profiler::journal::{
        FRAME_DEADLINE, ForegroundEvent, ForegroundJournalEntry, IntervalBoundary, IntervalSealer,
        TASK_POLL_FLOOR, begin_foreground_turn, foreground_runnable_counter,
        install_test_foreground_journal, record_frame_pending, record_present, record_task_poll,
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

    /// A hang, and then nothing: the window it kept from drawing is dirty, so
    /// the writer keeps the interval open at the hang's turn end, and no turn
    /// ends after it. The detector closes the interval at its first poll past
    /// the deadline, idle where the hang ended, and reports the hang.
    #[test]
    fn a_hang_before_the_foreground_goes_quiet_is_reported_at_the_first_poll_past_the_deadline() {
        let (journal, _journal_guard) = install_test_foreground_journal(64, 8);
        let mut collector = journal.collector();
        let runnables = foreground_runnable_counter();
        let window_id = WindowId::from(9);
        let dirty_at = Instant::now();
        let hang_start = dirty_at + Duration::from_millis(1);
        let hang_end = hang_start + Duration::from_millis(600);

        record_frame_pending(window_id, dirty_at);
        runnables.queued();
        task_poll(hang_start, hang_end);
        let entries = collector.collect_unseen().entries;
        assert!(
            entries
                .iter()
                .all(|entry| !matches!(entry, ForegroundJournalEntry::Boundary(_))),
            "the pending frame keeps the interval open: {entries:?}"
        );

        let mut sealer = IntervalSealer::new(dirty_at);
        let just_before = hang_end + FRAME_DEADLINE - Duration::from_micros(1);
        let early = super::push_entries(&mut sealer, entries, just_before);
        assert!(early.is_empty(), "nothing closes before the deadline");
        let snapshots = super::push_entries(&mut sealer, [], hang_end + FRAME_DEADLINE);
        let [snapshot] = snapshots.as_slice() else {
            panic!("one interval closes at the deadline: {snapshots:?}");
        };
        assert!(
            matches!(snapshot.boundary, IntervalBoundary::Idle { ended_at } if ended_at == hang_end),
            "closed idle where the hang ended: {:?}",
            snapshot.boundary
        );
        let incident = HangIncident::detect(snapshot.clone(), THRESHOLD, THRESHOLD)
            .expect("the hang is reported");
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
        let later = super::push_entries(&mut sealer, [], hang_end + FRAME_DEADLINE * 3);
        assert!(later.is_empty(), "closed once: {later:?}");
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
