use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::store::Store;
use tempfile::TempDir;

use crate::_util::context::BenchCtx;
use crate::_util::record::BenchRecord;

struct BenchSqliteStore {
    _temp_dir: TempDir,
    sqlite_store: SqliteStore,
}

impl BenchSqliteStore {
    const SQL_TIMEOUT_S: f64 = 5.0;

    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("bench.db");
        let store = SqliteStore::new(db_path, Self::SQL_TIMEOUT_S)
            .await
            .unwrap();

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
            rt.block_on(db.sqlite_store.push_batch(records.iter()))
                .unwrap();
        });
}
