//! Decides when the active window's next snapshot is due.

use std::time::Duration;

use tokio::time::Instant;

use super::config::Timing;
use super::pacing::{Burst, Pacing};
use super::record::{Change, Trigger};

#[derive(Debug, Clone, Copy)]
struct Pending {
    burst: Burst,
    due: Instant,
    /// Whether any change of the burst was an edit.
    edited: bool,
}

/// Decides when the next snapshot is due.
#[derive(Debug)]
pub struct Debounce {
    timing: Timing,
    pending: Option<Pending>,
    last_snapshot: Option<Instant>,
    /// The current shortest time between two content snapshots.
    interval: Duration,
    /// The current time after which a quiet window is read anyway.
    poll: Option<Duration>,
}

impl Debounce {
    pub const fn new(timing: Timing) -> Self {
        Self {
            timing,
            pending: None,
            last_snapshot: None,
            interval: timing.min_interval,
            poll: timing.poll_interval,
        }
    }

    pub fn poke(&mut self, now: Instant, trigger: Trigger) {
        let edited = trigger == Trigger::Edited || self.pending.is_some_and(|p| p.edited);
        let burst =
            self.pending.map_or_else(|| Burst::new(now, trigger), |p| p.burst.with_change(now));
        let mut due = burst.settled(&self.timing);
        if rests(burst.trigger())
            && let Some(last) = self.last_snapshot
        {
            due = due.max(last + self.interval);
        }
        self.pending = Some(Pending { burst, due, edited });
    }

    /// When the next snapshot is due: at the pending trigger, or else at the next poll.
    pub fn due(&self) -> Option<Instant> {
        self.pending.map(|p| p.due).or_else(|| Some(self.last_snapshot? + self.poll?))
    }

    pub fn take(&mut self, now: Instant) -> Option<Trigger> {
        let trigger = match self.pending.take() {
            Some(pending) => pending.burst.trigger(),
            None if now >= self.last_snapshot? + self.poll? => Trigger::Polled,
            None => return None,
        };
        self.last_snapshot = Some(now);
        Some(trigger)
    }

    /// Sets the rest before the next content snapshot from the cost of the last read.
    pub fn paced(&mut self, read_time: Duration) {
        self.interval = Pacing::active(&self.timing).rest_after(read_time);
    }

    /// Adapts the polling to the result of a snapshot that `trigger` led to. Only a poll or a
    /// change inside the window counts.
    ///
    /// Each unchanged snapshot doubles the wait before the next poll, and a changed snapshot
    /// resets it.
    pub fn observed(&mut self, trigger: Trigger, change: Change) {
        if !(is_content(trigger) || trigger == Trigger::Polled) {
            return;
        }
        self.poll = match change {
            Change::Changed => self.timing.poll_interval,
            Change::Unchanged => {
                self.poll.map(|poll| (poll * 2).min(self.timing.max_poll_interval))
            }
        };
    }

    /// Reschedules the snapshot that `take` just handed out, because the window of the snapshot
    /// was not ready to be read.
    pub fn defer(&mut self, now: Instant, trigger: Trigger) {
        self.last_snapshot = None;
        self.poke(now, trigger);
    }

    /// Schedules another snapshot soon, because the last one found nothing to read.
    pub fn retry(&mut self, now: Instant) {
        self.last_snapshot = None;
        self.pending = Some(Pending {
            burst: Burst::new(now, Trigger::Restructured),
            due: now + self.timing.content_settle,
            edited: false,
        });
    }

    /// The current shortest time between two content snapshots.
    #[cfg(test)]
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    /// The longest that changes or loading can postpone a snapshot.
    pub const fn max_wait(&self) -> Duration {
        self.timing.max_wait
    }

    /// Whether an edit waits to be read, whichever change of its burst came first.
    pub fn has_pending_edit(&self) -> bool {
        self.pending.is_some_and(|pending| pending.edited)
    }

    /// Forgets the pending snapshot and the snapshot history of the previous window.
    pub const fn reset(&mut self) {
        self.pending = None;
        self.last_snapshot = None;
        self.interval = self.timing.min_interval;
        self.poll = self.timing.poll_interval;
    }
}

/// Whether the trigger reports a change inside a window that was already captured.
const fn is_content(trigger: Trigger) -> bool {
    matches!(trigger, Trigger::Edited | Trigger::Restructured)
}

