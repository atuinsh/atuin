//! The upload `atuin setup --hub` starts, as far as it has got: what the upload queue the daemon
//! drains has left of it.
//!
//! The count's total is what the first read left, grown only when more is queued than it counted,
//! so what's done never goes back. A backfill counts once the daemon has fixed where it stops,
//! which it does for this machine's threads as it first sees Atuin for Teams on.

use std::time::{Duration, Instant};

use atuin_common::path::AbsPathBuf;
use atuin_daemon::service::pidfile::Pidfile;
use atuin_octavo::hub::UserId;
use atuin_octavo::queue::{Backlog, QueueError, UploadQueue};

use super::count;

/// How long the backlog may sit unchanged before the screen says why it might be stuck.
const STALL_AFTER: Duration = Duration::from_secs(30);

/// What setup queued for the daemon to upload, to watch it go.
pub(super) struct Upload {
    queue: UploadQueue,
    user: UserId,
    pidfile: AbsPathBuf,
}

impl Upload {
    /// `user`'s upload in `queue`, sent by the daemon `pidfile` names.
    pub(super) const fn new(queue: UploadQueue, user: UserId, pidfile: AbsPathBuf) -> Self {
        Self {
            queue,
            user,
            pidfile,
        }
    }

    pub(super) async fn backlog(&self) -> Result<Backlog, QueueError> {
        self.queue.backlog(&self.user).await
    }

    /// Whether a daemon runs to upload it.
    pub(super) fn daemon_running(&self) -> bool {
        Pidfile::new(&self.pidfile).is_live()
    }
}

/// `done` of `total` up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Gauge {
    pub(super) done: u64,
    pub(super) total: u64,
}

impl Gauge {
    /// With `left` still to go.
    fn left(self, left: u64) -> Self {
        let total = self.total.max(self.done.saturating_add(left));
        Self {
            done: total.saturating_sub(left),
            total,
        }
    }

