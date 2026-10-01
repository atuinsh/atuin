//! A chaos test of the recovery protocol on the real components: an in-memory record store and
//! sidecar, the real coordinator, capture's sink, the sync worker's reprojection and the gRPC
//! rebuild, on a multi-thread runtime. Test hooks (see [`super::hooks`]) delay every step of the
//! protocol at random, fail wipes, fail, panic or give up replays, panic captures, fail the
//! warm-up's reads, drop appends at random points, and storm replays and sync
//! reprojections with invalidations (toward their pass cap; at startup too, and with rebuilds
//! and forgets landing in the backoffs between incomplete replays), while rebuilds, captures (of
//! lines already persisted and new ones, and of a resumed transcript warmed from the sidecar),
//! sync reprojections, rewrites of this host's and another's record series under their
//! watermarks, and reads run at once. Once it quiesces, it checks the protocol's invariants.
//!
//! `ATUIN_CHAOS_ITERATIONS` sets how many seeds to run: by default 100, a few seconds in all, as
//! each takes some 50ms of in-memory SQLite.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use atuin_client::ai_session::{NativeSessionId, SourceId};
use atuin_common::harnesstools::session::{AnyMessage, SessionId};
use atuin_domain::record::{RecordId, RecordTag};
use futures::future::BoxFuture;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rstest::rstest;
use time::OffsetDateTime;
use tonic::Request;

use super::engine::{Start, warm};
use super::hooks::{Fault, Hooks, INJECTED_PANIC, Point, Section};
use super::message_enricher::MessageEnricher;
use super::recovery::Backoff;
use super::*;
use crate::grpc::ai::session::pb::ai_session_server::AiSession as _;
use crate::grpc::ai::session::pb::{ListSessionsRequest, RebuildSessionsRequest};
use crate::grpc::ai::session::{Service, is_rebuilding};
use crate::sync::spawn_ai_session_projector;

const TIMEOUT: Duration = Duration::from_secs(20);

/// Counts, over all iterations, of what the chaos made happen: to tell it did.
#[derive(Debug, Default)]
struct Totals {
    replays: AtomicU64,
    replay_panics: AtomicU64,
    replay_failures: AtomicU64,
    wipes: AtomicU64,
    wipe_failures: AtomicU64,
    captured: AtomicU64,
    ended_unrecoverable: AtomicU64,
    /// Captures a rebuild overtook between their wait and their lock.
    captures_overtaken: AtomicU64,
    /// This host's record series rewritten under capture.
    local_rewrites: AtomicU64,
    /// The other host's.
    remote_rewrites: AtomicU64,
    /// Series the sync worker found it must not forget beside capture, handed to the coordinator.
    forgets_held_off: AtomicU64,
    /// Invalidation storms, over a replay or a sync reprojection: clearing the watermarks over
    /// and over, or rewriting the other host's series over and over.
    storms: AtomicU64,
    rewriting_storms: AtomicU64,
    /// Replays that gave up, invalidated every pass (the cap itself, not injected).
    replays_incomplete: AtomicU64,
    /// Replays made to give up as incomplete by a fault.
    injected_incomplete: AtomicU64,
    /// Sync reprojections that gave up, invalidated every pass.
    syncs_incomplete: AtomicU64,
    /// Storms over startup recovery, stopping on their own; and the replays they made give up,
    /// every one while they lasted.
    startup_storms: AtomicU64,
    startup_storm_incomplete: AtomicU64,
    /// Backoffs the coordinator began waiting out before replaying again, after an incomplete
    /// replay; and those a wipe cut short.
    backoffs: AtomicU64,
    backoffs_cut_short: AtomicU64,
    /// Rebuilds, and forgets of this host's sessions, asked for during a backoff.
    rebuilds_in_backoff: AtomicU64,
    forgets_in_backoff: AtomicU64,
    /// Warm-ups of the resumed transcript that read the sidecar, those begun while the store was
    /// recovering (a wipe not replayed yet: they wait for the replay), and its lines captured new.
    warm_ups: AtomicU64,
    warm_ups_recovering: AtomicU64,
    resumed_captured: AtomicU64,
    /// Appends dropped at a random point (as the import RPC drops its stream's on a disconnect)
    /// and captured again; those whose record was stored by the time they were dropped (or just
    /// after, by the rest of their capture running on); those whose line was then a duplicate
    /// (the dropped one had pushed it, or another capture had); and captures panicked before or
    /// after their push.
    appends_dropped: AtomicU64,
    dropped_once_pushed: AtomicU64,
    dropped_then_duplicate: AtomicU64,
    capture_panics: AtomicU64,
    /// Warm-up steps (its repair or a read) failed, which the warm-up retries.
    warm_read_failures: AtomicU64,
}

/// How the last replay to end did, as the chaos made it (see [`Chaos::last_outcome`]).
const OUTCOME_NONE: u8 = 0;
const OUTCOME_OK: u8 = 1;
const OUTCOME_INCOMPLETE: u8 = 2;
const OUTCOME_FAILED: u8 = 3;
const OUTCOME_PANICKED: u8 = 4;

