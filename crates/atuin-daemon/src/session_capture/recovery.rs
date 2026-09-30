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
//! time: rebuild requests, and replays reporting they are done (a supervisor task turns a replay
//! that panicked or was aborted into a report too). It wipes inline, holding capture's lock, so
//! nothing interleaves with a wipe but what the locks allow.
//!
//! Its decisions are a pure transition function, [`CoordState::step`], from an [`Event`] to the
//! [`Effect`]s the coordinator then carries out; a wipe's outcome is fed back as an event. The
//! tests below check the protocol's invariants over every event sequence up to a bound, and over
//! random longer ones.

use std::collections::VecDeque;
use std::sync::Arc;

use atuin_client::ai_session::{DbError, ReprojectProgress};
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
}

/// What the coordinator does, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Effect {
    /// Set the store's state.
    Broadcast(StoreState),
    /// Empty the sidecar under capture's lock, then report [`Event::WipeDone`].
    Wipe,
    /// Spawn a replay.
    StartReplay(Replay),
    /// Answer the rebuild being handled.
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
    /// Whether a replay has made the store ready: then capture started, and a store left
    /// unavailable by a failed replay can be rebuilt again. Until then, a failed startup recovery
    /// leaves it unavailable until restart.
    pub(super) recovered: bool,
    /// While a rebuild's wipe runs: the state it replaced, restored should the wipe fail with no
    /// replay running.
    pub(super) wiping: Option<StoreState>,
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
            recovered: false,
            wiping: None,
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
            Event::Rebuild => self.rebuild(),
            Event::WipeDone { ok } => self.wiped(ok),
            Event::ReplayDone { replay, result } => self.replayed(replay, result),
        }
    }

    fn rebuild(&mut self) -> Vec<Effect> {
        if self.state == StoreState::Unavailable && !self.recovered {
            return vec![Effect::Reply(Err(Refusal::Unavailable))];
        }
        self.wiping = Some(self.state);
        self.state = StoreState::Recovering;
        // Said before the wipe takes capture's lock, which capture checks again under that lock:
        // no capture checks the sidecar from then until a replay settles.
        vec![Effect::Broadcast(StoreState::Recovering), Effect::Wipe]
    }

    fn wiped(&mut self, ok: bool) -> Vec<Effect> {
        let Some(before) = self.wiping.take() else {
            debug_assert!(false, "a wipe reported with none running");
            return Vec::new();
        };
        let mut effects = Vec::new();
        if !ok {
            // Nothing was deleted. A replay running settles the state; otherwise the store is as
            // it was: ready, or unavailable.
            if self.replay.is_none() {
                self.state = before;
                effects.push(Effect::Broadcast(before));
            }
            effects.push(Effect::Reply(Err(Refusal::WipeFailed)));
            return effects;
        }
        self.generation += 1;
        // A replay running notices the wipe when it reports, and is replayed again.
        if self.replay.is_none() {
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
            return vec![self.start_replay()];
        }
        self.state = match result {
            ReplayResult::Ok => {
                self.recovered = true;
                StoreState::Ready
            }
            ReplayResult::Failed | ReplayResult::Panicked => StoreState::Unavailable,
        };
        vec![Effect::Broadcast(self.state)]
    }

    fn start_replay(&mut self) -> Effect {
        debug_assert!(self.replay.is_none(), "a second replay");
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
    ReplayDone {
        replay: Replay,
        result: ReplayResult,
    },
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
    /// The replay running, aborted with the coordinator.
    running: Option<AbortHandle>,
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        // Only stops the task: nothing is left to report to.
        if let Some(running) = &self.running {
            running.abort();
        }
    }
}