    const fn is_full(self) -> bool {
        self.done == self.total
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Uploading,
    /// The account has Atuin for Teams off, which pauses the upload until it's back on.
    Paused,
    /// Nothing has gone up for [`STALL_AFTER`]: the daemon is retrying, or isn't running.
    Stalled {
        daemon_running: bool,
    },
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Progress {
    /// Thread messages, `None` while the daemon has yet to count this machine's.
    pub(super) threads: Option<Gauge>,
    /// Thread messages this build can't read yet, left out of `threads`.
    pub(super) held: u64,
    pub(super) state: State,
}

impl Progress {
    /// What the summary says of the upload once setup leaves it.
    pub(super) fn summary(&self) -> String {
        if self.state == State::Done {
            let held = match self.held {
                0 => String::new(),
                1 => " but 1 thread message this Atuin can't read yet".into(),
                held => format!(" but {} thread messages this Atuin can't read yet", count(held)),
            };
            return format!("Uploaded everything here to Atuin for Teams{held}");
        }
        let Some(Gauge { done, total }) = self.threads.filter(|gauge| gauge.total > 0) else {
            return "Uploading to Atuin for Teams in the background".into();
        };
        let rest = if self.state == State::Paused {
            "the rest goes once your account has Atuin for Teams again"
        } else {
            "the rest uploading in the background"
        };
        format!("{} of {} thread messages up, {rest}", count(done), count(total))
    }
}

/// The thread messages `backlog` has left, or `None` while a backfill has yet to be fixed.
fn threads_left(backlog: &Backlog) -> Option<u64> {
    let walk: Option<u64> = backlog
        .backfills
        .iter()
        .map(|backfill| Some(backfill.until_idx?.saturating_sub(backfill.next_idx)))
        .sum();
    walk.map(|walk| walk.saturating_add(backlog.thread_messages))
}

/// Turns reads of the upload queue into [`Progress`].
#[derive(Debug, Default)]
pub(super) struct Tracker {
    threads: Option<Gauge>,
    /// What the last read left, and when that last changed.
    last: Option<(Option<u64>, Instant)>,
}

impl Tracker {
    /// How far the upload has got, as `backlog`, read at `now`, shows.
    pub(super) fn update(
        &mut self,
        backlog: &Backlog,
        daemon_running: bool,
        now: Instant,
    ) -> Progress {
        let left = threads_left(backlog);
        if let Some(threads) = left {
            self.threads = Some(self.threads.unwrap_or_default().left(threads));
        }
        let since = match self.last {
            Some((last, since)) if last == left => since,
            _ => now,
        };
        self.last = Some((left, since));

        let threads = left.and(self.threads);
        let state = if threads.is_some_and(Gauge::is_full) {
            State::Done
        } else if backlog.off {
            State::Paused
        } else if now.saturating_duration_since(since) >= STALL_AFTER {
            State::Stalled { daemon_running }
        } else {
            State::Uploading
        };
        Progress {
            threads,
            held: backlog.held_thread_messages,
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_domain::record::HostId;
    use atuin_octavo::queue::ThreadBackfill;
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn backlog(backfills: &[(u64, Option<u64>)], thread_messages: u64) -> Backlog {
        Backlog {
            thread_messages,
            held_thread_messages: 0,
            backfills: backfills
                .iter()
                .enumerate()
                .map(|(n, &(next_idx, until_idx))| ThreadBackfill {
                    host_id: HostId(uuid::Uuid::from_u128(n as u128)),
                    next_idx,
                    until_idx,
                })
                .collect(),
            off: false,
        }
    }

    const fn gauge(done: u64, total: u64) -> Gauge {
        Gauge { done, total }
    }

    #[rstest]
    fn counts_go_up_as_the_backlog_drains() {
        let (mut tracker, now) = (Tracker::default(), Instant::now());
        let first = tracker.update(&backlog(&[(0, Some(40))], 2), true, now);
        assert_eq!(first.threads, Some(gauge(0, 42)));

        let later = tracker.update(&backlog(&[(25, Some(40))], 0), true, now);
        assert_eq!(later.threads, Some(gauge(27, 42)));
        assert_eq!(later.state, State::Uploading);
    }

    #[rstest]
    fn threads_count_once_every_backfill_is_fixed() {
        let (mut tracker, now) = (Tracker::default(), Instant::now());
        let counting = tracker.update(&backlog(&[(0, Some(10)), (0, None)], 0), true, now);
        assert_eq!((counting.threads, counting.state), (None, State::Uploading));

        let fixed = tracker.update(&backlog(&[(5, Some(10)), (0, Some(20))], 0), true, now);
        assert_eq!(fixed.threads, Some(gauge(0, 25)));
    }

    #[rstest]
    fn more_queued_grows_the_total_without_taking_back_what_went_up() {
        let (mut tracker, now) = (Tracker::default(), Instant::now());
        tracker.update(&backlog(&[], 10), true, now);
        tracker.update(&backlog(&[], 4), true, now);
        let grown = tracker.update(&backlog(&[], 9), true, now);
        assert_eq!(grown.threads, Some(gauge(6, 15)));
    }

    #[rstest]
    #[case::drained(backlog(&[(40, Some(40))], 0), true, State::Done)]
    #[case::off(Backlog { off: true, ..backlog(&[], 5) }, true, State::Paused)]
    #[case::uploading(backlog(&[], 5), true, State::Uploading)]
    fn state_of_the_first_read(
        #[case] backlog: Backlog,
        #[case] daemon_running: bool,
        #[case] state: State,
    ) {
        let progress = Tracker::default().update(&backlog, daemon_running, Instant::now());
        assert_eq!(progress.state, state);
    }

    #[rstest]
    #[case::retrying(true)]
    #[case::no_daemon(false)]
    fn a_backlog_that_sits_still_stalls(#[case] daemon_running: bool) {
        let (mut tracker, start) = (Tracker::default(), Instant::now());
        let stuck = backlog(&[], 5);
        tracker.update(&stuck, daemon_running, start);
        let before = tracker.update(&stuck, daemon_running, start + STALL_AFTER / 2);
        assert_eq!(before.state, State::Uploading);

        let after = tracker.update(&stuck, daemon_running, start + STALL_AFTER);
        assert_eq!(after.state, State::Stalled { daemon_running });

        let moving = tracker.update(&backlog(&[], 4), daemon_running, start + STALL_AFTER * 2);
        assert_eq!(moving.state, State::Uploading);
    }

    #[rstest]
    #[case::done(Some(gauge(3, 3)), 0, State::Done, "Uploaded everything here to Atuin for Teams")]
    #[case::done_but_held(
        Some(gauge(3, 3)),
        2,
        State::Done,
        "Uploaded everything here to Atuin for Teams but 2 thread messages this Atuin can't read \
         yet"
    )]
    #[case::uploading(
        Some(gauge(830, 4_100)),
        0,
        State::Uploading,
        "830 of 4,100 thread messages up, the rest uploading in the background"
    )]
    #[case::paused(
        Some(gauge(1, 9)),
        0,
        State::Paused,
        "1 of 9 thread messages up, the rest goes once your account has Atuin for Teams again"
    )]
    #[case::counting(
        None,
        0,
        State::Stalled { daemon_running: false },
        "Uploading to Atuin for Teams in the background"
    )]
    fn summary_says_how_far_it_got(
        #[case] threads: Option<Gauge>,
        #[case] held: u64,
        #[case] state: State,
        #[case] expected: &str,
    ) {
        let progress = Progress {
            threads,
            held,
            state,
        };
        assert_eq!(progress.summary(), expected);
    }

    proptest! {
        /// Whatever the queue does between reads, what's done never goes back, never passes the
        /// total, and is the total only once nothing is left.
        #[test]
        fn done_only_moves_forward(lefts in prop::collection::vec(0..1_000u64, 1..50)) {
            let mut gauge = Gauge::default();
            for left in lefts {
                let next = gauge.left(left);
                prop_assert!(next.done >= gauge.done);
                prop_assert!(next.done <= next.total);
                prop_assert_eq!(next.is_full(), left == 0);
                gauge = next;
            }
        }
    }
}