#[derive(Debug)]
struct Chaos {
    rng: parking_lot::Mutex<StdRng>,
    /// Whether to fail and panic things, besides delaying them.
    faults: AtomicBool,
    /// How many replays a storm over startup recovery still makes give up as incomplete: every
    /// one, while it lasts.
    relentless: Arc<AtomicU32>,
    replays: Arc<AtomicUsize>,
    wipes: Arc<AtomicUsize>,
    captures: Arc<AtomicUsize>,
    violations: parking_lot::Mutex<Vec<String>>,
    totals: Arc<Totals>,
    probe: parking_lot::Mutex<Option<Probe>>,
    /// Storms still running.
    storming: Arc<AtomicUsize>,
    /// Wipes that went ahead.
    emptied: Arc<AtomicU64>,
    /// How many wipes had gone ahead when the last replay to end began.
    last_replayed_from: Arc<AtomicU64>,
    /// How the replay running is ending, and how the last one to end did: an `OUTCOME_*`.
    outcome: Arc<AtomicU8>,
    last_outcome: Arc<AtomicU8>,
    /// The coordinator's mailbox, to ask for rebuilds and forgets during backoffs, and how many
    /// such requests are still on their way.
    coordinator: parking_lot::Mutex<Option<mpsc::WeakUnboundedSender<Msg>>>,
    requests: Arc<AtomicUsize>,
}

/// What [`Chaos::check_complete`] looks at.
#[derive(Debug, Clone)]
struct Probe {
    sidecar: AiSessionDatabase,
    host: HostId,
    /// The record store, its key, and the other host, for storms rewriting its series. Its
    /// writes take `remote_writes`.
    store: SqliteStore,
    key: Key,
    remote: HostId,
    remote_writes: Arc<tokio::sync::Mutex<()>>,
    /// This host's lines pushed to the record store: those persisted, and each capture reported
    /// new. A line pushed twice (the dedup gate trusting a sidecar missing it) is reported new
    /// twice.
    pushed: Arc<parking_lot::Mutex<Vec<LineKey>>>,
}

/// Leaves a section when dropped (a panic unwinding included).
struct Leave {
    counter: Arc<AtomicUsize>,
    /// A replay's: where to say, when it ends, how many wipes had gone ahead when it began.
    replayed_from: Option<(Arc<AtomicU64>, u64)>,
    /// A storm of invalidations over a replay, to stop.
    storm: Option<Arc<AtomicBool>>,
    /// A replay's: how it ended, and where to say so.
    outcome: Option<(Arc<AtomicU8>, Arc<AtomicU8>)>,
}

impl Drop for Leave {
    fn drop(&mut self) {
        if let Some(storm) = &self.storm {
            storm.store(true, Ordering::SeqCst);
        }
        if let Some((last, from)) = &self.replayed_from {
            last.store(*from, Ordering::SeqCst);
        }
        if let Some((outcome, last)) = &self.outcome {
            last.store(outcome.load(Ordering::SeqCst), Ordering::SeqCst);
        }
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Invalidate over and over until `stop`, so that a reprojection meanwhile starts over pass
/// after pass, toward its cap: clear the sidecar's watermarks (deleting no row), or, if
/// `rewriting`, rewrite the other host's series, which a reprojection forgets on finding it, as
/// one does when a sync downloads it rewritten. Forgetting it deletes the sessions it added to,
/// this host's rows in them included, and has the pass go round again for them.
fn storm(probe: Probe, rewriting: bool, stop: Arc<AtomicBool>, storming: Arc<AtomicUsize>) {
    storming.fetch_add(1, Ordering::SeqCst);
    tokio::spawn(async move {
        let _done = Leave {
            counter: storming,
            replayed_from: None,
            storm: None,
            outcome: None,
        };
        while !stop.load(Ordering::SeqCst) {
            if rewriting {
                let _writes = probe.remote_writes.lock().await;
                super::tests::rewrite_series(&probe.store, &probe.key, probe.remote).await;
            } else {
                probe.sidecar.clear_reproject_watermarks().await.unwrap();
            }
            tokio::task::yield_now().await;
        }
    });
}

/// No replay has ended yet.
const NONE_REPLAYED: u64 = u64::MAX;

impl Chaos {
    fn new(seed: u64, totals: Arc<Totals>) -> Self {
        Self {
            rng: parking_lot::Mutex::new(StdRng::seed_from_u64(seed)),
            faults: AtomicBool::new(true),
            relentless: Arc::default(),
            replays: Arc::default(),
            wipes: Arc::default(),
            captures: Arc::default(),
            violations: parking_lot::Mutex::default(),
            totals,
            probe: parking_lot::Mutex::default(),
            storming: Arc::default(),
            emptied: Arc::default(),
            last_replayed_from: Arc::new(AtomicU64::new(NONE_REPLAYED)),
            outcome: Arc::new(AtomicU8::new(OUTCOME_NONE)),
            last_outcome: Arc::new(AtomicU8::new(OUTCOME_NONE)),
            coordinator: parking_lot::Mutex::default(),
            requests: Arc::default(),
        }
    }

    /// Ask the coordinator, a moment from now, for a rebuild or (as the sync worker does) a
    /// forget of this host's sessions: to land during the backoff just begun.
    fn request_in_backoff(&self) {
        let (Some(coordinator), Some(probe)) = (
            self.coordinator.lock().as_ref().and_then(mpsc::WeakUnboundedSender::upgrade),
            self.probe.lock().clone(),
        ) else {
            return;
        };
        let (forget, ms) = {
            let mut rng = self.rng.lock();
            (rng.gen_bool(0.5), rng.gen_range(0..4))
        };
        let asked = if forget {
            &self.totals.forgets_in_backoff
        } else {
            &self.totals.rebuilds_in_backoff
        };
        asked.fetch_add(1, Ordering::Relaxed);
        let requests = self.requests.clone();
        requests.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            let _done = Leave {
                counter: requests,
                replayed_from: None,
                storm: None,
                outcome: None,
            };
            tokio::time::sleep(Duration::from_millis(ms)).await;
            let (reply, answer) = oneshot::channel();
            let msg = if forget {
                Msg::SeriesRewritten {
                    host: probe.host,
                    reply,
                }
            } else {
                Msg::Rebuild(reply)
            };
            if coordinator.send(msg).is_ok() {
                drop(coordinator);
                let _ = answer.await;
            }
        });
    }

