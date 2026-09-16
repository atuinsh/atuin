use std::time::Duration;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::utils::uuid_v7;
use atuin_domain::record::{
    EncryptedData, Host, HostId, Record, RecordSeriesKey, RecordTag, RecordVersion,
};
use easy_cast::Conv;
use rand::Rng;
use rand::distributions::Alphanumeric;
use tempfile::TempDir;

use crate::_util::context::BenchCtx;

struct BenchRecord;

impl BenchRecord {
    /// Controls how large the record payload is. Roughly, this is between 200 and 400 bytes for
    /// a typical history record.
    ///
    /// Breakdown:
    ///  - id (UUID string, 36 bytes)
    ///  - timestamp (u64, 8 bytes)
    ///  - duration (i64, 8 bytes)
    ///  - exit code (i64, 8 bytes)
    ///  - command (variable — average shell command is ~20-50 bytes, but can be much longer)
    ///  - cwd (path string, ~20-60 bytes)
    ///  - session (string, ~36 bytes)
    ///  - hostname (string, ~10-30 bytes)
    ///  - deleted_at (optional u64)
    ///  - author (string)
    const PAYLOAD_SIZE: usize = 300;

    /// Rough size of the PASETO PIE-wrapped key.
    const KEY_SIZE: usize = 150;

    fn chain(ctx: &mut BenchCtx, n: usize) -> Vec<Record<EncryptedData>> {
        Self::chain_sized(ctx, n, Self::PAYLOAD_SIZE).0
    }

    /// Like [`chain`](Self::chain) but with a caller-chosen payload size, and returning the series
    /// key so a reader can query the populated `(host, tag)` series.
    fn chain_sized(
        ctx: &mut BenchCtx,
        n: usize,
        payload: usize,
    ) -> (Vec<Record<EncryptedData>>, RecordSeriesKey) {
        let host_id = HostId(uuid_v7());
        let host = Host::new(host_id);
        let version: String = "v1".into();
        let tag = uuid_v7().simple().to_string();
        let data: String =
            ctx.rng().sample_iter(&Alphanumeric).take(payload).map(char::from).collect();
        let key: String =
            ctx.rng().sample_iter(&Alphanumeric).take(Self::KEY_SIZE).map(char::from).collect();

        let records = (0..u64::conv(n))
            .map(|idx| {
                Record::builder()
                    .host(host.clone())
                    .version(RecordVersion::from(version.clone()))
                    .tag(RecordTag::Other(tag.clone()))
                    .data(EncryptedData {
                        raw: data.clone(),
                        cek: key.clone(),
                    })
                    .idx(idx)
                    .build()
            })
            .collect();

        (records, RecordSeriesKey::new(host_id, RecordTag::Other(tag)))
    }
}

struct BenchSqliteStore {
    _temp_dir: TempDir,
    sqlite_store: SqliteStore,
}

impl BenchSqliteStore {
    const SQL_TIMEOUT: Duration = Duration::from_secs(5);

    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("bench.db");
        let store = SqliteStore::new(db_path, Self::SQL_TIMEOUT).await.unwrap();

        Self {
            _temp_dir: dir,
            sqlite_store: store,
        }
    }
}

/// Benchmark to exercise the latency of pushing a batch of varying records.
/// The parameters are:
///  - 1 proves out the case of adding one shell entry via `push_record` (history/store.rs).
///  - 100 is the page size used by `sync_remote` (record/sync.rs).
#[divan::bench(args = [1, 10, 100], sample_count = 500, min_time = 1)]
fn push_batch(bencher: divan::Bencher, n: usize) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    bencher
        .with_inputs(|| {
            let mut ctx = BenchCtx::new();
            let db = rt.block_on(BenchSqliteStore::new());
            let records = BenchRecord::chain(&mut ctx, n);
            (db, records)
        })
        .bench_values(|(db, records)| {
            rt.block_on(db.sqlite_store.push_batch(records.iter())).unwrap();
        });
}

/// Allocating the next append index only needs the tail record's `idx`, yet `push_record` /
/// `push_batch` (history/store.rs) reach for the whole tail record via `last()` on every append.
/// These two benchmarks read the *same* populated series both ways, parametrized over the tail's
/// data-blob size: `last()` loads and decodes that blob (plus two UUID parses) only to discard
/// everything but `idx`, while the index-only probe stays flat.
///
/// The parameters are data-blob byte sizes: 256 is around a typical shell entry; 8192 a long
/// command line or kv value; 65536 a dotfiles/scripts record holding whole file contents. The
/// series holds 1000 records so both queries share the same `order by idx desc limit 1` index
/// seek; only the discarded payload differs, so `last()` diverges as it grows while the scalar
/// probe stays flat.
const TAIL_PAYLOADS: [usize; 3] = [256, 8192, 65536];
const SERIES_LEN: usize = 1000;

/// Queries issued per timed sample. A single `block_on` fixed cost (runtime entry) is otherwise
/// large enough to swamp the per-query decode difference, so we amortize it over a batch.
const PROBES_PER_SAMPLE: usize = 64;

/// In-memory so the timed work is the read-path CPU cost the fix changes (row + blob decode and
/// two UUID parses vs a scalar), not tempfile I/O jitter. Both benches share this identical setup.
fn populate_series(
    payload: usize,
) -> (tokio::runtime::Runtime, SqliteStore, RecordSeriesKey) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut ctx = BenchCtx::new();
    let (records, series) = BenchRecord::chain_sized(&mut ctx, SERIES_LEN, payload);
    let store = rt.block_on(SqliteStore::in_memory(Duration::from_secs(5))).unwrap();
    rt.block_on(store.push_batch(records.iter())).unwrap();
    (rt, store, series)
}

/// OLD: fetch the whole tail record just to take its `idx`.
#[divan::bench(args = TAIL_PAYLOADS, min_time = 2)]
fn tail_idx_via_last(bencher: divan::Bencher, payload: usize) {
    let (rt, store, series) = populate_series(payload);
    bencher.bench(|| {
        rt.block_on(async {
            for _ in 0..PROBES_PER_SAMPLE {
                let last = store.last(&series).await.unwrap();
                divan::black_box(last.map(|record| record.idx));
            }
        });
    });
}

/// NEW: read only the `idx` column via the covering `record_uniq` index.
#[divan::bench(args = TAIL_PAYLOADS, min_time = 2)]
fn tail_idx_via_scalar(bencher: divan::Bencher, payload: usize) {
    let (rt, store, series) = populate_series(payload);
    bencher.bench(|| {
        rt.block_on(async {
            for _ in 0..PROBES_PER_SAMPLE {
                divan::black_box(store.tail_idx(&series).await.unwrap());
            }
        });
    });
}
