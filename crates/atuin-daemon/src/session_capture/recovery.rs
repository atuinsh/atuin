//! Recovery of the AI-session sidecar: startup recovery and rebuilds, coordinated by one actor.
//!
//! The sidecar is a projection of the record store, and capture's dedup index: capture pushes a
//! line to the record store only if the sidecar does not hold it yet, so a sidecar missing
//! persisted records would have capture push them twice. Rebuilds empty the sidecar (a wipe) and
//! replays refill it, while capture, reads and the sync worker carry on around them.
//!
//! One task, the [`Coordinator`], owns everything that decides the store's [`StoreState`]: the
//! state itself (it is the only writer of its watch channel), how many wipes there have been
//! (`gen`), the replay running and whether startup recovery succeeded. It takes one message at a
//! time: rebuild requests, the sync worker finding a series it must not forget beside capture
//! (see [`AiSessionStore::reproject_beside_capture`]), and replays reporting they are done (a
//! supervisor task turns a replay that panicked or was aborted into a report too). It wipes
//! inline, holding capture's lock, so nothing interleaves with a wipe but what the locks allow.
//! A wipe is a rebuild's (everything) or the sync worker's (the one host's sessions): both hold
//! capture off first, and are replayed after.
//!
//! [`AiSessionStore::reproject_beside_capture`]: atuin_client::ai_session::AiSessionStore::reproject_beside_capture
//!
//! Its decisions are a pure transition function, [`CoordState::step`], from an [`Event`] to the
//! [`Effect`]s the coordinator then carries out; a wipe's outcome is fed back as an event, and so
//! is a backoff running out. The tests below check the protocol's invariants over every event
//! sequence up to a bound, and over random longer ones.
//!
//! A replay that gave up because invalidations kept landing ([`ReplayResult::Incomplete`]) is
//! replayed again, however often, after a backoff growing with each one in a row: nothing is
//! wrong with the store, so it must not be left unavailable, and the invalidations stop sooner or
//! later (each forget clears the rewrite it handled). A wipe meanwhile cuts the backoff short.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{BuildError, DbError, ReprojectProgress};
use atuin_domain::record::HostId;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;

#[cfg(test)]
use super::hooks::{Fault, Point, Section};
use super::{RebuildError, Sink, StoreState};

/// Names one replay, so a report from one the coordinator no longer waits on is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReplayId(u64);

/// A replay the coordinator started: which one, and how many wipes there had been.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Replay {
    pub(super) id: ReplayId,
    pub(super) started_gen: u64,
}

/// How a replay ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplayResult {
    Ok,
    /// Reprojecting failed: the sidecar may be missing records.
    Failed,
    /// The replay task panicked or was aborted.
    Panicked,
    /// Invalidations kept landing until the reprojection gave up
    /// ([`BuildError::Incomplete`]): the sidecar may be missing records, but a replay again may
    /// well get through.
    Incomplete,
}

/// Names one backoff, so a backoff the coordinator no longer waits out is ignored when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Wait(u64);

/// How long to wait before replaying again after replays ended [`ReplayResult::Incomplete`] in a
/// row: `first` after the first, doubling with each after it, up to `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub first: Duration,
    pub max: Duration,
}

impl Backoff {
    pub const DEFAULT: Self = Self {
        first: Duration::from_millis(100),
        max: Duration::from_secs(30),
    };

    /// The wait before replaying again after `retry` incomplete replays in a row (from 1).
    pub(super) fn delay(self, retry: u32) -> Duration {
        let doublings = retry.saturating_sub(1);
        let factor = 1_u32.checked_shl(doublings).filter(|_| doublings < 32);
        factor.map_or(self.max, |factor| self.first.saturating_mul(factor)).min(self.max)
    }
}

/// What a wipe empties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    /// Everything ([`atuin_client::ai_session::AiSessionDatabase::reset`]): a rebuild.
    All,
    /// The sessions of the host whose series changed
    /// ([`atuin_client::ai_session::AiSessionDatabase::forget_host`]).
    Host,
}

/// Why a rebuild was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// Startup recovery failed: nothing to rebuild with until restart.
    Unavailable,
    /// The wipe failed (one transaction: nothing was deleted).
    WipeFailed,
}

/// What the coordinator is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Event {
    /// A rebuild was asked for.
    Rebuild,
    /// The sync worker found a series rewritten or deleted under its watermark whose forgetting
    /// deletes rows of this host, which capture dedups against: this host's own (or another's,
    /// in this host's sessions). Handled as a rebuild wiping only that host's sessions.
    SeriesRewritten,
    /// The wipe a [`Effect::Wipe`] asked for is over. Always the next event after that effect:
    /// the coordinator wipes inline.
    WipeDone {
        ok: bool,
    },
    /// A replay ended.
    ReplayDone {
        replay: Replay,
        result: ReplayResult,
    },
    /// A backoff an [`Effect::ReplayAfter`] asked for ran out.
    WaitedOut {
        wait: Wait,
    },
}

/// What the coordinator does, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Effect {
    /// Set the store's state.
    Broadcast(StoreState),
    /// Empty the sidecar (or part) under capture's lock, then report [`Event::WipeDone`].
    Wipe(Scope),
    /// Spawn a replay.
    StartReplay(Replay),
    /// Wait out the backoff after `retry` incomplete replays in a row (see [`Backoff::delay`]),
    /// then report [`Event::WaitedOut`]. The coordinator takes other messages meanwhile.
    ReplayAfter {
        wait: Wait,
        retry: u32,
    },
    /// Answer the request being handled (a rebuild, or the sync worker's).
    Reply(Result<(), Refusal>),
}