    /// I1 where it matters: the sidecar holds every line of this host pushed so far. Called
    /// holding capture's lock with the store found ready, where capture trusts the sidecar.
    async fn check_complete(&self, when: &str) {
        let Some(probe) = self.probe.lock().clone() else {
            return;
        };
        let held = keys(&probe.sidecar, Some(probe.host)).await;
        let missing: Vec<_> =
            probe.pushed.lock().iter().filter(|key| !held.contains(*key)).cloned().collect();
        if !missing.is_empty() {
            self.violation(format!("{when} while it was missing {missing:?}"));
        }
    }

    /// Watch the store's state: it settles only on a replay that began after the last wipe (the
    /// last replay to end, as one runs at a time and the coordinator settles only once it has),
    /// ready only on one that succeeded, and unavailable only on one that failed or panicked,
    /// never on one that gave up as incomplete (that one is replayed again). A settled state
    /// read with no wipe or replay ending around the read is checked.
    async fn watch_settling(&self, mut state: watch::Receiver<StoreState>) {
        while state.changed().await.is_ok() {
            let before = self.settling();
            let now = *state.borrow_and_update();
            if now == StoreState::Recovering || self.settling() != before {
                continue;
            }
            let (emptied, replayed_from, outcome) = before;
            if replayed_from != NONE_REPLAYED && replayed_from != emptied {
                self.violation(format!(
                    "{now:?} on a replay begun after {replayed_from} wipes, of {emptied}"
                ));
            }
            self.check_outcome(now, outcome);
        }
    }

    /// The store settled `state` on a replay that ended `outcome`.
    fn check_outcome(&self, state: StoreState, outcome: u8) {
        let expected: &[u8] = match state {
            StoreState::Ready => &[OUTCOME_OK],
            StoreState::Unavailable => &[OUTCOME_FAILED, OUTCOME_PANICKED],
            StoreState::Recovering => return,
        };
        if !expected.contains(&outcome) {
            self.violation(format!("{state:?} on a replay that ended {outcome}"));
        }
    }

    fn settling(&self) -> (u64, u64, u8) {
        (
            self.emptied.load(Ordering::SeqCst),
            self.last_replayed_from.load(Ordering::SeqCst),
            self.last_outcome.load(Ordering::SeqCst),
        )
    }

    fn violation(&self, what: String) {
        self.violations.lock().push(what);
    }

    /// Roll a die: true with probability `p`, when faults are on.
    fn fault(&self, p: f64) -> bool {
        self.faults.load(Ordering::SeqCst) && self.rng.lock().gen_bool(p)
    }

