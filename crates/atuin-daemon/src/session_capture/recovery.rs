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
//! is a backoff running out.
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
use super::hooks::{Fault, Point};
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
                Effect::StartReplay(replay) => self.start_replay(replay),
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
        let wiped = {
            let _local = self.sink.sidecar.lock_local_projection().await;
            #[cfg(test)]
            self.sink.hook(Point::WipeLocked).await;
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
        let timer = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(Msg::WaitedOut(wait));
        });
        self.waiting = Some(timer.abort_handle());
    }
}

/// Replay the sidecar from the record store.
async fn run_replay(sink: Arc<Sink>, progress: ReprojectProgress) -> ReplayResult {
    let started = std::time::Instant::now();
    // Capture is held off (the store is recovering while a replay runs), so a series found
    // rewritten is forgotten here and now, this host's included.
    let result = sink.records.reproject_with(&sink.sidecar, &progress).await;
    #[cfg(test)]
    if matches!(result, Err(BuildError::Incomplete)) {
        sink.hook(Point::ReplayIncomplete).await;
    }
    #[cfg(test)]
    if sink.hook(Point::ReplayBeforeSettle).await == Fault::Panic {
        std::panic::panic_any(super::hooks::INJECTED_PANIC);
    }
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
    use rstest::rstest;

    use super::*;

    /// The replay `id`, started at generation `started_gen`.
    fn replay(id: u64, started_gen: u64) -> Replay {
        Replay {
            id: ReplayId(id),
            started_gen,
        }
    }

    /// The known-bad interleavings, by name: the state the store ends in. Recovering (its wipe
    /// over), it always has a replay running, or one waited for, to end it.
    #[rstest]
    // A rebuild's wipe while startup recovery replays: that replay goes round again.
    #[case::rebuild_during_recovery(&[Event::Rebuild, Event::WipeDone { ok: true }], StoreState::Recovering)]
    // A failed wipe with no replay running restores the store as it was.
    #[case::failed_wipe_when_ready(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Ok },
        Event::Rebuild,
        Event::WipeDone { ok: false },
    ], StoreState::Ready)]
    // A replay that panicked after a rebuild: the store is unavailable, and a rebuild replays.
    #[case::rebuild_after_a_panic(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Ok },
        Event::Rebuild,
        Event::WipeDone { ok: true },
        Event::ReplayDone { replay: replay(1, 1), result: ReplayResult::Panicked },
        Event::Rebuild,
        Event::WipeDone { ok: true },
    ], StoreState::Recovering)]
    // A failed startup recovery refuses rebuilds.
    #[case::failed_startup(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Failed },
        Event::Rebuild,
    ], StoreState::Unavailable)]
    // And the sync worker's forget: capture never started.
    #[case::series_rewritten_after_failed_startup(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Failed },
        Event::SeriesRewritten,
    ], StoreState::Unavailable)]
    // This host's series rewritten while ready: capture is held off before the forget, and a
    // replay follows.
    #[case::series_rewritten_while_ready(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Ok },
        Event::SeriesRewritten,
    ], StoreState::Recovering)]
    #[case::series_rewritten_then_replayed(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Ok },
        Event::SeriesRewritten,
        Event::WipeDone { ok: true },
        Event::ReplayDone { replay: replay(1, 1), result: ReplayResult::Ok },
    ], StoreState::Ready)]
    // A failed forget restores the store as it was: the next sync finds the series again.
    #[case::failed_forget_when_ready(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Ok },
        Event::SeriesRewritten,
        Event::WipeDone { ok: false },
    ], StoreState::Ready)]
    // A replay invalidated pass after pass is replayed again after a backoff, still recovering.
    #[case::incomplete_is_retried(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Incomplete },
    ], StoreState::Recovering)]
    #[case::incomplete_retried_then_ready(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Incomplete },
        Event::WaitedOut { wait: Wait(1) },
        Event::ReplayDone { replay: replay(2, 0), result: ReplayResult::Ok },
    ], StoreState::Ready)]
    // However often, at startup included: never unavailable for it (nor ready).
    #[case::incomplete_never_unavailable(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Incomplete },
        Event::WaitedOut { wait: Wait(1) },
        Event::ReplayDone { replay: replay(2, 0), result: ReplayResult::Incomplete },
        Event::WaitedOut { wait: Wait(3) },
        Event::ReplayDone { replay: replay(4, 0), result: ReplayResult::Incomplete },
        Event::WaitedOut { wait: Wait(5) },
        Event::ReplayDone { replay: replay(6, 0), result: ReplayResult::Incomplete },
    ], StoreState::Recovering)]
    // A rebuild during a backoff cuts it short: its replay starts at once, the timer firing
    // later is ignored, and that replay settles the store.
    #[case::rebuild_during_a_backoff(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Incomplete },
        Event::Rebuild,
        Event::WipeDone { ok: true },
        Event::WaitedOut { wait: Wait(1) },
        Event::ReplayDone { replay: replay(2, 1), result: ReplayResult::Ok },
    ], StoreState::Ready)]
    // As does the sync worker's forget.
    #[case::series_rewritten_during_a_backoff(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Ok },
        Event::SeriesRewritten,
        Event::WipeDone { ok: true },
        Event::ReplayDone { replay: replay(1, 1), result: ReplayResult::Incomplete },
        Event::SeriesRewritten,
        Event::WipeDone { ok: true },
        Event::ReplayDone { replay: replay(3, 2), result: ReplayResult::Ok },
    ], StoreState::Ready)]
    // A failed wipe during a backoff leaves it waited out, still recovering.
    #[case::failed_wipe_during_a_backoff(&[
        Event::ReplayDone { replay: replay(0, 0), result: ReplayResult::Incomplete },
        Event::Rebuild,
        Event::WipeDone { ok: false },
        Event::WaitedOut { wait: Wait(1) },
        Event::ReplayDone { replay: replay(2, 0), result: ReplayResult::Ok },
    ], StoreState::Ready)]
    fn named_interleavings(#[case] events: &[Event], #[case] expected: StoreState) {
        let (mut core, _) = CoordState::boot();
        for &event in events {
            core.step(event);
        }
        assert_eq!(core.state, expected);
        if core.state == StoreState::Recovering && core.wiping.is_none() {
            assert!(core.replay.is_some() || core.backoff.is_some(), "stalled: {core:?}");
        }
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
