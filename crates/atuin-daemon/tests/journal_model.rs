//! Model-based check of the journal: any interleaving of start / finish / cancel / delete /
//! rebuild over a fixed set of commands must keep four views in agreement -- the in-flight map,
//! the history db, the search index, and what another machine would rebuild from the record
//! store -- and must emit exactly the events a shell watching `atuin history tail` expects.
#![cfg(unix)]

mod common;

use std::collections::HashSet;
use std::time::Duration;

use atuin_client::history::{History, HistoryId};
use atuin_client::settings::Search;
use atuin_daemon::CmdEvent;
use common::{TestEnv, history};
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
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..SLOTS).prop_map(Op::Start),
        4 => (0..SLOTS).prop_map(Op::Finish),
        1 => (0..SLOTS).prop_map(Op::Cancel),
        2 => proptest::collection::vec(0..SLOTS, 0..4).prop_map(Op::Delete),
        1 => Just(Op::Rebuild),
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
    expected_events: Vec<(Kind, HistoryId)>,
}

impl Model {
    fn new() -> Self {
        Self {
            slots: (0..SLOTS)
                .map(|i| (history(&format!("model cmd {i:02}")), State::Fresh))
                .collect(),
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