    /// A random pause of up to some milliseconds, to spread rebuilds, syncs and reads over the
    /// time capture runs.
    async fn pause(&self) {
        let ms = self.rng.lock().gen_range(0..12);
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    /// A random delay: nothing, a few yields, or (rarely) a timer tick.
    async fn delay(&self) {
        let (kind, n) = {
            let mut rng = self.rng.lock();
            (rng.gen_range(0..100), rng.gen_range(1..8))
        };
        match kind {
            0..40 => {}
            40..97 => {
                for _ in 0..n {
                    tokio::task::yield_now().await;
                }
            }
            _ => tokio::time::sleep(Duration::from_micros(200)).await,
        }
    }
}

impl Hooks for Chaos {
    fn at(&self, point: Point) -> BoxFuture<'_, Fault> {
        Box::pin(async move {
            self.delay().await;
            // Widen the windows a rebuild could land in.
            if matches!(point, Point::CaptureWaited | Point::BeforeWipe)
                && self.rng.lock().gen_bool(0.3)
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            match point {
                Point::WipeLocked if self.fault(0.1) => {
                    self.totals.wipe_failures.fetch_add(1, Ordering::Relaxed);
                    Fault::Fail
                }
                Point::WipeLocked => {
                    // Said before it happens, but under capture's lock and while recovering.
                    self.emptied.fetch_add(1, Ordering::SeqCst);
                    Fault::None
                }
                Point::ReplayBeforeSettle
                    if self
                        .relentless
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                        .is_ok() =>
                {
                    self.totals.startup_storm_incomplete.fetch_add(1, Ordering::Relaxed);
                    self.outcome.store(OUTCOME_INCOMPLETE, Ordering::SeqCst);
                    Fault::Incomplete
                }
                Point::ReplayBeforeSettle if self.fault(0.1) => {
                    self.totals.replay_panics.fetch_add(1, Ordering::Relaxed);
                    self.outcome.store(OUTCOME_PANICKED, Ordering::SeqCst);
                    Fault::Panic
                }
                Point::ReplayBeforeSettle if self.fault(0.1) => {
                    self.totals.replay_failures.fetch_add(1, Ordering::Relaxed);
                    self.outcome.store(OUTCOME_FAILED, Ordering::SeqCst);
                    Fault::Fail
                }
                Point::ReplayBeforeSettle if self.fault(0.05) => {
                    self.totals.injected_incomplete.fetch_add(1, Ordering::Relaxed);
                    self.outcome.store(OUTCOME_INCOMPLETE, Ordering::SeqCst);
                    Fault::Incomplete
                }
                Point::ReplayIncomplete => {
                    self.totals.replays_incomplete.fetch_add(1, Ordering::Relaxed);
                    self.outcome.store(OUTCOME_INCOMPLETE, Ordering::SeqCst);
                    Fault::None
                }
                Point::BackoffStarted => {
                    self.totals.backoffs.fetch_add(1, Ordering::Relaxed);
                    if self.fault(0.5) {
                        self.request_in_backoff();
                    }
                    Fault::None
                }
                Point::BackoffCutShort => {
                    self.totals.backoffs_cut_short.fetch_add(1, Ordering::Relaxed);
                    Fault::None
                }
                Point::WarmRead(_) if self.fault(0.1) => {
                    self.totals.warm_read_failures.fetch_add(1, Ordering::Relaxed);
                    Fault::Fail
                }
                Point::CapturePushing | Point::CapturePushed if self.fault(0.1) => {
                    self.totals.capture_panics.fetch_add(1, Ordering::Relaxed);
                    Fault::Panic
                }
                Point::WarmChecked => {
                    self.totals.warm_ups.fetch_add(1, Ordering::Relaxed);
                    // Where the warm-up trusts the sidecar, as capture's dedup gate does.
                    self.check_complete("a warm-up read the sidecar").await;
                    Fault::None
                }
                Point::SyncForgetHeldOff => {
                    self.totals.forgets_held_off.fetch_add(1, Ordering::Relaxed);
                    Fault::None
                }
                Point::SyncIncomplete => {
                    self.totals.syncs_incomplete.fetch_add(1, Ordering::Relaxed);
                    Fault::None
                }
                Point::SyncReproject if self.fault(0.2) => {
                    // A storm over (some of) the sync worker's reprojection.
                    if let Some(stop) = self.storm() {
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(3)).await;
                            stop.store(true, Ordering::SeqCst);
                        });
                    }
                    Fault::None
                }
                Point::CaptureOvertaken => {
                    self.totals.captures_overtaken.fetch_add(1, Ordering::Relaxed);
                    Fault::None
                }
                Point::CaptureChecked => {
                    self.check_complete("capture checked the sidecar").await;
                    Fault::None
                }
                _ => Fault::None,
            }
        })
    }

    fn enter(&self, section: Section) -> Box<dyn Send> {
        let counter = match section {
            Section::Replay => {
                self.totals.replays.fetch_add(1, Ordering::Relaxed);
                &self.replays
            }
            Section::Wipe => {
                self.totals.wipes.fetch_add(1, Ordering::Relaxed);
                &self.wipes
            }
            Section::Capture => &self.captures,
        };
        let now = counter.fetch_add(1, Ordering::SeqCst) + 1;
        if matches!(section, Section::Replay) && now > 1 {
            self.violation(format!("{now} replays at once"));
        }
        if self.wipes.load(Ordering::SeqCst) > 0 && self.captures.load(Ordering::SeqCst) > 0 {
            self.violation("a capture between its check and its push during a wipe".to_owned());
        }
        let replayed_from = matches!(section, Section::Replay)
            .then(|| (self.last_replayed_from.clone(), self.emptied.load(Ordering::SeqCst)));
        let outcome = matches!(section, Section::Replay).then(|| {
            self.outcome.store(OUTCOME_OK, Ordering::SeqCst);
            (self.outcome.clone(), self.last_outcome.clone())
        });
        let storm = if matches!(section, Section::Replay) && self.fault(0.1) {
            self.storm()
        } else {
            None
        };
        Box::new(Leave {
            counter: counter.clone(),
            replayed_from,
            storm,
            outcome,
        })
    }

    /// Short, so storms of incomplete replays back off often within an iteration, but long
    /// enough for rebuilds and forgets to land during the backoffs.
    fn warm_backoff(&self) -> Backoff {
        Backoff {
            first: Duration::from_millis(1),
            max: Duration::from_millis(8),
        }
    }

    fn backoff(&self) -> Backoff {
        Backoff {
            first: Duration::from_millis(4),
            max: Duration::from_millis(32),
        }
    }
}