impl Coordinator {
    /// Spawn the coordinator, recovering at once. Returns its mailbox and the store's state.
    pub(super) fn spawn(
        sink: Arc<Sink>,
        state: watch::Sender<StoreState>,
        progress: ReprojectProgress,
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
            running: None,
        };
        let task = tokio::spawn(coordinator.run(boot));
        (tx, task)
    }

    async fn run(mut self, boot: Vec<Effect>) {
        self.apply(boot, None).await;
        while let Some(msg) = self.rx.recv().await {
            match msg {
                Msg::Rebuild(reply) => {
                    let effects = self.core.step(Event::Rebuild);
                    self.apply(effects, Some(reply)).await;
                }
                Msg::ReplayDone { replay, result } => {
                    if self.core.replay == Some(replay) {
                        self.running = None;
                    }
                    let effects = self.core.step(Event::ReplayDone { replay, result });
                    self.apply(effects, None).await;
                }
            }
        }
    }

    /// Carry out `effects` in order, answering `reply` (the rebuild being handled).
    async fn apply(
        &mut self,
        effects: Vec<Effect>,
        mut reply: Option<oneshot::Sender<Result<(), RebuildError>>>,
    ) {
        let mut effects = VecDeque::from(effects);
        let mut wipe_error = None;
        while let Some(effect) = effects.pop_front() {
            match effect {
                Effect::Broadcast(state) => self.broadcast(state),
                Effect::Wipe => {
                    let wiped = self.wipe().await;
                    let ok = wiped.is_ok();
                    if let Err(err) = wiped {
                        tracing::error!(
                            ?err,
                            "failed to empty the ai-session sidecar to rebuild it"
                        );
                        wipe_error = Some(err);
                    }
                    effects.extend(self.core.step(Event::WipeDone { ok }));
                }
                Effect::StartReplay(replay) => self.start_replay(replay),
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

    /// Empty the sidecar, holding capture's lock so no capture is between its check and its push.
    async fn wipe(&self) -> Result<(), DbError> {
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
            self.sink.sidecar.reset().await
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
}

/// Replay the sidecar from the record store.
async fn run_replay(sink: Arc<Sink>, progress: ReprojectProgress) -> ReplayResult {
    #[cfg(test)]
    let _section = sink.enter(Section::Replay);
    let started = std::time::Instant::now();
    let result = sink.records.reproject_with(&sink.sidecar, &progress).await;
    #[cfg(test)]
    let result = match sink.hook(Point::ReplayBeforeSettle).await {
        Fault::None => result,
        Fault::Fail => Err(DbError::InvalidRecordId.into()),
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

    /// The world around the coordinator's pure core: which replays run, what each rebuild was
    /// answered, and the last replay the coordinator heard from. Checks the protocol's invariants
    /// after every step.
    #[derive(Debug, Clone)]
    struct World {
        core: CoordState,
        /// Replay tasks alive (spawned, not yet reported).
        live: BTreeSet<u64>,
        /// Every replay ever started, to report stale ones.
        started: Vec<Replay>,
        /// Rebuilds asked for and not answered yet.
        unanswered: usize,
        /// The last replay the coordinator acted on: how it ended, and the generation it started
        /// at.
        last_done: Option<(u64, ReplayResult)>,
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
                unanswered: 0,
                last_done: None,
                broadcasts: Vec::new(),
            };
            world.run(&effects);
            world.check();
            world
        }

        /// The events that can happen next. Stale reports come from replays that already
        /// reported, and from made-up ones.
        fn enabled(&self) -> Vec<Event> {
            if self.core.wiping.is_some() {
                return vec![Event::WipeDone { ok: true }, Event::WipeDone { ok: false }];
            }
            let mut events = vec![Event::Rebuild];
            if let Some(replay) = self.core.replay {
                for result in [ReplayResult::Ok, ReplayResult::Failed, ReplayResult::Panicked] {
                    events.push(Event::ReplayDone { replay, result });
                }
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
            events
        }

        fn apply(&mut self, event: Event) {
            let before = self.core.clone();
            if let Event::Rebuild = event {
                self.unanswered += 1;
            }
            let current = matches!(event, Event::ReplayDone { replay, .. } if Some(replay) == self.core.replay);
            if let Event::ReplayDone { replay, result } = event
                && current
            {
                assert!(self.live.remove(&replay.id.0), "a report from a replay not running");
                self.last_done = Some((replay.started_gen, result));
            }
            let effects = self.core.step(event);
            if matches!(event, Event::ReplayDone { .. }) && !current {
                assert_eq!(self.core, before, "a stale report changed the coordinator");
                assert!(effects.is_empty(), "a stale report did something");
            }
            self.run(&effects);
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
                    Effect::Wipe => {
                        assert_eq!(i, effects.len() - 1, "the wipe is the last effect");
                        assert_eq!(
                            state,
                            Some(StoreState::Recovering),
                            "recovering is said before wiping"
                        );
                    }
                    Effect::StartReplay(replay) => {
                        // I4: at most one replay.
                        assert!(self.live.is_empty(), "a second replay started: {:?}", self.live);
                        assert_eq!(replay.started_gen, self.core.generation);
                        self.live.insert(replay.id.0);
                        self.started.push(replay);
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
            // The core's replay is the live one.
            assert_eq!(core.replay.map(|r| r.id.0).into_iter().collect::<BTreeSet<_>>(), self.live);
            // I4.
            assert!(self.live.len() <= 1);
            // The last state said is the core's.
            assert_eq!(self.broadcasts.last(), Some(&core.state));
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
                // L2: no stall. Recovering always has a replay running to end it.
                StoreState::Recovering => assert!(core.replay.is_some(), "stalled: {core:?}"),
                // Unavailable only if the last replay heard from started at the current generation
                // and failed: one that started before a wipe is replayed again rather than end a
                // rebuild that was answered ok (which would wait on a replay that never comes, or
                // be refused as unavailable with none running).
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

        /// Once events stop: a wipe running reports (failing unless `result` is ok), the replay
        /// running, if any, ends with `result`, and the store settles after at most one more.
        fn settle(mut self, result: ReplayResult) -> Self {
            if self.core.wiping.is_some() {
                self.apply(Event::WipeDone {
                    ok: result == ReplayResult::Ok,
                });
            }
            // The replay running, then (if a wipe came after it started) one more.
            for _ in 0..2 {
                if let Some(replay) = self.core.replay {
                    self.apply(Event::ReplayDone { replay, result });
                }
            }
            assert!(self.core.replay.is_none(), "settling needs at most two replays");
            assert_ne!(self.core.state, StoreState::Recovering);
            self
        }

        /// L1: once rebuilds stop and nothing fails, the store ends ready. (A failed startup
        /// recovery excepted: that store stays unavailable until restart.) A store left
        /// unavailable by a failed rebuild takes one more rebuild.
        fn check_liveness(&self) {
            let settled = self.clone().settle(ReplayResult::Ok);
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
                    let again = again.settle(ReplayResult::Ok);
                    assert_eq!(again.core.state, StoreState::Ready);
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
        explore(&World::boot(), depth.unwrap_or(10), &mut visited);
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
        proptest!(config, |(choices in proptest::collection::vec(any::<usize>(), 0..200))| {
            let mut world = World::boot();
            for choice in choices {
                let event = pick(&world, choice);
                world.apply(event);
                world.check_liveness();
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
    fn named_interleavings(#[case] events: &[Event], #[case] expected: StoreState) {
        let mut world = World::boot();
        for &event in events {
            world.apply(event);
        }
        assert_eq!(world.core.state, expected);
        world.check_liveness();
    }
}
