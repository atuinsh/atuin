use std::sync::atomic::{AtomicU64, Ordering};

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_client::settings::DiskUsageLimit;
use atuin_daemon::OutputCaptureEngine;
use easy_cast::Conv;
use tempfile::TempDir;
use tokio::runtime::Runtime;

fn main() {
    divan::main();
}

const BODY_BYTES: usize = 5 * 1024;
const BATCH: usize = 100;
const WORDS: &[&str] = &[
    "error",
    "warning",
    "compiled",
    "target",
    "debug",
    "release",
    "finished",
    "running",
    "cargo",
    "build",
    "failed",
    "missing",
    "semicolon",
    "expected",
    "found",
    "unused",
    "import",
    "variable",
    "function",
    "module",
];

static NEXT: AtomicU64 = AtomicU64::new(1);

fn next() -> (u64, HistoryId) {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    (n, HistoryId::from_bytes(*uuid::Uuid::from_u128(u128::from(n)).as_bytes()))
}

fn body(seed: u64) -> String {
    let mut out = String::with_capacity(BODY_BYTES + 16);
    let mut n = seed;
    while out.len() < BODY_BYTES {
        n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        out.push_str(WORDS[usize::conv(n >> 33) % WORDS.len()]);
        out.push(if n.trailing_zeros() >= 5 {
            '\n'
        } else {
            ' '
        });
    }
    out
}

fn cap(seed: u64) -> CommandCapture {
    let output_start = body(seed);
    CommandCapture {
        output_observed_bytes: u64::conv(output_start.len()),
        output_start,
        output_end: None,
        terminal_width: 80,
        terminal_height: 24,
    }
}

struct Engine {
    engine: OutputCaptureEngine,
    _dir: TempDir,
}

fn open(rt: &Runtime) -> Engine {
    let dir = tempfile::tempdir().unwrap();
    let engine = rt
        .block_on(OutputCaptureEngine::open(dir.path().join("capture"), DiskUsageLimit::Unlimited));
    Engine { engine, _dir: dir }
}

fn capture_n(rt: &Runtime, engine: &OutputCaptureEngine, n: usize) -> Vec<HistoryId> {
    rt.block_on(async {
        let mut ids = Vec::with_capacity(n);
        for _ in 0..n {
            let (seed, id) = next();
            engine.capture(id, cap(seed)).await.unwrap();
            ids.push(id);
        }
        ids
    })
}

fn seeded(rt: &Runtime, n: usize) -> Engine {
    let e = open(rt);
    capture_n(rt, &e.engine, n);
    e
}

#[divan::bench(args = [1000, 4000, 8000], sample_count = 10, sample_size = 5)]
fn capture(bencher: divan::Bencher, n: usize) {
    let rt = Runtime::new().unwrap();
    let e = seeded(&rt, n);
    bencher
        .with_inputs(|| {
            let (seed, id) = next();
            (id, cap(seed))
        })
        .bench_values(|(id, c)| rt.block_on(e.engine.capture(id, c)).unwrap());
}

#[divan::bench(args = [1000, 4000], sample_count = 5, sample_size = 2)]
fn remove_batch(bencher: divan::Bencher, n: usize) {
    let rt = Runtime::new().unwrap();
    let e = seeded(&rt, n);
    bencher
        .with_inputs(|| capture_n(&rt, &e.engine, BATCH))
        .bench_values(|ids| rt.block_on(e.engine.remove(ids)).unwrap());
}

#[divan::bench(args = [10, 50, 200], sample_count = 10, sample_size = 5)]
fn search(bencher: divan::Bencher, limit: usize) {
    let rt = Runtime::new().unwrap();
    let e = seeded(&rt, 4000);
    let store = e.engine.store();
    bencher.bench_local(|| {
        rt.block_on(async {
            store.search("error", limit, Some(2)).await.try_collect::<Vec<_>>().await
        })
        .unwrap()
    });
}

#[divan::bench(args = [1000, 2000, 10_000], sample_count = 3, sample_size = 1)]
fn fill(bencher: divan::Bencher, n: usize) {
    let rt = Runtime::new().unwrap();
    bencher.with_inputs(|| open(&rt)).bench_values(|e| {
        capture_n(&rt, &e.engine, n);
        e
    });
}
