use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::store::Store;
use atuin_common::record::{EncryptedData, Host, HostId, Record};
use atuin_common::utils::uuid_v7;
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
        let host = Host::new(HostId(uuid_v7()));
        let version: String = "v1".into();
        let tag = uuid_v7().simple().to_string();
        let data: String = ctx
            .rng()
            .sample_iter(&Alphanumeric)
            .take(Self::PAYLOAD_SIZE)
            .map(char::from)
            .collect();
        let key: String = ctx
            .rng()
            .sample_iter(&Alphanumeric)
            .take(Self::KEY_SIZE)
            .map(char::from)
            .collect();

        (0..n as u64)
            .map(|idx| {
                Record::builder()
                    .host(host.clone())
                    .version(version.clone())
                    .tag(tag.clone())
                    .data(EncryptedData {
                        data: data.clone(),
                        content_encryption_key: key.clone(),
                    })
                    .idx(idx)
                    .build()
            })
            .collect()
    }
}

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
///  - 100 is the old page size used by `sync_remote` (record/sync.rs).
///  - 1000 is the new page size used by `sync_remote`.
#[divan::bench(args = [1, 10, 100, 1000], sample_count = 500, min_time = 1)]
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