/// Whether the snapshot waits for the window to rest after its last read.
const fn rests(trigger: Trigger) -> bool {
    is_content(trigger) || matches!(trigger, Trigger::Retried)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tokio::time::Instant;

    use super::*;
    use crate::capture::tests::{MS, timing};

    #[rstest]
    #[case::activation_settles_quickly(&[(0, Trigger::Activated)], 400, Trigger::Activated)]
    #[case::content_settles_slowly(&[(0, Trigger::Edited)], 1500, Trigger::Edited)]
    #[case::a_rearrangement_settles_quickly(&[(0, Trigger::Restructured)], 400, Trigger::Restructured)]
    #[case::later_changes_postpone(&[(0, Trigger::Edited), (1000, Trigger::Edited)], 2500, Trigger::Edited)]
    #[case::first_trigger_wins(&[(0, Trigger::Activated), (100, Trigger::Edited)], 500, Trigger::Activated)]
    #[case::max_wait_bounds_postponement(
        &[(0, Trigger::Edited), (1400, Trigger::Edited), (2800, Trigger::Edited), (4200, Trigger::Edited)],
        5000,
        Trigger::Edited
    )]
    fn debounce_schedules_snapshots(
        timing: Timing,
        #[case] pokes: &[(u64, Trigger)],
        #[case] due_ms: u64,
        #[case] trigger: Trigger,
    ) {
        let start = Instant::now();
        let mut debounce = Debounce::new(timing);
        for &(at_ms, trigger) in pokes {
            debounce.poke(start + Duration::from_millis(at_ms), trigger);
        }
        let due = start + Duration::from_millis(due_ms);
        assert_eq!(debounce.due(), Some(due));
        assert_eq!(debounce.take(due), Some(trigger));
        assert_eq!(debounce.due(), None);
    }

    #[rstest]
    #[case::content_waits_for_the_interval(Trigger::Edited, 10_400)]
    #[case::navigation_is_immediate(Trigger::Loaded, 1400)]
    fn min_interval_applies_to_content_changes(
        timing: Timing,
        #[case] trigger: Trigger,
        #[case] due_ms: u32,
    ) {
        let start = Instant::now();
        let mut debounce = Debounce::new(timing);
        debounce.poke(start, Trigger::Activated);
        debounce.take(start + 400 * MS);

        debounce.poke(start + 1000 * MS, trigger);
        assert_eq!(debounce.due(), Some(start + due_ms * MS));
    }

    #[rstest]
    #[case::cheap_reads_keep_the_shortest_rest(20, 10_000)]
    #[case::costly_reads_rest_longer(1_000, 20_000)]
    #[case::the_rest_is_capped(5_000, 40_000)]
    fn the_rest_between_content_snapshots_follows_the_read_cost(
        timing: Timing,
        #[case] read_ms: u32,
        #[case] rest_ms: u32,
    ) {
        let mut debounce = Debounce::new(timing);
        debounce.paced(read_ms * MS);
        assert_eq!(debounce.interval(), rest_ms * MS);
    }

    #[rstest]
    fn a_quiet_window_is_polled_at_a_growing_interval(timing: Timing) {
        let start = Instant::now();
        let mut debounce = Debounce::new(Timing {
            poll_interval: Some(30_000 * MS),
            ..timing
        });
        // Nothing is polled before the first snapshot of a window.
        assert_eq!(debounce.due(), None);
        debounce.poke(start, Trigger::Activated);
        debounce.take(start + 400 * MS);

        assert_eq!(debounce.due(), Some(start + 30_400 * MS));
        assert_eq!(debounce.take(start + 20_000 * MS), None);
        assert_eq!(debounce.take(start + 30_400 * MS), Some(Trigger::Polled));
        debounce.observed(Trigger::Polled, Change::Unchanged);
        assert_eq!(debounce.due(), Some(start + 90_400 * MS));
        debounce.observed(Trigger::Polled, Change::Changed);
        assert_eq!(debounce.due(), Some(start + 60_400 * MS));
    }

    #[rstest]
    #[case::alone(&[Trigger::Edited], true)]
    #[case::after_a_rearrangement(&[Trigger::Restructured, Trigger::Edited], true)]
    #[case::before_a_rearrangement(&[Trigger::Edited, Trigger::Restructured], true)]
    #[case::none(&[Trigger::Restructured, Trigger::Loaded], false)]
    fn an_edit_anywhere_in_the_burst_waits_to_be_read(
        timing: Timing,
        #[case] pokes: &[Trigger],
        #[case] pending_edit: bool,
    ) {
        let start = Instant::now();
        let mut debounce = Debounce::new(timing);
        for (i, &trigger) in (0_u32..).zip(pokes) {
            debounce.poke(start + i * 100 * MS, trigger);
        }
        assert_eq!(debounce.has_pending_edit(), pending_edit);
        debounce.take(start + 10_000 * MS);
        assert!(!debounce.has_pending_edit());
    }

    #[rstest]
    fn retry_ignores_the_minimum_interval(timing: Timing) {
        let start = Instant::now();
        let mut debounce = Debounce::new(timing);
        debounce.poke(start, Trigger::Activated);
        debounce.take(start + 400 * MS);

        debounce.retry(start + 400 * MS);
        assert_eq!(debounce.due(), Some(start + 1900 * MS));
    }
}
