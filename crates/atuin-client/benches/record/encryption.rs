use atuin_client::history::Version;
use atuin_client::history::store::HistoryRecord;
use atuin_common::utils::uuid_v7;
use atuin_domain::record::{
    DecryptedData, EncryptedData, Host, HostId, Record, RecordTag, RecordVersion,
};

use crate::_util::context::BenchCtx;
use crate::history::BenchHistory;

/// Records are encrypted before they leave the machine and decrypted again when they are pulled
/// back down, so PASETO wrapping/unwrapping runs once per synced entry (record/sync.rs).
///
/// The parameters are:
///  - 1 proves out the case of encrypting a single new shell entry.
///  - 100 is the page size used by `sync_remote` (record/sync.rs).
const BATCH_SIZES: [usize; 2] = [1, 100];

/// Fixed wrapping key. Real keys are user specific, but the work done is key independent.
const KEY: [u8; 32] = [0x42; 32];

#[divan::bench(args = BATCH_SIZES, min_time = 1)]
fn encrypt(bencher: divan::Bencher, n: usize) {
    bencher.with_inputs(|| decrypted_records(n)).bench_values(
        |records: Vec<Record<DecryptedData>>| {
            for record in records {
                divan::black_box(record.encrypt(&KEY.into()));
            }
        },
    );
}

#[divan::bench(args = BATCH_SIZES, min_time = 1)]
fn decrypt(bencher: divan::Bencher, n: usize) {
    bencher
        .with_inputs(|| {
            decrypted_records(n)
                .into_iter()
                .map(|record| record.encrypt(&KEY.into()))
                .collect::<Vec<Record<EncryptedData>>>()
        })
        .bench_values(|records: Vec<Record<EncryptedData>>| {
            for record in records {
                divan::black_box(record.decrypt(&KEY.into()).unwrap());
            }
        });
}

/// Build a chain of history records, serialized exactly as the history store would store them.
fn decrypted_records(n: usize) -> Vec<Record<DecryptedData>> {
    let mut ctx = BenchCtx::new();
    let host = Host::new(HostId(uuid_v7()));

    BenchHistory::count(&mut ctx, n)
        .into_iter()
        .enumerate()
        .map(|(idx, history)| {
            let data = HistoryRecord::Create(history).serialize().unwrap();

            Record::builder()
                .host(host.clone())
                .version(RecordVersion::from(Version::LATEST.name()))
                .tag(RecordTag::History)
                .data(data)
                .idx(idx as u64)
                .build()
        })
        .collect()
}
