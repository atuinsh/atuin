//! Head-to-head for the daemon's finish path appending a `Create` record.
//!
//! When a command finishes, the daemon serializes the `History` into the record store but still
//! needs the entry afterwards (search index + broadcast). The old code passed it by value to
//! `HistoryStore::push`, which forced a full `History::clone` at the call site. The new
//! `serialize_create`/`push_ref` path serializes straight from the borrow.
//!
//! `old` reproduces the clone-then-serialize the value-taking `push` required; `new` is the
//! borrow-only `serialize_create`. Both emit byte-identical records, so this isolates exactly the
//! avoided clone.

use atuin_client::history::History;
use atuin_client::history::store::HistoryRecord;

/// Batch sizes: 1 is a single finished command (`atuin history end`); 100 mirrors the record sync
/// page size, i.e. a burst of finishes replayed back to back.
const BATCH_SIZES: [usize; 2] = [1, 100];

fn main() {
    divan::main();
}

/// OLD: `push(history.clone())` — clone the entry, then serialize the owning `Create`.
#[divan::bench(args = BATCH_SIZES, min_time = 1)]
fn old(bencher: divan::Bencher, n: usize) {
    bencher.with_inputs(|| histories(n)).bench_values(|histories: Vec<History>| {
        for history in &histories {
            let record = HistoryRecord::Create(history.clone());
            divan::black_box(record.serialize().unwrap());
        }
    });
}

/// NEW: `push_ref(&history)` — serialize straight from the borrow, no clone.
#[divan::bench(args = BATCH_SIZES, min_time = 1)]
fn new(bencher: divan::Bencher, n: usize) {
    bencher.with_inputs(|| histories(n)).bench_values(|histories: Vec<History>| {
        for history in &histories {
            divan::black_box(HistoryRecord::serialize_create(history).unwrap());
        }
    });
}

/// Realistic finished commands: non-trivial command lines, a real-looking cwd, and the owned
/// `String` fields (session, author, shell) that make cloning a `History` cost something.
fn histories(n: usize) -> Vec<History> {
    const COMMANDS: [&str; 6] = [
        "cargo build --release --features sqlite",
        "git commit -m 'perf: append finished history by reference'",
        "curl -s https://api.atuin.sh/v0/sync/status",
        "grep -rn push_ref crates/atuin-client/src",
        "docker compose -f infra/docker-compose.yml up -d",
        "kubectl rollout status deployment/atuin-server -n prod",
    ];

    (0..n)
        .map(|i| {
            let offset = i64::try_from(i).unwrap();
            History::import()
                .command(COMMANDS[i % COMMANDS.len()])
                .cwd("/Users/atuin/src/github.com/atuinsh/atuin")
                .timestamp(time::OffsetDateTime::from_unix_timestamp(1_700_000_000 + offset).unwrap())
                .session("018cd4fead897597852527a31c998059")
                .author("atuin")
                .shell("bash")
                .build()
                .into()
        })
        .collect()
}
