//! Turns accumulated history into plaintext `packfile` manifest records.

use atuin_domain::caps::PackfileCap;
use atuin_domain::record::{Host, Record, RecordSeriesKey, RecordTag, RecordVersion};
use thiserror::Error;
use tracing::{instrument, trace};

use super::record::{PackManifestData, PackManifestDataV1, ParsingError};
use crate::record::sqlite_store::SqliteStore;

#[derive(Debug, Error)]
pub enum PackingError {
    #[error("error accessing the record store: {0}")]
    Store(eyre::Report),

    #[error("corrupt packfile manifest in the store: {0}")]
    ManifestLoad(#[from] ParsingError),

    #[error("failed to encode a packfile manifest: {0}")]
    ManifestStore(Box<dyn std::error::Error + Send + Sync>),
}

/// Write a `packfile` manifest record for each contiguous history run of `count` records.
#[instrument(level = "trace", skip(store), err)]
pub async fn try_pack(
    store: &SqliteStore,
    series: &RecordSeriesKey,
    cap: Option<PackfileCap>,
) -> Result<(), PackingError> {
    debug_assert!(series.tag != RecordTag::Packfile);

    // Count is the pack size. Packing waits until at least `count` unpacked records have
    // accumulated and then emits manifests of exactly that many records.
    let Some(count) = cap.map(|c| c.record_count).filter(|&n| n > 0) else {
        return Ok(());
    };

    let last_pack = store
        .last(&RecordSeriesKey::new(series.host, RecordTag::Packfile))
        .await
        .map_err(PackingError::Store)?;

    // `start` is the first unpacked source idx; `pack_idx` is the next idx in the *packfile*
    // stream (a separate sequence). Both come from the same latest manifest record.
    let start = match &last_pack {
        Some(record) => PackManifestData::parse(record)?.range().end,
        None => 0,
    };
    let mut pack_idx = last_pack.map_or(0, |record| record.idx + 1);

    let Some(ceiling) =
        store.last(series).await.map_err(PackingError::Store)?.map(|record| record.idx)
    else {
        trace!("no history yet; nothing to pack");
        return Ok(());
    };

    if ceiling < start {
        trace!(ceiling, start, "history already packed up to the floor");
        return Ok(());
    }

    let mut cursor = start;
    while cursor <= ceiling && ceiling - cursor + 1 >= count {
        // The loop guard guarantees at least `count` records remain, so each run is exactly `count`.
        let run = store.next(series, cursor, count).await.map_err(PackingError::Store)?;

        let Some(end) = run.last().map(|record| record.idx) else {
            break;
        };

        let manifest = PackManifestDataV1 {
            host: series.host,
            tag: series.tag.clone(),
            start_idx: cursor,
            end_idx: end,
        };
        let record = Record::builder()
            .host(Host::new(series.host))
            .version(RecordVersion::V1)
            .tag(RecordTag::Packfile)
            .idx(pack_idx)
            .data(manifest.encode().map_err(PackingError::ManifestStore)?)
            .build();

        store.push(&record).await.map_err(PackingError::Store)?;
        trace!(pack_idx, start = cursor, end, "wrote packfile manifest");

        pack_idx += 1;
        cursor = end + 1;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{EncryptedData, HostId};
    use proptest::prelude::*;
    use rstest::{fixture, rstest};

    use super::*;
    use crate::settings::test_local_timeout;

    #[fixture]
    async fn store() -> SqliteStore {
        SqliteStore::new(":memory:", test_local_timeout()).await.unwrap()
    }

    #[fixture]
    fn host() -> HostId {
        HostId(uuid_v7())
    }

    /// Push a single history record at `idx` (only its idx matters to the packer).
    async fn push_history(store: &SqliteStore, host: HostId, idx: u64) {
        let record = Record::builder()
            .host(Host::new(host))
            .version("v1".into())
            .tag(RecordTag::History)
            .idx(idx)
            .data(EncryptedData {
                raw: "d".into(),
                cek: "k".into(),
            })
            .build();
        store.push(&record).await.unwrap();
    }

    /// Seed a contiguous run of `count` history records, idx `0..count`.
    async fn seed_history(store: &SqliteStore, host: HostId, count: u64) {
        for idx in 0..count {
            push_history(store, host, idx).await;
        }
    }

    /// The `[start_idx, end_idx]` ranges of the manifest records currently in the store.
    async fn manifest_ranges(store: &SqliteStore, host: HostId) -> Vec<(u64, u64)> {
        store
            .next(&RecordSeriesKey::new(host, RecordTag::Packfile), 0, 1000)
            .await
            .unwrap()
            .iter()
            .map(|record| match PackManifestData::parse(record).unwrap() {
                PackManifestData::V1(v1) => (v1.start_idx, v1.end_idx),
            })
            .collect()
    }

    async fn pack(store: &SqliteStore, host: HostId, count: u64) {
        try_pack(
            store,
            &RecordSeriesKey::new(host, RecordTag::History),
            Some(PackfileCap {
                version: 1,
                record_count: count,
            }),
        )
        .await
        .unwrap();
    }

    /// One concrete example for readability; the proptests below prove the general invariants.
    #[rstest]
    #[tokio::test]
    async fn packs_multiple_and_leaves_remainder(#[future(awt)] store: SqliteStore, host: HostId) {
        seed_history(&store, host, 5).await; // idx 0..=4
        pack(&store, host, 2).await;
        // Runs of 2: (0,1) and (2,3); idx 4 alone is below the pack size, so it waits.
        assert_eq!(manifest_ranges(&store, host).await, vec![(0, 1), (2, 3)]);
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        /// For any history size and any pack size, a single `try_pack` produces manifests that
        /// tile `[0, packed)` exactly -- contiguous, no gaps or overlaps -- each holding exactly
        /// `pack_count` records, never reaching past the records that exist, and leaving fewer than
        /// `pack_count` records unpacked.
        #[test]
        fn packing_tiles_a_maximal_prefix(count in 0u64..=40, pack_count in 1u64..=8) {
            runtime().block_on(async move {
                let store = SqliteStore::new(":memory:", test_local_timeout()).await.unwrap();
                let host = HostId(uuid_v7());
                seed_history(&store, host, count).await;

                pack(&store, host, pack_count).await;
                let ranges = manifest_ranges(&store, host).await;

                let mut next = 0u64;
                for (start, end) in ranges {
                    assert_eq!(start, next, "manifests must tile contiguously from 0");
                    assert!(end >= start, "manifest range is inverted");
                    let size = end - start + 1;
                    assert_eq!(size, pack_count, "pack size {size} is not exactly {pack_count}");
                    next = end + 1;
                }

                assert!(next <= count, "packed {next} records beyond the {count} that exist");
                assert!(
                    count - next < pack_count,
                    "left {} unpacked, but pack size is {pack_count}",
                    count - next
                );
            });
        }

        /// Packing is idempotent and monotonic: a second call with no new history changes nothing,
        /// and packing again after more history arrives only extends coverage -- it never rewrites
        /// or drops an existing manifest.
        #[test]
        fn packing_is_idempotent_and_monotonic(
            first in 0u64..=25,
            extra in 0u64..=25,
            pack_count in 1u64..=6,
        ) {
            runtime().block_on(async move {
                let store = SqliteStore::new(":memory:", test_local_timeout()).await.unwrap();
                let host = HostId(uuid_v7());

                seed_history(&store, host, first).await;
                pack(&store, host, pack_count).await;
                let after_first = manifest_ranges(&store, host).await;

                // A second pass with nothing new must be a no-op.
                pack(&store, host, pack_count).await;
                assert_eq!(
                    manifest_ranges(&store, host).await,
                    after_first,
                    "re-pack was not a no-op"
                );

                // More history arrives; existing manifests must survive verbatim as a prefix.
                for idx in first..first + extra {
                    push_history(&store, host, idx).await;
                }
                pack(&store, host, pack_count).await;
                let after_more = manifest_ranges(&store, host).await;
                assert!(
                    after_more.starts_with(&after_first),
                    "existing manifests were rewritten"
                );

                let total = first + extra;
                let packed = after_more.last().map_or(0, |&(_, end)| end + 1);
                assert!(packed <= total);
                assert!(
                    total - packed < pack_count,
                    "left {} unpacked, but pack size is {pack_count}",
                    total - packed
                );
            });
        }
    }
}