impl Chaos {
    /// Start a [`storm`] of either kind, returning its stop.
    fn storm(&self) -> Option<Arc<AtomicBool>> {
        let probe = self.probe.lock().clone()?;
        let rewriting = self.rng.lock().gen_bool(0.5);
        let started = if rewriting {
            &self.totals.rewriting_storms
        } else {
            &self.totals.storms
        };
        started.fetch_add(1, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        storm(probe, rewriting, stop.clone(), self.storming.clone());
        Some(stop)
    }
}

impl Chaos {
    /// The listener's append of `msg`, which the chaos may drop at a random point: waiting for
    /// the store or capture's locks, or in the middle of its push and projection. A dropped (or
    /// panicked) append is captured again, as a re-read of the line captures it: under a new
    /// record id. Also says whether one was.
    async fn append(&self, sink: &Sink, msg: Message) -> (Result<Appended, AppendError>, bool) {
        let mut again = false;
        loop {
            let mut attempt = msg.clone();
            if again {
                attempt.id = RecordId(atuin_common::utils::uuid_v7());
            }
            let id = attempt.id;
            let cut = self.fault(0.5).then(|| {
                let mut rng = self.rng.lock();
                (rng.gen_range(0..3), rng.gen_range(1..100))
            });
            let appended = engine::append(sink, attempt);
            let appended = match cut {
                None => Some(appended.await),
                // At a random point of the runtime's.
                Some((0, yields)) => {
                    let cut = async {
                        for _ in 0..yields {
                            tokio::task::yield_now().await;
                        }
                    };
                    tokio::select! {
                        appended = appended => Some(appended),
                        () = cut => None,
                    }
                }
                // At a random point of its own: woken for the `nth` time, before it observes the
                // await that just completed.
                Some((1, nth)) => drop_at_wake(appended, DropAt::Wake(nth)).await,
                // Woken once its record is stored, before it observes so: as the import RPC's
                // stream is dropped, the push it was awaiting done, on a client's disconnect.
                Some(_) => drop_at_wake(appended, DropAt::Stored(&sink.records, id)).await,
            };
            let Some(appended) = appended else {
                self.totals.appends_dropped.fetch_add(1, Ordering::Relaxed);
                if sink.records.holds(id).await.unwrap_or(false) {
                    self.totals.dropped_once_pushed.fetch_add(1, Ordering::Relaxed);
                }
                again = true;
                continue;
            };
            match appended {
                // An injected panic (see `Point::CapturePushed`).
                Err(AppendError::Aborted) => again = true,
                appended => {
                    if again && matches!(appended, Ok(Appended::Duplicate)) {
                        self.totals.dropped_then_duplicate.fetch_add(1, Ordering::Relaxed);
                    }
                    return (appended, again);
                }
            }
        }
    }
}

/// Says when the future polled with it was woken, and wakes the task polling that future.
#[derive(Debug, Default)]
struct Wakes {
    woken: AtomicBool,
    outer: parking_lot::Mutex<Option<std::task::Waker>>,
}

impl std::task::Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::SeqCst);
        if let Some(outer) = self.outer.lock().as_ref() {
            outer.wake_by_ref();
        }
    }
}

/// Which wake of a future [`drop_at_wake`] drops it at (past its first poll).
enum DropAt<'a> {
    /// The `nth`.
    Wake(usize),
    /// The first once the record store holds the record with this id.
    Stored(&'a AiSessionStore, RecordId),
}

/// Run `inner`, but drop it at a wake (see [`DropAt`]), before polling it again: just after
/// something it awaits completed (a query, a lock, a task), and before it observed so.
async fn drop_at_wake<F: Future>(inner: F, mut at: DropAt<'_>) -> Option<F::Output> {
    use std::task::{Context, Poll, Waker};

    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut inner = std::pin::pin!(inner);
    let mut first = true;
    loop {
        if !first {
            std::future::poll_fn(|cx| {
                *wakes.outer.lock() = Some(cx.waker().clone());
                if wakes.woken.swap(false, Ordering::SeqCst) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            let now = match &mut at {
                DropAt::Wake(nth) => {
                    *nth = nth.saturating_sub(1);
                    *nth == 0
                }
                DropAt::Stored(records, id) => records.holds(*id).await.unwrap_or(false),
            };
            if now {
                return None;
            }
        }
        first = false;
        if let Poll::Ready(output) = inner.as_mut().poll(&mut Context::from_waker(&waker)) {
            return Some(output);
        }
    }
}

/// Note a line of this host captured: reported new, or a duplicate after a dropped append,
/// which may have pushed it (and so may not have been reported new).
fn note_pushed(pushed: &parking_lot::Mutex<Vec<LineKey>>, key: LineKey, new: bool) {
    let mut pushed = pushed.lock();
    if new || !pushed.contains(&key) {
        pushed.push(key);
    }
}

fn line(session: &str, source: &str) -> Message {
    Message::builder()
        .id(RecordId(atuin_common::utils::uuid_v7()))
        .session(HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from(session.to_owned()),
        })
        .source_id(SourceId::from(source.to_owned()))
        .timestamp(OffsetDateTime::UNIX_EPOCH)
        .role(Role::User)
        .content(vec![Content::Text("hello".to_owned())])
        .build()
}

/// An id-less Codex prompt: identical lines of it are told apart by their ordinal alone, which
/// a resumed transcript's warm-up reads back from the sidecar.
fn prompt() -> AnyMessage {
    AnyMessage::Codex(
        serde_json::from_value(serde_json::json!({
            "type": "response_item", "timestamp": "2026-09-18T10:00:00.123Z",
            "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": "continue"}]},
        }))
        .unwrap(),
    )
}

/// The resumed transcript's session.
const RESUMED: &str = "w";

type LineKey = (String, String);

fn key_of(msg: &Message) -> LineKey {
    (msg.session.session.to_string(), msg.source_id.to_string())
}

/// Every message key the sidecar holds, of `host` only if given.
async fn keys(db: &AiSessionDatabase, host: Option<HostId>) -> BTreeSet<LineKey> {
    let mut keys = BTreeSet::new();
    for session in db.list_sessions(&SessionFilter::default()).await.unwrap() {
        let mut messages = Box::pin(db.messages(&session.handle));
        while let Some(msg) = messages.next().await {
            let msg = msg.unwrap();
            if host.is_none() || msg.host == host {
                keys.insert((session.handle.session.to_string(), msg.source_id.to_string()));
            }
        }
    }
    keys
}

/// What the record store holds, projected afresh.
async fn stored(records: &AiSessionStore) -> AiSessionDatabase {
    let fresh = AiSessionDatabase::in_memory().await.unwrap();
    records.build(&fresh).await.unwrap();
    fresh
}

