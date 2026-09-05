//! Model-based check of the journal: any interleaving of start / finish / cancel / delete /
//! rebuild / register-output over a fixed set of commands must keep five views in agreement -- the
//! in-flight map, the history db, the search index, what another machine would rebuild from the
//! record store, and which commands have captured output -- and must emit exactly the events a
//! shell watching `atuin history tail` expects.
#![cfg(unix)]

mod common;

use std::collections::HashSet;
use std::time::Duration;

use atuin_client::history::{History, HistoryId};
use atuin_client::settings::Search;
use atuin_daemon::{CaptureError, CmdEvent, RegisterOutputError};
use common::{TestEnv, capture, history};
use futures::{FutureExt, StreamExt};
use proptest::prelude::*;

const SLOTS: u8 = 10;

#[derive(Debug, Clone)]
enum Op {
    Start(u8),
    Finish(u8),
    Cancel(u8),
    Delete(Vec<u8>),
    Rebuild,
    Register(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..SLOTS).prop_map(Op::Start),
        4 => (0..SLOTS).prop_map(Op::Finish),
        1 => (0..SLOTS).prop_map(Op::Cancel),
        2 => proptest::collection::vec(0..SLOTS, 0..4).prop_map(Op::Delete),
        1 => Just(Op::Rebuild),
        3 => (0..SLOTS).prop_map(Op::Register),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Fresh,
    InFlight,
    Persisted,
    Gone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Started,
    Finished,
    Cancelled,
}

fn kind_of(event: &CmdEvent) -> (Kind, HistoryId) {
    match event {
        CmdEvent::Started(h) => (Kind::Started, h.id),
        CmdEvent::Finished(h) => (Kind::Finished, h.id),
        CmdEvent::Cancelled(h) => (Kind::Cancelled, h.id),
    }
}

struct Model {
    slots: Vec<(History, State)>,
    /// Whether the journal holds captured output for the slot's command.
    has_output: Vec<bool>,
    expected_events: Vec<(Kind, HistoryId)>,
}

impl Model {
    fn new() -> Self {
        Self {
            slots: (0..SLOTS)
                .map(|i| (history(&format!("model cmd {i:02}")), State::Fresh))
                .collect(),
            has_output: vec![false; usize::from(SLOTS)],
            expected_events: Vec::new(),
        }
    }

    fn id(&self, slot: u8) -> HistoryId {
        self.slots[usize::from(slot)].0.id
    }

    fn persisted(&self) -> HashSet<HistoryId> {
        self.slots.iter().filter(|(_, s)| *s == State::Persisted).map(|(h, _)| h.id).collect()
    }
}