/// The coordinator's state: what its decisions depend on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CoordState {
    pub(super) state: StoreState,
    /// How many wipes have emptied the sidecar.
    pub(super) generation: u64,
    /// The replay running, if any: at most one.
    pub(super) replay: Option<Replay>,
    /// The backoff being waited out before replaying again, if any: only with no replay running.
    pub(super) backoff: Option<Wait>,
    /// Whether a replay has made the store ready: then capture started, and a store left
    /// unavailable by a failed replay can be rebuilt again. Until then, a failed startup recovery
    /// leaves it unavailable until restart.
    pub(super) recovered: bool,
    /// While a rebuild's wipe runs: the state it replaced, restored should the wipe fail with no
    /// replay running.
    pub(super) wiping: Option<StoreState>,
    /// Replays in a row ended [`ReplayResult::Incomplete`] at this generation: what the backoff
    /// grows with. Reset when the store settles, and by a wipe.
    pub(super) retries: u32,
    next_id: u64,
}

impl CoordState {
    /// Boot: recovering, with startup recovery's replay started.
    pub(super) fn boot() -> (Self, Vec<Effect>) {
        let replay = Replay {
            id: ReplayId(0),
            started_gen: 0,
        };
        let coord = Self {
            state: StoreState::Recovering,
            generation: 0,
            replay: Some(replay),
            backoff: None,
            recovered: false,
            wiping: None,
            retries: 0,
            next_id: 1,
        };
        (coord, vec![Effect::Broadcast(StoreState::Recovering), Effect::StartReplay(replay)])
    }

    /// Decide what `event` does. Nothing but [`Event::WipeDone`] may follow an [`Effect::Wipe`]
    /// until it does (the coordinator wipes inline, taking no message meanwhile).
    pub(super) fn step(&mut self, event: Event) -> Vec<Effect> {
        debug_assert!(
            self.wiping.is_none() || matches!(event, Event::WipeDone { .. }),
            "{event:?} while wiping"
        );
        match event {
            Event::Rebuild => self.rebuild(Scope::All),
            Event::SeriesRewritten => self.rebuild(Scope::Host),
            Event::WipeDone { ok } => self.wiped(ok),
            Event::ReplayDone { replay, result } => self.replayed(replay, result),
            Event::WaitedOut { wait } => self.waited_out(wait),
        }
    }

    /// A rebuild, or the sync worker's forget: the same but for what the wipe empties.
    fn rebuild(&mut self, scope: Scope) -> Vec<Effect> {
        // Capture never started (so has nothing to dedup), and nothing replays until restart.
        if self.state == StoreState::Unavailable && !self.recovered {
            return vec![Effect::Reply(Err(Refusal::Unavailable))];
        }
        self.wiping = Some(self.state);
        self.state = StoreState::Recovering;
        // Said before the wipe takes capture's lock, which capture checks again under that lock:
        // no capture checks the sidecar from then until a replay settles.
        vec![Effect::Broadcast(StoreState::Recovering), Effect::Wipe(scope)]
    }

    fn wiped(&mut self, ok: bool) -> Vec<Effect> {
        let Some(before) = self.wiping.take() else {
            debug_assert!(false, "a wipe reported with none running");
            return Vec::new();
        };
        let mut effects = Vec::new();
        if !ok {
            // Nothing was deleted. A replay running (or waited for) settles the state; otherwise
            // the store is as it was: ready, or unavailable.
            if self.replay.is_none() && self.backoff.is_none() {
                self.state = before;
                effects.push(Effect::Broadcast(before));
            }
            effects.push(Effect::Reply(Err(Refusal::WipeFailed)));
            return effects;
        }
        self.generation += 1;
        // A new generation: whatever kept invalidating the replays before may be over.
        self.retries = 0;
        // A replay running notices the wipe when it reports, and is replayed again. One waited
        // for starts now: what it waited out is moot.
        if self.replay.is_none() {
            self.backoff = None;
            effects.push(self.start_replay());
        }
        effects.push(Effect::Reply(Ok(())));
        effects
    }

    fn replayed(&mut self, replay: Replay, result: ReplayResult) -> Vec<Effect> {
        if self.replay != Some(replay) {
            // Not the replay running: a stale report.
            return Vec::new();
        }
        self.replay = None;
        if replay.started_gen != self.generation {
            // A wipe since it started may have deleted what it projected: replay again rather
            // than call the sidecar ready.
            self.retries = 0;
            return vec![self.start_replay()];
        }
        if result == ReplayResult::Incomplete {
            // Invalidations kept it from finishing, which is no fault of the store: replay again,
            // still recovering, once they have had a while to stop. Never unavailable for it, or
            // a burst of them at startup would leave the store so until restart.
            self.retries = self.retries.saturating_add(1);
            let wait = Wait(self.next_id);
            self.next_id += 1;
            self.backoff = Some(wait);
            return vec![Effect::ReplayAfter {
                wait,
                retry: self.retries,
            }];
        }
        self.retries = 0;
        self.state = match result {
            ReplayResult::Ok => {
                self.recovered = true;
                StoreState::Ready
            }
            ReplayResult::Failed | ReplayResult::Panicked => StoreState::Unavailable,
            ReplayResult::Incomplete => unreachable!("replayed again above"),
        };
        vec![Effect::Broadcast(self.state)]
    }