async fn iteration(seed: u64, totals: Arc<Totals>) {
    let mut rng = StdRng::seed_from_u64(seed ^ 0x5eed);
    let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
    let key = Key::generate();
    let local_host = HostId(atuin_common::utils::uuid_v7());
    let records =
        AiSessionStore::builder().store(store.clone()).host_id(local_host).key(key.clone()).build();
    // Another host's records, as a sync downloads them.
    let remote_host = HostId(atuin_common::utils::uuid_v7());
    let remote = AiSessionStore::builder()
        .store(store.clone())
        .host_id(remote_host)
        .key(key.clone())
        .build();
    let sidecar = AiSessionDatabase::in_memory().await.unwrap();

    // Lines already persisted, which capture reads again.
    let persisted: Vec<_> =
        (0..3).map(|i| line(&format!("s{}", i % 2), &format!("p{i}"))).collect();
    for msg in &persisted {
        records.push(msg).await.unwrap();
    }
    remote.push(&line("r", "r0")).await.unwrap();
    // A transcript of identical id-less prompts, two read before the daemon (re)started.
    let resumed = SessionId::from(RESUMED.to_owned());
    let mut before = MessageEnricher::new(HarnessKind::Codex);
    let mut persisted_resumed = Vec::new();
    for _ in 0..2 {
        for msg in before.capture(&resumed, &prompt()) {
            records.push(&msg).await.unwrap();
            persisted_resumed.push(key_of(&msg));
        }
    }

    let pushed = Arc::new(parking_lot::Mutex::new(
        persisted.iter().map(key_of).chain(persisted_resumed.iter().cloned()).collect(),
    ));
    let chaos = Arc::new(Chaos::new(seed, totals.clone()));
    let remote_writes = Arc::new(tokio::sync::Mutex::new(()));
    *chaos.probe.lock() = Some(Probe {
        sidecar: sidecar.clone(),
        host: local_host,
        store: store.clone(),
        key: key.clone(),
        remote: remote_host,
        remote_writes: remote_writes.clone(),
        pushed: pushed.clone(),
    });
    // A storm of invalidations over startup recovery (an invalidation burst at startup), which
    // stops on its own: every replay meanwhile gives up, some of them for real (the sidecar's
    // watermarks cleared, or the other host's series rewritten, over and over). Recovery replays
    // through it, recovering, backing off longer each time, and settles once it is over: never
    // unavailable for it (see `Chaos::check_outcome`), which at startup would last until restart.
    if rng.gen_bool(0.3)
        && let Some(stop) = chaos.storm()
    {
        chaos.totals.startup_storms.fetch_add(1, Ordering::Relaxed);
        chaos.relentless.store(rng.gen_range(1..=6), Ordering::SeqCst);
        let relentless = chaos.relentless.clone();
        tokio::spawn(async move {
            while relentless.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            stop.store(true, Ordering::SeqCst);
        });
    }
    let capture = Arc::new(AiHarnessSessionCapture::open_with(
        records.clone(),
        sidecar.clone(),
        false,
        BlockingPool::new(NonZeroUsize::MIN),
        Some(chaos.clone() as Arc<dyn Hooks>),
    ));
    *chaos.coordinator.lock() = capture.coordinator.as_ref().map(mpsc::UnboundedSender::downgrade);
    let watching = tokio::spawn({
        let (chaos, state) = (chaos.clone(), capture.state.clone());
        async move { chaos.watch_settling(state).await }
    });
    let service = Service::new(capture.clone());
    let projector =
        spawn_ai_session_projector(records.clone(), sidecar.clone(), capture.recovery());

    let mut tasks = tokio::task::JoinSet::new();
    // Rebuilds, through gRPC: during startup recovery or another rebuild, or (mostly) once the
    // store settled, while capture runs.
    for _ in 0..rng.gen_range(1..=3) {
        let (service, chaos, capture) = (service.clone(), chaos.clone(), capture.clone());
        let (count, settled_first) = (rng.gen_range(1..=2), rng.gen_bool(0.7));
        tasks.spawn(async move {
            for _ in 0..count {
                if settled_first {
                    capture.ready().await;
                }
                chaos.pause().await;
                if let Err(status) =
                    service.rebuild_sessions(Request::new(RebuildSessionsRequest {})).await
                    && !matches!(
                        status.code(),
                        tonic::Code::FailedPrecondition | tonic::Code::Internal
                    )
                {
                    chaos.violation(format!("rebuild answered {status:?}"));
                }
            }
        });
    }
    // Sync downloads, and the sync worker reprojecting them. The other host adds to one of this
    // host's sessions sometimes: forgetting it then deletes rows of this host.
    let shared = rng.gen_bool(0.5);
    {
        let (remote, chaos, projector) = (remote.clone(), chaos.clone(), projector.clone());
        let remote_writes = remote_writes.clone();
        tasks.spawn(async move {
            for i in 1..3 {
                chaos.pause().await;
                let session = if shared && i == 1 {
                    "s0"
                } else {
                    "r"
                };
                {
                    let _writes = remote_writes.lock().await;
                    remote.push(&line(session, &format!("r{i}"))).await.unwrap();
                }
                projector.send(()).unwrap();
            }
        });
    }
    // Series rewritten under their watermarks, as a sync finds them: this host's (which the
    // sync worker must leave to the coordinator), and the other host's. Each lands at once as
    // far as projecting goes: under capture's lock, which projecting this host's series takes.
    for _ in 0..rng.gen_range(0..=2) {
        let (store, key, chaos, projector, sidecar) =
            (store.clone(), key.clone(), chaos.clone(), projector.clone(), sidecar.clone());
        let (host, settled_first) = if rng.gen_bool(0.7) {
            (local_host, rng.gen_bool(0.8))
        } else {
            (remote_host, rng.gen_bool(0.8))
        };
        let (capture, remote_writes) = (capture.clone(), remote_writes.clone());
        tasks.spawn(async move {
            if settled_first {
                capture.ready().await;
            }
            chaos.pause().await;
            {
                let _writes = remote_writes.lock().await;
                let _local = sidecar.lock_local_projection().await;
                super::tests::rewrite_series(&store, &key, host).await;
            }
            let rewrites = if host == local_host {
                &chaos.totals.local_rewrites
            } else {
                &chaos.totals.remote_rewrites
            };
            rewrites.fetch_add(1, Ordering::Relaxed);
            projector.send(()).unwrap();
        });
    }
    // Reads: served, or refused as rebuilding.
    {
        let (service, chaos) = (service.clone(), chaos.clone());
        tasks.spawn(async move {
            for _ in 0..3 {
                chaos.pause().await;
                let listed = service
                    .list_sessions(Request::new(ListSessionsRequest {
                        harness: None,
                        updated_since: None,
                        filter: None,
                    }))
                    .await;
                if let Err(status) = listed
                    && !is_rebuilding(&status)
                {
                    chaos.violation(format!("a read failed: {status:?}"));
                }
            }
        });
    }
    // Ready under capture's lock, the sidecar holds every record of this host (I1). (Capture
    // checks the same where it trusts the sidecar: see `Chaos::check_complete`.)
    {
        let (capture, chaos, sidecar) = (capture.clone(), chaos.clone(), sidecar.clone());
        tasks.spawn(async move {
            for _ in 0..4 {
                chaos.pause().await;
                let _local = sidecar.lock_local_projection().await;
                if capture.is_available() {
                    chaos.check_complete("ready").await;
                }
            }
        });
    }
    // Capture: two listeners re-reading the persisted lines, with overlapping new ones.
    let new: Vec<_> = (0..6).map(|i| (format!("s{}", i % 3), format!("n{i}"))).collect();
    let mut captures = tokio::task::JoinSet::new();
    for range in [0..4, 2..6] {
        let mut lines: Vec<_> =
            persisted.iter().map(key_of).chain(new[range].iter().cloned()).collect();
        if rng.gen_bool(0.5) {
            lines.reverse();
        }
        let (capture, chaos, pushed) = (capture.clone(), chaos.clone(), pushed.clone());
        captures.spawn(async move {
            for (session, source) in lines {
                chaos.delay().await;
                // The listener's append: paused while unavailable, retried once ready.
                let (appended, again) = chaos.append(&capture.sink, line(&session, &source)).await;
                match appended {
                    Ok(Appended::New) => {
                        chaos.totals.captured.fetch_add(1, Ordering::Relaxed);
                        note_pushed(&pushed, (session, source), true);
                    }
                    Ok(Appended::Duplicate) if again => {
                        note_pushed(&pushed, (session, source), false);
                    }
                    Ok(Appended::Duplicate) => {}
                    Err(err) => chaos.violation(format!("capture failed: {err}")),
                }
            }
        });
    }

    // The resumed transcript: reopened before each new prompt (as after a restart, or a
    // listener reopening it), so its bookkeeping is warmed from the sidecar each time, across
    // rebuilds' wipes, forgets of this host's sessions and replays. Each prompt is new: one taken
    // for a duplicate was given the id of a stored one, by a warm-up that read a sidecar missing
    // the rows it counts.
    let resumed_new = 4;
    {
        let (capture, chaos, pushed) = (capture.clone(), chaos.clone(), pushed.clone());
        captures.spawn(async move {
            let mut enricher = MessageEnricher::new(HarnessKind::Codex);
            for _ in 0..resumed_new {
                chaos.pause().await;
                // Often right after a wipe, while its replay refills the sidecar.
                if chaos.rng.lock().gen_bool(0.5) {
                    let mut state = capture.state.clone();
                    let wiped = state.wait_for(|state| *state == StoreState::Recovering);
                    let _ = tokio::time::timeout(Duration::from_millis(20), wiped).await;
                }
                if capture.is_recovering() {
                    chaos.totals.warm_ups_recovering.fetch_add(1, Ordering::Relaxed);
                }
                warm(&capture.sink, &mut enricher, &resumed, Start::Resumed).await;
                for msg in enricher.capture(&resumed, &prompt()) {
                    let key = key_of(&msg);
                    match chaos.append(&capture.sink, msg).await {
                        (Ok(Appended::New), _) => {
                            chaos.totals.resumed_captured.fetch_add(1, Ordering::Relaxed);
                            note_pushed(&pushed, key, true);
                        }
                        // The dropped append had pushed it.
                        (Ok(Appended::Duplicate), true) => note_pushed(&pushed, key, false),
                        (Ok(Appended::Duplicate), false) => {
                            chaos.violation(format!("a new resumed prompt dropped: {key:?}"));
                        }
                        (Err(err), _) => chaos.violation(format!("capture failed: {err}")),
                    }
                }
            }
        });
    }

    // Everything but capture ends.
    let ended = tokio::time::timeout(TIMEOUT, async {
        while let Some(task) = tasks.join_next().await {
            task.unwrap();
        }
    });
    assert!(ended.await.is_ok(), "seed {seed}: the chaos never ended");

    // No stall: the store settles (L2).
    let mut state = capture.state.clone();
    let settled = tokio::time::timeout(TIMEOUT, state.wait_for(|s| *s != StoreState::Recovering));
    settled.await.unwrap_or_else(|_| panic!("seed {seed}: stalled recovering")).unwrap();

    // Settled on how the last replay ended: never on an incomplete one, however long a storm
    // kept them so. (Checked unless a replay ended around the read: the sync worker, or a
    // request made during a backoff, may still have the coordinator wipe.)
    let before = chaos.settling();
    let now = *capture.state.borrow();
    if now != StoreState::Recovering && chaos.settling() == before {
        chaos.check_outcome(now, before.2);
    }

    // Healthy from here: a store left unavailable by a failed rebuild comes back with one more
    // (L1); one whose startup recovery failed stays unavailable, refusing rebuilds.
    chaos.faults.store(false, Ordering::SeqCst);
    // Once the requests made during backoffs are in, and their replays done.
    let requested = tokio::time::timeout(TIMEOUT, async {
        while chaos.requests.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    assert!(requested.await.is_ok(), "seed {seed}: a request was never answered");
    let settled = tokio::time::timeout(TIMEOUT, state.wait_for(|s| *s != StoreState::Recovering));
    let settled =
        *settled.await.unwrap_or_else(|_| panic!("seed {seed}: stalled recovering")).unwrap();
    let mut recoverable = true;
    if settled == StoreState::Unavailable {
        match capture.rebuild().await {
            Ok(()) => {}
            Err(RebuildError::Unavailable) => recoverable = false,
            Err(err) => panic!("seed {seed}: the last rebuild failed: {err}"),
        }
    }
    if recoverable {
        let ready = tokio::time::timeout(TIMEOUT, capture.ready()).await;
        assert!(
            ready.unwrap_or_else(|_| panic!("seed {seed}: stalled")),
            "seed {seed}: unavailable"
        );
        let captured = tokio::time::timeout(TIMEOUT, async {
            while let Some(task) = captures.join_next().await {
                task.unwrap();
            }
        });
        assert!(captured.await.is_ok(), "seed {seed}: capture never resumed");
    } else {
        totals.ended_unrecoverable.fetch_add(1, Ordering::Relaxed);
        assert_eq!(settled, StoreState::Unavailable);
        captures.abort_all();
    }

    watching.abort();
    let violations = chaos.violations.lock().clone();
    assert!(violations.is_empty(), "seed {seed}: {violations:#?}");
    // The record store as it ends: no storm left rewriting it.
    let calm = tokio::time::timeout(TIMEOUT, async {
        while chaos.storming.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    assert!(calm.await.is_ok(), "seed {seed}: a storm never stopped");

    // No duplicate record (I2): each line pushed once, and each key once in the record store.
    let pushed = pushed.lock().clone();
    let distinct: BTreeSet<_> = pushed.iter().cloned().collect();
    assert_eq!(pushed.len(), distinct.len(), "seed {seed}: a line pushed twice: {pushed:?}");
    let all = store.all_tagged(&RecordTag::AiSession).await.unwrap().len();
    let fresh = stored(&records).await;
    let expected = keys(&fresh, None).await;
    assert_eq!(all, expected.len(), "seed {seed}: a record pushed twice");
    let local = keys(&fresh, Some(local_host)).await;
    assert_eq!(local, distinct, "seed {seed}");
    if recoverable {
        // Every line captured, once.
        let lines = persisted.len() + new.len() + persisted_resumed.len() + resumed_new;
        assert_eq!(local.len(), lines, "seed {seed}");
        // Ready: every record projected (I1), this host's at once, downloads once the sync
        // worker is done.
        assert!(local.is_subset(&keys(&sidecar, Some(local_host)).await), "seed {seed}");
        projector.send(()).unwrap();
        let synced = tokio::time::timeout(TIMEOUT, async {
            while keys(&sidecar, None).await != expected {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        assert!(synced.await.is_ok(), "seed {seed}: downloads never projected");
    } else {
        let persisted = persisted.len() + persisted_resumed.len();
        assert_eq!(local.len(), persisted, "seed {seed}: captured while unavailable");
    }
}

/// Keep the panics injected into replays out of the test output; report every other.
fn quiet_injected_panics() {
    static ONCE: parking_lot::Once = parking_lot::Once::new();
    ONCE.call_once(|| {
        let report = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.payload().downcast_ref::<&str>() != Some(&INJECTED_PANIC) {
                report(info);
            }
        }));
    });
}

/// Runs seeds `0..ATUIN_CHAOS_ITERATIONS` (by default 100), several at once.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_survives_chaos() {
    quiet_injected_panics();
    let iterations: u64 =
        std::env::var("ATUIN_CHAOS_ITERATIONS").ok().and_then(|n| n.parse().ok()).unwrap_or(100);
    let totals = Arc::new(Totals::default());
    let started = std::time::Instant::now();
    let mut seeds = futures::stream::iter(0..iterations)
        .map(|seed| {
            let totals = totals.clone();
            tokio::spawn(iteration(seed, totals))
        })
        .buffer_unordered(6);
    while let Some(done) = seeds.next().await {
        if let Err(err) = done {
            std::panic::resume_unwind(err.into_panic());
        }
    }
    eprintln!("{iterations} chaos iterations in {:?}: {totals:#?}", started.elapsed());
}