/// Apply one op to both the real journal and the model, updating the model's slot states and
/// expected event log to match what the journal did.
async fn apply(env: &TestEnv, model: &mut Model, op: &Op) {
    match op {
        Op::Start(slot) => {
            let (h, state) = &mut model.slots[usize::from(*slot)];
            if *state != State::Fresh {
                return; // ids are single-use over the wire; the model never restarts one
            }
            let id = env.journal.start_cmd(h.clone());
            assert_eq!(id, h.id);
            *state = State::InFlight;
            model.expected_events.push((Kind::Started, h.id));
        }
        Op::Finish(slot) => {
            let id = model.id(*slot);
            let result = env.journal.finish(id, 0, Duration::from_millis(1)).await;
            let (_, state) = &mut model.slots[usize::from(*slot)];
            if *state == State::InFlight {
                result.expect("finishing an in-flight command");
                *state = State::Persisted;
                model.expected_events.push((Kind::Finished, id));
            } else {
                assert!(result.is_err(), "finish of a {state:?} command must fail");
            }
        }
        Op::Cancel(slot) => {
            let id = model.id(*slot);
            let result = env.journal.cancel(id).await;
            let (_, state) = &mut model.slots[usize::from(*slot)];
            if *state == State::InFlight {
                result.expect("cancelling an in-flight command");
                *state = State::Gone;
                model.has_output[usize::from(*slot)] = false;
                model.expected_events.push((Kind::Cancelled, id));
            } else {
                assert!(result.is_err(), "cancel of a {state:?} command must fail");
            }
        }
        Op::Delete(slots) => {
            let ids: Vec<HistoryId> = slots.iter().map(|s| model.id(*s)).collect();
            let deleted = env
                .journal
                .delete(ids, &Search::default())
                .await
                .expect("delete never fails on healthy stores");
            assert_eq!(deleted, slots.len());
            for slot in slots {
                let id = model.id(*slot);
                model.has_output[usize::from(*slot)] = false;
                let (_, state) = &mut model.slots[usize::from(*slot)];
                match *state {
                    State::InFlight => {
                        *state = State::Gone;
                        model.expected_events.push((Kind::Cancelled, id));
                    }
                    // A tombstone now exists for this id. Over the wire ids are server-generated,
                    // so a tombstoned id can never be started later; the model mirrors that.
                    State::Persisted | State::Fresh => *state = State::Gone,
                    State::Gone => {}
                }
            }
        }
        Op::Register(slot) => {
            let id = model.id(*slot);
            let result = env.journal.register_command_output(id, capture("out")).await;
            let state = model.slots[usize::from(*slot)].1;
            let has_output = &mut model.has_output[usize::from(*slot)];
            match (state, *has_output) {
                (State::InFlight | State::Persisted, false) => {
                    result.expect("registering output for a live command");
                    *has_output = true;
                }
                (State::InFlight | State::Persisted, true) => assert!(
                    matches!(
                        result,
                        Err(RegisterOutputError::Capture(CaptureError::AlreadyExists))
                    ),
                    "a second capture for a live command must be refused: {result:?}"
                ),
                (State::Fresh | State::Gone, _) => assert!(
                    matches!(result, Err(RegisterOutputError::NotLive(_))),
                    "output for a {state:?} command must be refused: {result:?}"
                ),
            }
        }
        Op::Rebuild => env
            .journal
            .rebuild(&Search::default())
            .await
            .expect("rebuild never fails on healthy stores"),
    }
}

async fn check_invariants(env: &TestEnv, model: &Model, step: usize, op: &Op) {
    let ctx = format!("after step {step} ({op:?})");
    for (h, state) in &model.slots {
        assert_eq!(
            env.journal.get(h.id).is_ok(),
            *state == State::InFlight,
            "{ctx}: in-flight view of {}",
            h.command
        );
    }
    let persisted = model.persisted();
    assert_eq!(env.active_ids().await, persisted, "{ctx}: history db");
    assert_eq!(env.index_count().await, persisted.len(), "{ctx}: index size");
    for (h, state) in &model.slots {
        let hits = env.index_hits(&h.command).await;
        assert_eq!(
            hits.contains(&h.id),
            *state == State::Persisted,
            "{ctx}: index view of {}",
            h.command
        );
    }
    for (slot, (h, state)) in model.slots.iter().enumerate() {
        let stored = env.journal.get_command_output(h.id).await.unwrap().is_some();
        assert_eq!(stored, model.has_output[slot], "{ctx}: captured output of {}", h.command);
        assert!(
            !stored || matches!(state, State::InFlight | State::Persisted),
            "{ctx}: {} has output but is {state:?}",
            h.command
        );
    }
    let replayed = env.fresh_db_from_store().await;
    let mut replayed_ids = HashSet::new();
    let mut pager = replayed.all_paged(100, false, false);
    while let Some(page) = pager.next().await.unwrap() {
        replayed_ids.extend(page.into_iter().map(|h| h.id));
    }
    assert_eq!(replayed_ids, persisted, "{ctx}: another machine's replay of the store");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]

    #[test]
    fn journal_agrees_with_its_model(ops in proptest::collection::vec(op(), 1..24)) {
        common::current_thread_runtime().block_on(async {
            let env = TestEnv::builder().build().await;
            let mut events = env.journal.subscribe();
            let mut model = Model::new();

            for (step, op) in ops.iter().enumerate() {
                apply(&env, &mut model, op).await;
                check_invariants(&env, &model, step, op).await;
            }

            // Every event the journal broadcast, in order. All are already queued: the journal
            // sends before returning from each call.
            let mut observed = Vec::new();
            while let Some(Some(event)) = events.next().now_or_never() {
                observed.push(kind_of(&event.expect("no lag with <128 events")));
            }
            prop_assert_eq!(observed, model.expected_events);
            Ok(())
        })?;
    }
}
