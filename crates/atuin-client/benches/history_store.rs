use atuin_client::history::Version;
use atuin_client::history::store::HistoryRecord;
use atuin_domain::record::DecryptedData;

use crate::_util::context::BenchCtx;
use crate::history::BenchHistory;

/// Every recorded command is serialized before it is stored or synced, and every entry received
/// from the server is deserialized again (history/store.rs), so both directions sit on a hot path.
///
/// The parameters are:
///  - 1 proves out a single shell entry, as written by `atuin history end`.
///  - 100 is the page size used by `sync_remote` (record/sync.rs).
const BATCH_SIZES: [usize; 2] = [1, 100];

#[divan::bench(args = BATCH_SIZES, min_time = 1)]
fn serialize(bencher: divan::Bencher, n: usize) {
    bencher.with_inputs(|| records(n)).bench_values(|records: Vec<HistoryRecord>| {
        for record in &records {
            divan::black_box(record.serialize().unwrap());
        }
    });
}

#[divan::bench(args = BATCH_SIZES, min_time = 1)]
fn deserialize(bencher: divan::Bencher, n: usize) {
    bencher
        .with_inputs(|| {
            records(n)
                .iter()
                .map(|record| record.serialize().unwrap())
                .collect::<Vec<DecryptedData>>()
        })
        .bench_values(|serialized: Vec<DecryptedData>| {
            for bytes in &serialized {
                divan::black_box(
                    HistoryRecord::deserialize(bytes, Version::LATEST.name()).unwrap(),
                );
            }
        });
}

fn records(n: usize) -> Vec<HistoryRecord> {
    let mut ctx = BenchCtx::new();
    BenchHistory::count(&mut ctx, n).into_iter().map(HistoryRecord::Create).collect()
}