    fn waited_out(&mut self, wait: Wait) -> Vec<Effect> {
        if self.backoff != Some(wait) {
            // Cut short by a wipe, which started the replay already.
            return Vec::new();
        }
        self.backoff = None;
        vec![self.start_replay()]
    }

    fn start_replay(&mut self) -> Effect {
        debug_assert!(self.replay.is_none(), "a second replay");
        debug_assert!(self.backoff.is_none(), "a replay while waiting to replay");
        let replay = Replay {
            id: ReplayId(self.next_id),
            started_gen: self.generation,
        };
        self.next_id += 1;
        self.replay = Some(replay);
        Effect::StartReplay(replay)
    }
}

/// A message to the coordinator.
pub(super) enum Msg {
    Rebuild(oneshot::Sender<Result<(), RebuildError>>),
    /// From the sync worker: see [`Event::SeriesRewritten`]. `host` is the host to forget.
    SeriesRewritten {
        host: HostId,
        reply: oneshot::Sender<Result<(), RebuildError>>,
    },
    ReplayDone {
        replay: Replay,
        result: ReplayResult,
    },
    /// From a backoff's timer: see [`Event::WaitedOut`].
    WaitedOut(Wait),
}

/// The actor carrying out [`CoordState`]'s decisions. See the module docs.
pub(super) struct Coordinator {
    core: CoordState,
    rx: mpsc::UnboundedReceiver<Msg>,
    /// For replays' supervisors to report on. Weak, so the coordinator ends once the facade is
    /// gone and no replay is left to report.
    tx: mpsc::WeakUnboundedSender<Msg>,
    state: watch::Sender<StoreState>,
    sink: Arc<Sink>,
    progress: ReprojectProgress,
    backoff: Backoff,
    /// The replay running, aborted with the coordinator.
    running: Option<AbortHandle>,
    /// The timer of the backoff being waited out, aborted with the coordinator or once moot.
    waiting: Option<AbortHandle>,
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        // Only stops the task: nothing is left to report to.
        for task in self.running.iter().chain(&self.waiting) {
            task.abort();
        }
    }
}

impl Coordinator {
    /// Spawn the coordinator, recovering at once. Returns its mailbox and the store's state.
    pub(super) fn spawn(
        sink: Arc<Sink>,
        state: watch::Sender<StoreState>,
        progress: ReprojectProgress,
        backoff: Backoff,
    ) -> (mpsc::UnboundedSender<Msg>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (core, boot) = CoordState::boot();
        let coordinator = Self {
            core,
            rx,
            tx: tx.downgrade(),
            state,
            sink,
            progress,
            backoff,
            running: None,
            waiting: None,
        };
        let task = tokio::spawn(coordinator.run(boot));
        (tx, task)
    }

    async fn run(mut self, boot: Vec<Effect>) {
        self.apply(boot, None, None).await;
        while let Some(msg) = self.rx.recv().await {
            match msg {
                Msg::Rebuild(reply) => {
                    let effects = self.core.step(Event::Rebuild);
                    self.apply(effects, Some(reply), None).await;
                }
                Msg::SeriesRewritten { host, reply } => {
                    let effects = self.core.step(Event::SeriesRewritten);
                    self.apply(effects, Some(reply), Some(host)).await;
                }
                Msg::ReplayDone { replay, result } => {
                    if self.core.replay == Some(replay) {
                        self.running = None;
                    }
                    let effects = self.core.step(Event::ReplayDone { replay, result });
                    self.apply(effects, None, None).await;
                }
                Msg::WaitedOut(wait) => {
                    if self.core.backoff == Some(wait) {
                        self.waiting = None;
                    }
                    let effects = self.core.step(Event::WaitedOut { wait });
                    self.apply(effects, None, None).await;
                }
            }
        }
    }

    /// Carry out `effects` in order, answering `reply` (the request being handled). `host` is
    /// the host a [`Scope::Host`] wipe forgets.
    async fn apply(
        &mut self,
        effects: Vec<Effect>,
        mut reply: Option<oneshot::Sender<Result<(), RebuildError>>>,
        host: Option<HostId>,
    ) {
        let mut effects = VecDeque::from(effects);
        let mut wipe_error = None;
        while let Some(effect) = effects.pop_front() {
            match effect {
                Effect::Broadcast(state) => self.broadcast(state),
                Effect::Wipe(scope) => {
                    let wiped = self.wipe(scope, host).await;
                    let ok = wiped.is_ok();
                    if let Err(err) = wiped {
                        tracing::error!(?err, ?scope, "failed to empty the ai-session sidecar");
                        wipe_error = Some(err);
                    }
                    effects.extend(self.core.step(Event::WipeDone { ok }));
                }
                Effect::StartReplay(replay) => {
                    #[cfg(test)]
                    if self.waiting.is_some() {
                        self.sink.hook(Point::BackoffCutShort).await;
                    }
                    self.start_replay(replay);
                }
                Effect::ReplayAfter { wait, retry } => self.wait_out(wait, retry),
                Effect::Reply(result) => {
                    let result = result.map_err(|refusal| match refusal {
                        Refusal::Unavailable => RebuildError::Unavailable,
                        Refusal::WipeFailed => {
                            wipe_error.take().map_or(RebuildError::Aborted, RebuildError::Sidecar)
                        }
                    });
                    if let Some(reply) = reply.take() {
                        // The caller may have stopped waiting: the rebuild went ahead regardless.
                        let _ = reply.send(result);
                    }
                }
            }
        }
    }

