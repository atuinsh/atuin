//! `atuin sync` with a daemon running, end to end: the CLI syncs, then has the daemon rebuild its
//! search index, so what the sync downloaded is in the daemon's search straight away.

#![cfg(all(unix, feature = "daemon", feature = "ai", feature = "sync"))]

mod common;

#[allow(dead_code, reason = "shared with the AI session tests")]
#[path = "common/agents.rs"]
mod agents;
#[allow(dead_code, reason = "shared with the AI session tests")]
#[path = "common/ai.rs"]
mod ai;

use ai::{Machine, SyncServer};
use atuin_client::settings::FilterMode;
use atuin_common::filter::OrFilter;
use atuin_daemon::client::{SearchClient, SearchParams};
use common::{SESSION, marker};
use rstest::rstest;

/// How many commands `machine`'s daemon finds for `query`. On a runtime of its own, as the sync
/// server starts its own and runtimes can't nest.
fn daemon_finds(machine: &Machine, query: &str) -> usize {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(search(machine, query))
}

async fn search(machine: &Machine, query: &str) -> usize {
    let mut client = SearchClient::new(machine.env.socket()).await.unwrap();
    let mut replies = client
        .search(SearchParams {
            query: query.to_owned(),
            query_id: 1,
            filter_mode: FilterMode::Global,
            context: None,
            shells: OrFilter::default(),
        })
        .await
        .unwrap();

    let mut found = 0;
    while let Some(reply) = replies.message().await.unwrap() {
        found += reply.ids.len();
    }
    found
}

/// A command synced down by `atuin sync` is found by the daemon's search without restarting it.
#[rstest]
fn synced_history_is_searchable_through_the_daemon() {
    let server = SyncServer::start();
    // The second machine's daemon never syncs on its own, so only `atuin sync` brings it anything.
    let (one, two) = Machine::pair_syncing(&server, 0, "capture = true\n");

    let word = marker();
    one.env.record(&format!("echo {word}"), SESSION, one.home());
    one.run(&["sync"]);

    assert_eq!(daemon_finds(&two, &word), 0, "found before syncing");

    let synced = two.run(&["sync"]);

    assert_eq!(daemon_finds(&two, &word), 1, "sync printed:\n{synced}\n{}", two.logs());
}
