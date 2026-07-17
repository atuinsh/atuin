//! End-to-end record sync benchmarks.
//!
//! These drive a real `atuin-client` against a real in-process `atuin-server` over a real TCP
//! socket on loopback, with a configurable round-trip time injected server-side. See
//! `_util::server::latency` for why the RTT is injected and what it does and does not model.
//!
//! They benchmark `sync_remote` rather than `sync::sync` because `sync_remote` takes `page_size`
//! as a parameter while `sync::sync` hardcodes it (`record/sync.rs:368`) — and because
//! `sync::sync` resolves its auth token through a process-wide `OnceCell` meta store, which a
//! benchmark that starts several servers cannot use.
//!
//! `diff` and `operations` run in untimed setup, so each sample times the paged transfer alone.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p atuin-client --bench benchmarks -- sync 2>/tmp/atuin-bench.stderr
//! ```
//!
//! Redirecting stderr is not optional. `sync_upload`/`sync_download` draw an `indicatif` progress
//! bar to stderr, which indicatif suppresses only when stderr is not a TTY. Left attached to a
//! terminal it adds real, page-count-proportional work, so a terminal run and a CI run measure
//! different things.

use std::time::Duration;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::store::Store;
use atuin_client::record::sync::{diff, operations, sync_remote};

use crate::_util::context::BenchCtx;
use crate::_util::record::BenchRecord;
use crate::_util::server::BenchServer;

/// The size of the corpus each sample transfers. Chosen to resemble a first sync of a real shell
/// history, and to make the page-count difference between 100 and 1000 large (100 requests vs 10).
const RECORDS: usize = 10_000;

/// Matches the timeout the sqlite_store bench uses. `settings::test_local_timeout` is
/// `#[cfg(test)]`, so it is not reachable from a bench target.
const SQL_TIMEOUT_S: f64 = 5.0;

#[derive(Clone, Copy)]
pub struct SyncArg {
    /// The `page_size` handed to `sync_remote`. 100 is what `sync::sync` uses today; 1000 is what
    /// PR #3584 proposed.
    pub page_size: u64,
    /// Injected server-side round-trip time. 0 is bare loopback, 20ms a same-region server, 100ms
    /// a cross-continent one.
    pub rtt_ms: u64,
}

impl std::fmt::Display for SyncArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "page={} rtt={}ms", self.page_size, self.rtt_ms)
    }
}

pub const ARGS: [SyncArg; 6] = [
    SyncArg {
        page_size: 100,
        rtt_ms: 0,
    },
    SyncArg {
        page_size: 1000,
        rtt_ms: 0,
    },
    SyncArg {
        page_size: 100,
        rtt_ms: 20,
    },
    SyncArg {
        page_size: 1000,
        rtt_ms: 20,
    },
    SyncArg {
        page_size: 100,
        rtt_ms: 100,
    },
    SyncArg {
        page_size: 1000,
        rtt_ms: 100,
    },
];

/// Client holds `RECORDS` records, server holds none: the full corpus goes up.
///
/// `sample_size = 1` is required — divan would otherwise calibrate a sample size against a
/// multi-second iteration and run for a very long time.
#[divan::bench(args = ARGS, sample_count = 5, sample_size = 1)]
fn upload(bencher: divan::Bencher, arg: SyncArg) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(BenchServer::start(Duration::from_millis(arg.rtt_ms)));
    let client = rt.block_on(server.register());

    bencher
        .with_inputs(|| {
            rt.block_on(async {
                // Uploading mutates the server, so every sample needs it emptied first.
                client.delete_store().await.unwrap();

                let dir = tempfile::tempdir().unwrap();
                let store = SqliteStore::new(dir.path().join("records.db"), SQL_TIMEOUT_S)
                    .await
                    .unwrap();

                let mut ctx = BenchCtx::new();
                let records = BenchRecord::chain(&mut ctx, RECORDS);
                store.push_batch(records.iter()).await.unwrap();

                let (diffs, _) = diff(&client, &store).await.unwrap();
                let ops = operations(diffs, &store).await.unwrap();

                (dir, store, ops)
            })
        })
        .bench_values(|(_dir, store, ops)| {
            let (uploaded, _) = rt
                .block_on(sync_remote(&client, ops, &store, arg.page_size))
                .unwrap();

            // A silently empty sync would benchmark nothing at all, very quickly.
            assert_eq!(uploaded, RECORDS as i64);
        });
}