    fn broadcast(&self, state: StoreState) {
        if state == StoreState::Unavailable {
            if self.core.recovered {
                tracing::error!(
                    "the ai-session sidecar could not be replayed; capture and import paused \
                     until a rebuild succeeds or restart"
                );
            } else {
                tracing::error!(
                    "the ai-session sidecar could not be recovered; capture and import disabled \
                     until restart"
                );
            }
        }
        self.state.send_replace(state);
    }

    /// Empty the sidecar, or what `host` projected for [`Scope::Host`], holding capture's lock so
    /// no capture is between its check and its push.
    async fn wipe(&self, scope: Scope, host: Option<HostId>) -> Result<(), DbError> {
        #[cfg(test)]
        self.sink.hook(Point::BeforeWipe).await;
        let wiped = {
            let _local = self.sink.sidecar.lock_local_projection().await;
            #[cfg(test)]
            let _section = self.sink.enter(Section::Wipe);
            #[cfg(test)]
            let fault = self.sink.hook(Point::WipeLocked).await;
            #[cfg(test)]
            if fault != Fault::None {
                return Err(DbError::Query(sqlx::Error::Protocol("injected wipe failure".into())));
            }
            match (scope, host) {
                (Scope::All, _) => self.sink.sidecar.reset().await,
                (Scope::Host, Some(host)) => {
                    tracing::warn!(
                        %host,
                        "ai-session records rewritten under capture: holding capture off to \
                         replay them"
                    );
                    self.sink.sidecar.forget_host(host).await.map(|_| ())
                }
                (Scope::Host, None) => unreachable!("a host's wipe asked for without the host"),
            }
        };
        #[cfg(test)]
        self.sink.hook(Point::AfterWipe).await;
        wiped
    }

    /// Spawn `replay`, and a supervisor reporting how it ended, panics and aborts included.
    fn start_replay(&mut self, replay: Replay) {
        let Some(tx) = self.tx.upgrade() else {
            // The facade is gone: nobody is left to serve.
            return;
        };
        // A backoff a wipe cut short: its timer is moot.
        if let Some(waiting) = self.waiting.take() {
            waiting.abort();
        }
        self.progress.clear();
        let task = tokio::spawn(run_replay(self.sink.clone(), self.progress.clone()));
        self.running = Some(task.abort_handle());
        tokio::spawn(async move {
            let result = match task.await {
                Ok(result) => result,
                Err(err) => {
                    tracing::error!(?err, "the ai-session sidecar replay stopped unexpectedly");
                    ReplayResult::Panicked
                }
            };
            let _ = tx.send(Msg::ReplayDone { replay, result });
        });
    }

    /// Report [`Event::WaitedOut`] once the backoff after `retry` incomplete replays in a row is
    /// over. Holds the mailbox open meanwhile, as a replay does: the coordinator still has to
    /// replay.
    fn wait_out(&mut self, wait: Wait, retry: u32) {
        let Some(tx) = self.tx.upgrade() else {
            return;
        };
        let delay = self.backoff.delay(retry);
        tracing::info!(
            ?delay,
            retry,
            "the ai-session sidecar replay kept being invalidated; replaying again after a backoff"
        );
        #[cfg(test)]
        let sink = self.sink.clone();
        let timer = tokio::spawn(async move {
            #[cfg(test)]
            sink.hook(Point::BackoffStarted).await;
            tokio::time::sleep(delay).await;
            let _ = tx.send(Msg::WaitedOut(wait));
        });
        self.waiting = Some(timer.abort_handle());
    }
}

/// Replay the sidecar from the record store.
async fn run_replay(sink: Arc<Sink>, progress: ReprojectProgress) -> ReplayResult {
    #[cfg(test)]
    let _section = sink.enter(Section::Replay);
    let started = std::time::Instant::now();
    // Capture is held off (the store is recovering while a replay runs), so a series found
    // rewritten is forgotten here and now, this host's included.
    let result = sink.records.reproject_with(&sink.sidecar, &progress).await;
    #[cfg(test)]
    if matches!(result, Err(BuildError::Incomplete)) {
        sink.hook(Point::ReplayIncomplete).await;
    }
    #[cfg(test)]
    let result = match sink.hook(Point::ReplayBeforeSettle).await {
        Fault::None => result,
        Fault::Fail => Err(DbError::InvalidRecordId.into()),
        Fault::Incomplete => Err(BuildError::Incomplete),
        Fault::Panic => std::panic::panic_any(super::hooks::INJECTED_PANIC),
    };
    match result {
        Ok(stats) => {
            tracing::info!(
                replayed = stats.replayed,
                restarted = stats.restarted,
                elapsed = ?started.elapsed(),
                "ai-session sidecar replayed"
            );
            ReplayResult::Ok
        }
        Err(BuildError::Incomplete) => {
            tracing::warn!("the ai-session sidecar replay kept being invalidated");
            ReplayResult::Incomplete
        }
        Err(err) => {
            tracing::error!(?err, "failed to reproject the ai-session sidecar");
            ReplayResult::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    /// The world around the coordinator's pure core: which replays run, which backoffs are
    /// waited out, what each rebuild was answered, and the last replay the coordinator heard
    /// from. Checks the protocol's invariants after every step.
    #[derive(Debug, Clone)]
    struct World {
        core: CoordState,
        /// Replay tasks alive (spawned, not yet reported).
        live: BTreeSet<u64>,
        /// Every replay ever started, to report stale ones.
        started: Vec<Replay>,
        /// Backoff timers alive (started, not yet reported).
        timers: BTreeSet<u64>,
        /// Every backoff ever started, to report stale ones.
        waits: Vec<Wait>,
        /// Rebuilds asked for and not answered yet.
        unanswered: usize,
        /// The last replay the coordinator acted on: how it ended, and the generation it started
        /// at.
        last_done: Option<(u64, ReplayResult)>,
        /// How many replays in a row, started at the generation given, the coordinator heard
        /// end incomplete.
        incomplete: (u64, u32),
        /// What the request being handled wipes: a rebuild everything, the sync worker's a host.
        asked: Option<Scope>,
        /// Every state the coordinator said, in order.
        broadcasts: Vec<StoreState>,
    }

    impl World {
        fn boot() -> Self {
            let (core, effects) = CoordState::boot();
            let mut world = Self {
                core,
                live: BTreeSet::new(),
                started: Vec::new(),
                timers: BTreeSet::new(),
                waits: Vec::new(),
                unanswered: 0,
                last_done: None,
                incomplete: (0, 0),
                asked: None,
                broadcasts: Vec::new(),
            };
            world.run(&effects);
            world.check();
            world
        }

        /// The events that can happen next. Stale reports come from replays (and backoffs) that
        /// already reported, and from made-up ones.
        fn enabled(&self) -> Vec<Event> {
            if self.core.wiping.is_some() {
                return vec![Event::WipeDone { ok: true }, Event::WipeDone { ok: false }];
            }
            let mut events = vec![Event::Rebuild, Event::SeriesRewritten];
            if let Some(replay) = self.core.replay {
                for result in [
                    ReplayResult::Ok,
                    ReplayResult::Failed,
                    ReplayResult::Panicked,
                    ReplayResult::Incomplete,
                ] {
                    events.push(Event::ReplayDone { replay, result });
                }
            }
            if let Some(wait) = self.core.backoff {
                events.push(Event::WaitedOut { wait });
            }
            // One that reported already, if any, and one never started.
            if let Some(&replay) = self.started.iter().rev().find(|r| Some(**r) != self.core.replay)
            {
                events.push(Event::ReplayDone {
                    replay,
                    result: ReplayResult::Ok,
                });
            }
            events.push(Event::ReplayDone {
                replay: Replay {
                    id: ReplayId(u64::MAX),
                    started_gen: self.core.generation,
                },
                result: ReplayResult::Ok,
            });
            // A backoff a wipe cut short, its timer firing all the same.
            if let Some(&wait) = self.waits.iter().rev().find(|w| Some(**w) != self.core.backoff) {
                events.push(Event::WaitedOut { wait });
            }
            events
        }

        fn apply(&mut self, event: Event) {
            let before = self.core.clone();
            match event {
                Event::Rebuild => self.asked = Some(Scope::All),
                Event::SeriesRewritten => self.asked = Some(Scope::Host),
                Event::WipeDone { .. } | Event::ReplayDone { .. } | Event::WaitedOut { .. } => {}
            }
            if matches!(event, Event::Rebuild | Event::SeriesRewritten) {
                self.unanswered += 1;
            }
            let current = match event {
                Event::ReplayDone { replay, .. } => Some(replay) == self.core.replay,
                Event::WaitedOut { wait } => Some(wait) == self.core.backoff,
                _ => true,
            };
            if let Event::ReplayDone { replay, result } = event
                && current
            {
                assert!(self.live.remove(&replay.id.0), "a report from a replay not running");
                self.last_done = Some((replay.started_gen, result));
                self.incomplete = match (result, self.incomplete) {
                    (ReplayResult::Incomplete, (at, n)) if at == replay.started_gen => (at, n + 1),
                    (ReplayResult::Incomplete, _) => (replay.started_gen, 1),
                    _ => (replay.started_gen, 0),
                };
            }
            if let Event::WaitedOut { wait } = event
                && current
            {
                assert!(self.timers.remove(&wait.0), "a backoff over that never started");
            }
            let effects = self.core.step(event);
            if !current {
                assert_eq!(self.core, before, "a stale report changed the coordinator");
                assert!(effects.is_empty(), "a stale report did something");
            }
            // A wipe makes every backoff moot: the shell aborts its timer when the replay starts.
            if effects.iter().any(|e| matches!(e, Effect::StartReplay(_))) {
                self.timers.clear();
            }
            self.run(&effects);
            if event == (Event::WipeDone { ok: true }) {
                // A wipe replays at once: a backoff it found is moot, a new generation.
                assert!(self.core.replay.is_some(), "no replay after a wipe");
                assert!(self.core.backoff.is_none(), "a backoff waited out after a wipe");
            }
            if let Event::ReplayDone {
                result: ReplayResult::Incomplete,
                ..
            } = event
                && current
            {
                // An incomplete replay never settles the store: it is replayed again, now or
                // after a backoff, still recovering.
                assert_eq!(self.core.state, StoreState::Recovering, "settled on an incomplete");
                assert!(self.core.replay.is_some() || self.core.backoff.is_some());
            }
            self.check();
        }

        /// Account for `effects`, as the coordinator would carry them out.
        fn run(&mut self, effects: &[Effect]) {
            let mut state = None;
            for (i, effect) in effects.iter().enumerate() {
                match *effect {
                    Effect::Broadcast(s) => {
                        state = Some(s);
                        self.broadcasts.push(s);
                    }
                    Effect::Wipe(scope) => {
                        assert_eq!(i, effects.len() - 1, "the wipe is the last effect");
                        // Capture is held off before anything is deleted, this host's rows
                        // (the sync worker's forget) included.
                        assert_eq!(
                            state,
                            Some(StoreState::Recovering),
                            "recovering is said before wiping"
                        );
                        assert_eq!(Some(scope), self.asked, "wiped what was not asked");
                    }
                    Effect::StartReplay(replay) => {
                        // I4: at most one replay, and none while a backoff is waited out.
                        assert!(self.live.is_empty(), "a second replay started: {:?}", self.live);
                        assert!(self.timers.is_empty(), "a replay while waiting to replay");
                        assert_eq!(replay.started_gen, self.core.generation);
                        self.live.insert(replay.id.0);
                        self.started.push(replay);
                    }
                    Effect::ReplayAfter { wait, retry } => {
                        // Only after an incomplete replay of this generation, with none running.
                        assert!(self.live.is_empty(), "a backoff beside a replay");
                        assert!(self.timers.is_empty(), "a second backoff");
                        assert_eq!(
                            self.last_done,
                            Some((self.core.generation, ReplayResult::Incomplete))
                        );
                        // The backoff grows with each incomplete replay in a row, and starts
                        // over with a new generation.
                        assert_eq!((self.core.generation, retry), self.incomplete);
                        self.timers.insert(wait.0);
                        self.waits.push(wait);
                    }
                    Effect::Reply(_) => {
                        assert!(self.unanswered > 0, "an answer to no rebuild");
                        self.unanswered -= 1;
                    }
                }
            }
            // The state broadcast is the core's (I5: the coordinator is the only writer, and says
            // every change).
            if let Some(state) = state {
                assert_eq!(state, self.core.state);
            }
        }

        fn check(&self) {
            let core = &self.core;
            // The core's replay is the live one, and its backoff the one waited out.
            assert_eq!(core.replay.map(|r| r.id.0).into_iter().collect::<BTreeSet<_>>(), self.live);
            assert_eq!(core.backoff.map(|w| w.0).into_iter().collect::<BTreeSet<_>>(), self.timers);
            // I4, with a replay waited for counted as running.
            assert!(self.live.len() + self.timers.len() <= 1);
            // The last state said is the core's.
            assert_eq!(self.broadcasts.last(), Some(&core.state));
            // A replay runs only while recovering: capture is held off, so a replay may forget
            // what a series rewritten under its watermark projected, this host's rows included.
            if core.replay.is_some() || core.backoff.is_some() {
                assert_eq!(core.state, StoreState::Recovering, "a replay beside capture");
            }
            // Incomplete replays are counted only while recovering.
            if core.state != StoreState::Recovering {
                assert_eq!(core.retries, 0);
            }
            if core.wiping.is_some() {
                // A rebuild in hand; it is answered once its wipe reports.
                assert_eq!(self.unanswered, 1);
                assert_eq!(core.state, StoreState::Recovering);
                return;
            }
            // Every rebuild is answered before the next event.
            assert_eq!(self.unanswered, 0);
            match core.state {
                // Ready only if the last replay heard from started at the current generation and
                // succeeded: no wipe since it started (so none since it ended either).
                StoreState::Ready => {
                    assert_eq!(self.last_done, Some((core.generation, ReplayResult::Ok)));
                    assert!(core.recovered);
                    assert!(core.replay.is_none());
                }
                // L2: no stall. Recovering always has a replay running, or one waited for, to
                // end it.
                StoreState::Recovering => {
                    assert!(core.replay.is_some() || core.backoff.is_some(), "stalled: {core:?}");
                }
                // Unavailable only if the last replay heard from started at the current generation
                // and failed (never as incomplete): one that started before a wipe is replayed
                // again rather than end a rebuild that was answered ok (which would wait on a
                // replay that never comes, or be refused as unavailable with none running).
                StoreState::Unavailable => {
                    assert!(core.replay.is_none());
                    assert!(
                        matches!(
                            self.last_done,
                            Some((generation, ReplayResult::Failed | ReplayResult::Panicked))
                                if generation == core.generation
                        ),
                        "unavailable on {:?} at generation {}",
                        self.last_done,
                        core.generation
                    );
                }
            }
        }

        /// Run whatever is in flight to its next report: a wipe (failing unless `result` is
        /// ok), a backoff, or the replay running (ending with `result`). False if nothing is.
        fn advance(&mut self, result: ReplayResult) -> bool {
            if self.core.wiping.is_some() {
                self.apply(Event::WipeDone {
                    ok: result == ReplayResult::Ok,
                });
            } else if let Some(wait) = self.core.backoff {
                self.apply(Event::WaitedOut { wait });
            } else if let Some(replay) = self.core.replay {
                self.apply(Event::ReplayDone { replay, result });
            } else {
                return false;
            }
            true
        }

        /// Once events stop: what is in flight ends with `result` (not incomplete), and the
        /// store settles after at most a wipe, a backoff, the replay running and one more (if a
        /// wipe came after it started).
        fn settle(mut self, result: ReplayResult) -> Self {
            assert_ne!(result, ReplayResult::Incomplete, "an incomplete replay never settles");
            let most = 4;
            for _ in 0..most {
                self.advance(result);
            }
            assert!(!self.advance(result), "settling needs at most {most} steps");
            assert_ne!(self.core.state, StoreState::Recovering);
            self
        }

        /// An invalidation storm: `n` replays in a row end incomplete. The store stays
        /// recovering throughout, each waiting out a longer backoff.
        fn storm(mut self, n: u32) -> Self {
            if self.core.wiping.is_some() {
                self.apply(Event::WipeDone { ok: true });
            }
            let recovering = self.core.state == StoreState::Recovering;
            let mut replays = 0;
            while replays < n && self.core.state == StoreState::Recovering {
                if self.core.replay.is_some() {
                    replays += 1;
                }
                assert!(self.advance(ReplayResult::Incomplete), "stalled in a storm");
            }
            if recovering {
                assert_eq!(self.core.state, StoreState::Recovering, "a storm settled the store");
            }
            self
        }

        /// L1: once rebuilds stop and nothing fails, the store ends ready, however long an
        /// invalidation storm kept its replays incomplete first. (A failed startup recovery
        /// excepted: that store stays unavailable until restart.) A store left unavailable by a
        /// failed rebuild takes one more rebuild.
        fn check_liveness(&self) {
            for storm in [0, 3] {
                let settled = self.clone().storm(storm).settle(ReplayResult::Ok);
                if self.core.state == StoreState::Recovering {
                    assert_eq!(settled.core.state, StoreState::Ready, "after a storm of {storm}");
                }
                match settled.core.state {
                    StoreState::Ready => {}
                    StoreState::Unavailable if !settled.core.recovered => {
                        let mut again = settled.clone();
                        again.apply(Event::Rebuild);
                        assert_eq!(again.core, settled.core, "a rebuild after a failed startup");
                    }
                    _ => {
                        let mut again = settled;
                        again.apply(Event::Rebuild);
                        again.apply(Event::WipeDone { ok: true });
                        let again = again.storm(storm).settle(ReplayResult::Ok);
                        assert_eq!(again.core.state, StoreState::Ready);
                    }
                }
            }
        }
    }

    /// Every event sequence up to `depth` long, depth first, checking the invariants after every
    /// step and liveness at every point the events could stop.
    fn explore(world: &World, depth: usize, visited: &mut u64) {
        *visited += 1;
        world.check_liveness();
        if depth == 0 {
            return;
        }
        for event in world.enabled() {
            let mut next = world.clone();
            next.apply(event);
            explore(&next, depth - 1, visited);
        }
    }

    #[rstest]
    fn every_short_event_sequence_keeps_the_invariants() {
        let depth = std::env::var("ATUIN_RECOVERY_DEPTH").ok().and_then(|d| d.parse().ok());
        let mut visited = 0;
        explore(&World::boot(), depth.unwrap_or(9), &mut visited);
        assert!(visited > 100_000, "explored {visited} states");
        eprintln!("explored {visited} event sequences");
    }

    /// An event, chosen among those enabled.
    fn pick(world: &World, choice: usize) -> Event {
        let enabled = world.enabled();
        enabled[choice % enabled.len()]
    }

    #[rstest]
    fn long_random_event_sequences_keep_the_invariants() {
        let config = ProptestConfig {
            cases: 2_000,
            ..ProptestConfig::default()
        };
        proptest!(config, |(
            choices in proptest::collection::vec(any::<usize>(), 0..200),
            storm in 0_u32..40,
        )| {
            let mut world = World::boot();
            for choice in choices {
                let event = pick(&world, choice);
                world.apply(event);
                world.check_liveness();
            }
            // However long a storm of invalidations lasts, the store recovers through it, and
            // is ready once it is over.
            let recovering = world.core.state == StoreState::Recovering;
            let stormy = world.clone().storm(storm);
            if recovering {
                prop_assert_eq!(stormy.core.state, StoreState::Recovering);
                prop_assert_eq!(stormy.settle(ReplayResult::Ok).core.state, StoreState::Ready);
            }
            let settled = world.settle(ReplayResult::Failed);
            prop_assert_ne!(settled.core.state, StoreState::Recovering);
        });
    }

    /// The known-bad interleavings, by name.
    #[rstest]
    // A rebuild's wipe while startup recovery replays: that replay goes round again.
    #[case::rebuild_during_recovery(&[Event::Rebuild, Event::WipeDone { ok: true }], StoreState::Recovering)]
    // A failed wipe with no replay running restores the store as it was.
    #[case::failed_wipe_when_ready(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Ok },
        Event::Rebuild,
        Event::WipeDone { ok: false },
    ], StoreState::Ready)]
    // A replay that panicked after a rebuild: the store is unavailable, and a rebuild replays.
    #[case::rebuild_after_a_panic(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Ok },
        Event::Rebuild,
        Event::WipeDone { ok: true },
        Event::ReplayDone { replay: Replay { id: ReplayId(1), started_gen: 1 }, result: ReplayResult::Panicked },
        Event::Rebuild,
        Event::WipeDone { ok: true },
    ], StoreState::Recovering)]
    // A failed startup recovery refuses rebuilds.
    #[case::failed_startup(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Failed },
        Event::Rebuild,
    ], StoreState::Unavailable)]
    // And the sync worker's forget: capture never started.
    #[case::series_rewritten_after_failed_startup(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Failed },
        Event::SeriesRewritten,
    ], StoreState::Unavailable)]
    // This host's series rewritten while ready: capture is held off before the forget, and a
    // replay follows.
    #[case::series_rewritten_while_ready(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Ok },
        Event::SeriesRewritten,
    ], StoreState::Recovering)]
    #[case::series_rewritten_then_replayed(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Ok },
        Event::SeriesRewritten,
        Event::WipeDone { ok: true },
        Event::ReplayDone { replay: Replay { id: ReplayId(1), started_gen: 1 }, result: ReplayResult::Ok },
    ], StoreState::Ready)]
    // A failed forget restores the store as it was: the next sync finds the series again.
    #[case::failed_forget_when_ready(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Ok },
        Event::SeriesRewritten,
        Event::WipeDone { ok: false },
    ], StoreState::Ready)]
    // A replay invalidated pass after pass is replayed again after a backoff, still recovering.
    #[case::incomplete_is_retried(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Incomplete },
    ], StoreState::Recovering)]
    #[case::incomplete_retried_then_ready(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Incomplete },
        Event::WaitedOut { wait: Wait(1) },
        Event::ReplayDone { replay: Replay { id: ReplayId(2), started_gen: 0 }, result: ReplayResult::Ok },
    ], StoreState::Ready)]
    // However often, at startup included: never unavailable for it (nor ready).
    #[case::incomplete_never_unavailable(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Incomplete },
        Event::WaitedOut { wait: Wait(1) },
        Event::ReplayDone { replay: Replay { id: ReplayId(2), started_gen: 0 }, result: ReplayResult::Incomplete },
        Event::WaitedOut { wait: Wait(3) },
        Event::ReplayDone { replay: Replay { id: ReplayId(4), started_gen: 0 }, result: ReplayResult::Incomplete },
        Event::WaitedOut { wait: Wait(5) },
        Event::ReplayDone { replay: Replay { id: ReplayId(6), started_gen: 0 }, result: ReplayResult::Incomplete },
    ], StoreState::Recovering)]
    // A rebuild during a backoff cuts it short: its replay starts at once, the timer firing
    // later is ignored, and that replay settles the store.
    #[case::rebuild_during_a_backoff(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Incomplete },
        Event::Rebuild,
        Event::WipeDone { ok: true },
        Event::WaitedOut { wait: Wait(1) },
        Event::ReplayDone { replay: Replay { id: ReplayId(2), started_gen: 1 }, result: ReplayResult::Ok },
    ], StoreState::Ready)]
    // As does the sync worker's forget.
    #[case::series_rewritten_during_a_backoff(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Ok },
        Event::SeriesRewritten,
        Event::WipeDone { ok: true },
        Event::ReplayDone { replay: Replay { id: ReplayId(1), started_gen: 1 }, result: ReplayResult::Incomplete },
        Event::SeriesRewritten,
        Event::WipeDone { ok: true },
        Event::ReplayDone { replay: Replay { id: ReplayId(3), started_gen: 2 }, result: ReplayResult::Ok },
    ], StoreState::Ready)]
    // A failed wipe during a backoff leaves it waited out, still recovering.
    #[case::failed_wipe_during_a_backoff(&[
        Event::ReplayDone { replay: Replay { id: ReplayId(0), started_gen: 0 }, result: ReplayResult::Incomplete },
        Event::Rebuild,
        Event::WipeDone { ok: false },
        Event::WaitedOut { wait: Wait(1) },
        Event::ReplayDone { replay: Replay { id: ReplayId(2), started_gen: 0 }, result: ReplayResult::Ok },
    ], StoreState::Ready)]
    fn named_interleavings(#[case] events: &[Event], #[case] expected: StoreState) {
        let mut world = World::boot();
        for &event in events {
            world.apply(event);
        }
        assert_eq!(world.core.state, expected);
        world.check_liveness();
    }

    /// 100ms, doubling with each incomplete replay in a row, up to 30s.
    #[rstest]
    #[case::first(1, Duration::from_millis(100))]
    #[case::second(2, Duration::from_millis(200))]
    #[case::fifth(5, Duration::from_millis(1_600))]
    #[case::capped(10, Duration::from_secs(30))]
    #[case::far_past_the_cap(40, Duration::from_secs(30))]
    #[case::the_last_retry(u32::MAX, Duration::from_secs(30))]
    fn the_backoff_doubles_up_to_its_cap(#[case] retry: u32, #[case] expected: Duration) {
        assert_eq!(Backoff::DEFAULT.delay(retry), expected);
    }
}
