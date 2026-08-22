//! The core sync engine that Atuin uses.
//!
//! The role of sync is to sync records between a remote server and a local client. There are two
//! core terms important to note:
//!
//! - Packfiles -- A packfile is a "bundle" of [`Record`]s.
//! - Loose pages -- Servers offer pagination of [`Record`]s and [`Record`]s. In effect, the server
//!   has an RPC call to query for N records. This query returns a page of "loose" records, where
//!   loose refers to the fact that these records are not packed into packfiles.
//!
//! TODO(markovejnovic): Migrate this outside of `record/`, since it handles a lot more than just
//!                      records.
//!
//! > do a sync :O
use std::cmp::Ordering;
use std::fmt::Write;
use std::num::NonZeroU64;

use atuin_common::encryption::paseto_v4;
use atuin_common::range::{Chunks, RangeExt};
use atuin_common::sync::MutEagerFutureCell;
use atuin_domain::caps::PackfileCap;
use atuin_domain::record::{
    Diff, EncryptedData, Record, RecordId, RecordIdx, RecordSeriesKey, RecordStatus, RecordTag,
};
use eyre::Result;
use futures::{StreamExt, TryStreamExt, stream};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use thiserror::Error;
use tokio::runtime::Handle;
use tracing::instrument;

use super::sqlite_store::SqliteStore;
use crate::api_client::Client;
use crate::packfile::PackedPackfile;
use crate::packfile::record::{PackManifestRecordView, ParsingError, UnpackError};

mod builder;
pub use builder::{ClientSource, SyncEngineBuilder, SyncEngineInit};

/// How many packfile blobs to download concurrently within a single page. (Uploads are batched by
/// [`Client::upload_packfiles`](crate::api_client::Client::upload_packfiles).)
const MAX_CONCURRENT_PACKFILE_TRANSFERS: usize = 16;

/// How many packfile manifests to pack concurrently before handing a page's blobs to the client.
const MAX_CONCURRENT_PACKS: usize = 16;

/// Records requested per sync page unless overridden with [`SyncEngine::with_page_size`].
pub const DEFAULT_PAGE_SIZE: NonZeroU64 = NonZeroU64::new(100).unwrap();

#[derive(Error, Debug, Clone)]
pub enum SyncError {
    #[error("the local store is ahead of the remote, but for another host. has remote lost data?")]
    LocalAheadOtherHost,

    #[error("an issue with the local database occurred: {msg:?}")]
    LocalStoreError {
        msg: String,
    },

    #[error("something has gone wrong with the sync logic: {msg:?}")]
    SyncLogicError {
        msg: String,
    },

    #[error("operational error: {msg:?}")]
    OperationalError {
        msg: String,
    },

    #[error("a request to the sync server failed: {msg:?}")]
    RemoteRequestError {
        msg: String,
    },

    #[error(
        "the encryption key on this machine does not match the data on the server. this usually \
         means a new machine was set up without copying the existing key. to fix: run `atuin key` \
         on a machine that already syncs correctly, then run `atuin store rekey <key>` on this \
         machine with the value from the other machine"
    )]
    WrongKey,
}

#[derive(Debug, Error)]
pub(crate) enum PackfileDownloadError {
    #[error("failed to load the packfile manifest: {0}")]
    PackManifest(#[from] ParsingError),

    #[error("packfile download failed: {0}")]
    Api(eyre::Report),

    #[error(transparent)]
    Unpack(#[from] UnpackError),

    #[error("failed to store the unpacked history: {0}")]
    Store(eyre::Report),
}

impl PackfileDownloadError {
    /// Whether repeating the same download operation would definitively fail.
    fn is_permanent(&self) -> bool {
        match self {
            Self::PackManifest(_) => true,
            Self::Unpack(_) => true,
            Self::Api(_) | Self::Store(_) => false,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum Operation {
    // Either upload or download until the states matches the below
    Upload {
        local: RecordIdx,
        remote: Option<RecordIdx>,
        series: RecordSeriesKey,
    },
    Download {
        remote: RecordIdx,
        series: RecordSeriesKey,
    },
    Noop {
        series: RecordSeriesKey,
    },
}

/// Drives atuin's sync.
#[derive(Clone)]
pub struct SyncEngine {
    client: Client,
    store: SqliteStore,
    /// How many records each sync page requests. Set via [`Self::with_page_size`].
    page_size: NonZeroU64,
}

/// A [`SyncEngine`] paired with an encryption key, for the operations that encrypt or decrypt.
/// Obtained from [`SyncEngine::keyed`].
pub struct Keyed<'k> {
    engine: &'k SyncEngine,
    key: &'k paseto_v4::Key,
    /// The result of verifying `key` against the remote.
    key_check: MutEagerFutureCell<Option<SyncError>>,
}

impl SyncEngine {
    /// Set how many records each sync page requests (default [`DEFAULT_PAGE_SIZE`]).
    #[must_use]
    pub fn with_page_size(mut self, page_size: NonZeroU64) -> Self {
        self.page_size = page_size;
        self
    }

    /// Pair this engine with an encryption `key` to run the crypto-touching sync operations.
    pub fn keyed<'k>(&'k self, key: &'k paseto_v4::Key) -> Keyed<'k> {
        let engine = self.clone();
        let key_for_check = key.clone();
        let key_check = MutEagerFutureCell::new(
            async move { engine.check_encryption_key(&key_for_check).await },
            &Handle::current(),
        );

        Keyed {
            engine: self,
            key,
            key_check,
        }
    }

    /// Verify that `key` can decrypt the remote's data. [`Option::None`] when the key is good.
    async fn check_encryption_key(&self, key: &paseto_v4::Key) -> Option<SyncError> {
        let remote_index = match self.record_status().await {
            Ok(idx) => idx,
            Err(e) => return Some(e),
        };

        self.check_key_against_index(key, &remote_index).await
    }

    /// As [`Self::check_encryption_key`], but against an already-fetched `remote_index`.
    async fn check_key_against_index(
        &self,
        key: &paseto_v4::Key,
        remote_index: &RecordStatus,
    ) -> Option<SyncError> {
        let sample = remote_index
            .hosts
            .iter()
            .flat_map(|(host, tags)| {
                tags.keys().map(move |tag| RecordSeriesKey::new(*host, tag.clone()))
            })
            // Note we have to skip `Packfile`s here because packfiles _aren't_ actually encrypted,
            // so using the default CEK would fail decryption.
            .find(|series| series.tag != RecordTag::Packfile);

        let series = sample?;

        let record = match self.client.records(&series).one().await {
            Ok(Some(record)) => record,
            Ok(None) => return None,
            Err(e) => return Some(SyncError::RemoteRequestError { msg: e.to_string() }),
        };

        record.decrypt(key).err().map(|_| SyncError::WrongKey)
    }

    /// Fetch the remote's record status index.
    #[instrument(level = "trace", skip_all, err)]
    pub async fn record_status(&self) -> Result<RecordStatus, SyncError> {
        self.client
            .record_status()
            .await
            .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn diff(&self) -> Result<(Vec<Diff>, RecordStatus), SyncError> {
        let local_index = self
            .store
            .status()
            .await
            .map_err(|e| SyncError::LocalStoreError { msg: e.to_string() })?;

        let remote_index = self
            .client
            .record_status()
            .await
            .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?;

        let diff = local_index.diff(&remote_index);

        Ok((diff, remote_index))
    }

    // Take a diff and resolve it into a set of operations. In theory this could be done as a part of
    // the diffing stage, but it's easier to reason about and test this way. It needs none of the
    // engine's state, so it's an associated function rather than a method.
    #[instrument(level = "trace", skip_all, fields(n_diffs = diffs.len()), err)]
    pub fn operations(diffs: Vec<Diff>) -> Result<Vec<Operation>, SyncError> {
        let mut operations = diffs
            .into_iter()
            .map(|diff| match (diff.local, diff.remote) {
                // We both have it! Could be either. Compare.
                (Some(local), Some(remote)) => Ok(match local.cmp(&remote) {
                    Ordering::Equal => Operation::Noop {
                        series: diff.series,
                    },
                    Ordering::Greater => Operation::Upload {
                        local,
                        remote: Some(remote),
                        series: diff.series,
                    },
                    Ordering::Less => Operation::Download {
                        remote,
                        series: diff.series,
                    },
                }),

                // Remote has it, we don't. Gotta be download
                (None, Some(remote)) => Ok(Operation::Download {
                    remote,
                    series: diff.series,
                }),

                // We have it, remote doesn't. Gotta be upload.
                (Some(local), None) => Ok(Operation::Upload {
                    local,
                    remote: None,
                    series: diff.series,
                }),

                // something is pretty fucked.
                (None, None) => Err(SyncError::SyncLogicError {
                    msg: String::from(
                        "diff has nothing for local or remote - (host, tag) does not exist",
                    ),
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;

        // sort them - purely so we have a stable testing order, and can rely on
        // same input = same output
        // We can sort by ID so long as we continue to use UUIDv7 or something
        // with the same properties
        operations.sort_by_key(|op| match op {
            Operation::Noop { series } => (0u8, series.host_id, 0u8, series.tag.clone()),
            Operation::Upload { series, .. } => (1u8, series.host_id, 0u8, series.tag.clone()),
            Operation::Download { series, .. } => {
                // Packfile manifests must expand before the history download runs, as that
                // `sync_download` will dedupe will have a chance at avoiding unnecessary downloads.
                let tag_priority = if series.tag == RecordTag::Packfile {
                    0u8
                } else {
                    1u8
                };
                (2u8, series.host_id, tag_priority, series.tag.clone())
            }
        });

        Ok(operations)
    }
}

impl Keyed<'_> {
    // TODO(markovejnovic): Seriously revisit the syncing logic and coupling.
    #[instrument(
        level = "trace",
        skip_all,
        fields(host = ?series.host_id, tag = ?series.tag, local, remote = ?remote, page_size = self.engine.page_size.get()),
        err
    )]
    async fn sync_upload(
        &self,
        series: &RecordSeriesKey,
        local: RecordIdx,
        remote: Option<RecordIdx>,
    ) -> Result<u64, SyncError> {
        let page_size = self.engine.page_size.get();
        let store = &self.engine.store;
        let client = &self.engine.client;
        // The first record the remote *doesn't* have.
        let first_missing_remote = remote.map_or(0, |n| n + 1);
        let expected = local + 1 - first_missing_remote;
        let mut progress = 0;

        let pb = ProgressBar::new(expected);
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] \
                 {human_pos}/{human_len} ({eta})",
            )
            .unwrap()
            .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
                write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
            })
            .progress_chars("#>-"),
        );

        println!(
            "Uploading {} records to {}/{}",
            expected,
            series.host_id.0.as_simple(),
            series.tag
        );

        while progress < expected {
            let page = store
                .next(series, first_missing_remote + progress, page_size)
                .await
                .map_err(|e| {
                    error!("failed to read upload page: {e:?}");

                    SyncError::LocalStoreError { msg: e.to_string() }
                })?;

            if page.is_empty() {
                break;
            }

            if series.tag == RecordTag::Packfile {
                let key = self.key.clone();
                let store = store.clone();
                let packed: Vec<_> = stream::iter(page.iter().cloned())
                    .map(|manifest| {
                        let (store, key) = (store.clone(), key.clone());
                        async move {
                            let view = PackManifestRecordView::new(&manifest)?;
                            let (blob, ids) = view.pack_records(&store, key).await?;
                            Ok::<_, eyre::Report>(PackedPackfile {
                                manifest_id: view.record.id,
                                records: ids,
                                blob,
                            })
                        }
                    })
                    .buffered(MAX_CONCURRENT_PACKS)
                    .try_collect()
                    .await
                    .map_err(|e| {
                        error!("failed to pack packfile: {e}");
                        SyncError::RemoteRequestError { msg: e.to_string() }
                    })?;
                client
                    .upload_packfiles(stream::iter(packed.into_iter().map(Ok::<_, eyre::Report>)))
                    .await
                    .map_err(|e| {
                        error!("failed to upload packfile: {e}");
                        SyncError::RemoteRequestError { msg: e.to_string() }
                    })?;
            }

            client.post_records(&page).await.map_err(|e| {
                error!("failed to post records: {e:?}");

                SyncError::RemoteRequestError { msg: e.to_string() }
            })?;

            progress += page.len() as u64;
            pb.set_position(progress);
        }

        pb.finish_with_message("Uploaded records");

        Ok(progress)
    }

    // TODO(markovejnovic): Seriously revisit the syncing logic and coupling.
    #[instrument(
        level = "trace",
        skip_all,
        fields(host = ?series.host_id, tag = ?series.tag, remote = ?remote, page_size = self.engine.page_size.get()),
        err
    )]
    async fn sync_download(
        &self,
        series: &RecordSeriesKey,
        remote: RecordIdx,
    ) -> Result<Vec<RecordId>, SyncError> {
        let page_size = self.engine.page_size.get();
        let store = &self.engine.store;
        // Scan the database to find the first missing local index, rather than assuming it's one
        // more than the highest local index. A prior packfile op for this host may have expanded a
        // pack whose history landed ABOVE a still-missing index; keying off the highest index would
        // never fetch the hole before it. Start from the actual missing index; records already
        // present above it will be "unnecessarily" redownloaded, but this is a no-op.
        let first_missing_local = store
            .first_gap(series)
            .await
            .map_err(|e| SyncError::LocalStoreError { msg: e.to_string() })?;

        // One higher than the latest record index we have locally. The case described above where
        // we have a hole in the sequence of record indices should not happen in practice; this
        // variable is used to detect that situation so we can print a warning.
        //
        // TODO: This adds a slight runtime cost, but while the packfile feature is new, let's err
        // on the side of catching potential problems.
        let latest = store
            .last(series)
            .await
            .map_err(|e| SyncError::LocalStoreError { msg: e.to_string() })?
            .map(|record| record.idx);

        if first_missing_local != latest.map_or(0, |n| n + 1) {
            tracing::warn!(
                "first missing record index is {first_missing_local}, but latest record is {}",
                std::fmt::from_fn(|f| {
                    match latest {
                        Some(n) => write!(f, "{n}"),
                        None => write!(f, "(none)"),
                    }
                }),
            );
        }

        let expected = (remote + 1).saturating_sub(first_missing_local);

        println!(
            "Downloading {} records from {}/{}",
            expected,
            series.host_id.0.as_simple(),
            series.tag
        );

        let pb = ProgressBar::new(expected);
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] \
                 {human_pos}/{human_len} ({eta})",
            )
            .unwrap()
            .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
                write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
            })
            .progress_chars("#>-"),
        );

        let chunks = (first_missing_local..first_missing_local + expected).chunks(page_size);
        let ret = self.download_pages(series, chunks, &pb).await?;

        pb.finish_with_message("Downloaded records");

        Ok(ret)
    }

    #[instrument(level = "trace", skip_all, fields(id = ?manifest.id), err)]
    async fn download_packed(
        &self,
        manifest: &Record<EncryptedData>,
    ) -> Result<Vec<RecordId>, PackfileDownloadError> {
        let view = PackManifestRecordView::new(manifest)?;
        let store = &self.engine.store;

        // Skip if we already have the whole range (history is contiguous, packfiles are prefixes).
        let head = store
            .last(&RecordSeriesKey::new(view.record.host.id, RecordTag::History))
            .await
            .map_err(PackfileDownloadError::Store)?;
        if let Some(head) = head
            && head.idx >= view.range().end - 1
        {
            // Range already available locally. Return the IDs.
            let existing = view
                .load_encrypted_packed_records(store)
                .await
                .map_err(|e| PackfileDownloadError::Store(e.into()))?;
            return Ok(existing.iter().map(|r| r.id).collect());
        }

        let blob = self
            .engine
            .client
            .download_packfile(view.record.id)
            .await
            .map_err(PackfileDownloadError::Api)?;

        let records = view.unpack_records(blob, self.key.clone()).await?;
        let ids: Vec<RecordId> = records.iter().map(|record| record.id).collect();

        store.push_batch(records.iter()).await.map_err(PackfileDownloadError::Store)?;

        Ok(ids)
    }

    /// Download the record pages the `chunks` cover into the local store, gracefully handling any
    /// packfiles along the way.
    #[instrument(level = "trace", skip_all, err)]
    async fn download_pages(
        &self,
        series: &RecordSeriesKey,
        chunks: Chunks<RecordIdx>,
        pb: &ProgressBar,
    ) -> Result<Vec<RecordId>, SyncError> {
        let mut ret = Vec::new();
        let mut progress = 0u64;

        let pages = self.engine.client.records(series).stream(chunks);
        futures::pin_mut!(pages);
        while let Some(page) = pages.next().await {
            let page = page.map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?;

            // A packfile manifest's history must land in the store *before* the manifest itself, so
            // a stored manifest always has its records. Loose pages have no such step.
            if series.tag == RecordTag::Packfile {
                ret.extend(self.expand_manifests(&page).await?);
            }

            self.engine
                .store
                .push_batch(page.iter())
                .await
                .map_err(|e| SyncError::LocalStoreError { msg: e.to_string() })?;

            ret.extend(page.iter().map(|f| f.id));

            progress += page.len() as u64;
            pb.set_position(progress);
        }

        Ok(ret)
    }

    /// Expand every packfile manifest in `page`, committing the history it covers into the local
    /// store. A manifest that can never expand is logged and skipped.
    #[instrument(level = "trace", skip_all, fields(count = page.len()), err)]
    async fn expand_manifests(
        &self,
        page: &[Record<EncryptedData>],
    ) -> Result<Vec<RecordId>, SyncError> {
        let mut ids = Vec::new();

        let mut downloads = stream::iter(0..page.len())
            .map(|i| self.download_packed(&page[i]))
            .buffered(MAX_CONCURRENT_PACKFILE_TRANSFERS)
            .enumerate();

        while let Some((i, result)) = downloads.next().await {
            match result {
                Ok(expanded) => ids.extend(expanded),
                Err(e) if e.is_permanent() => error!(
                    manifest_id = %page[i].id,
                    host = %page[i].host.id,
                    idx = page[i].idx,
                    "skipping unexpandable packfile manifest: {e}. you have lost data."
                ),
                Err(e) => {
                    error!("failed to download packfile: {e:?}");
                    return Err(SyncError::RemoteRequestError { msg: e.to_string() });
                }
            }
        }

        Ok(ids)
    }

    #[instrument(level = "trace", skip_all, fields(page_size = self.engine.page_size.get()), err)]
    pub async fn sync_remote(
        &self,
        operations: Vec<Operation>,
    ) -> Result<(u64, Vec<RecordId>), SyncError> {
        let mut uploaded = 0;
        let mut downloaded = Vec::new();

        let packfiles_enabled = matches!(
            self.engine.client.caps().get_server::<PackfileCap>().await,
            Ok(Some(cap)) if cap.record_count > 0
        );

        // this can totally run in parallel, but lets get it working first
        for i in operations {
            match i {
                Operation::Upload {
                    series,
                    local,
                    remote,
                } => {
                    if series.tag == RecordTag::Packfile && !packfiles_enabled {
                        debug!(
                            "server does not advertise PackfileCap; skipping packfile {} upload \
                             op, loose history covers it",
                            series.tag
                        );
                        continue;
                    }
                    uploaded += self.sync_upload(&series, local, remote).await?
                }

                Operation::Download { series, remote } => {
                    if series.tag == RecordTag::Packfile && !packfiles_enabled {
                        debug!(
                            "server does not advertise PackfileCap; skipping packfile {} download \
                             op, loose history covers it",
                            series.tag
                        );
                        continue;
                    }
                    let mut d = self.sync_download(&series, remote).await?;
                    downloaded.append(&mut d)
                }

                Operation::Noop { .. } => continue,
            }
        }

        Ok((uploaded, downloaded))
    }

    /// Check whether the key can decrypt the synced data.
    pub async fn key_valid(&self) -> Option<SyncError> {
        self.key_check.get().await
    }

    /// Verify the key against an already-fetched `remote_index`.
    ///
    /// Subsequent calls to [`Self::key_valid`] will return whether the key is valid against this
    /// new `remote_index`.
    pub async fn key_valid_against(&self, remote_index: &RecordStatus) -> Option<SyncError> {
        let verdict = self.engine.check_key_against_index(self.key, remote_index).await;
        self.key_check.overwrite(verdict.clone());
        verdict
    }

    /// Run a full sync: diff local against remote, verify the key can read the remote, resolve the
    /// diff into operations, then apply them.
    #[instrument(level = "trace", skip_all, err)]
    pub async fn sync(&self) -> Result<(u64, Vec<RecordId>), SyncError> {
        let (diff, remote_index) = self.engine.diff().await?;

        if let Some(err) = self.key_valid_against(&remote_index).await {
            return Err(err);
        }

        let operations = SyncEngine::operations(diff)?;
        self.sync_remote(operations).await
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{
        Diff, EncryptedData, HostId, Record, RecordIdx, RecordSeriesKey, RecordTag,
    };
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use crate::record::sqlite_store::SqliteStore;
    use crate::record::sync::{Operation, SyncEngine};
    use crate::settings::test_local_timeout;

    enum Expect {
        Upload {
            local: RecordIdx,
            remote: Option<RecordIdx>,
        },
        Download {
            remote: RecordIdx,
        },
        Noop,
        Err,
    }

    fn test_record() -> Record<EncryptedData> {
        Record::builder()
            .host(atuin_domain::record::Host::new(HostId(atuin_common::utils::uuid_v7())))
            .version("v1".into())
            .tag(RecordTag::Other(atuin_common::utils::uuid_v7().simple().to_string()))
            .data(EncryptedData {
                raw: String::new(),
                cek: String::new(),
            })
            .idx(0)
            .build()
    }

    // Take a list of local records, and a list of remote records.
    // Return the local database, and a diff of local/remote, ready to build
    // ops
    async fn build_test_diff(
        local_records: Vec<Record<EncryptedData>>,
        remote_records: Vec<Record<EncryptedData>>,
    ) -> (SqliteStore, Vec<Diff>) {
        let local_store = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .expect("failed to open in memory sqlite");
        let remote_store = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .expect("failed to open in memory sqlite"); // "remote"

        for i in local_records {
            local_store.push(&i).await.unwrap();
        }

        for i in remote_records {
            remote_store.push(&i).await.unwrap();
        }

        let local_index = local_store.status().await.unwrap();
        let remote_index = remote_store.status().await.unwrap();

        let diff = local_index.diff(&remote_index);

        (local_store, diff)
    }

    #[rstest]
    #[case::local_only(Some(0), None, Expect::Upload { local: 0, remote: None })]
    #[case::local_ahead(Some(2), Some(0), Expect::Upload { local: 2, remote: Some(0) })]
    #[case::remote_only(None, Some(3), Expect::Download { remote: 3 })]
    #[case::remote_ahead(Some(0), Some(2), Expect::Download { remote: 2 })]
    #[case::equal(Some(1), Some(1), Expect::Noop)]
    #[case::neither(None, None, Expect::Err)]
    fn operations_resolves_each_diff(
        #[case] local: Option<RecordIdx>,
        #[case] remote: Option<RecordIdx>,
        #[case] expect: Expect,
    ) {
        let series = RecordSeriesKey::new(HostId(uuid_v7()), RecordTag::History);
        let result = SyncEngine::operations(vec![Diff {
            series: series.clone(),
            local,
            remote,
        }]);

        let expected = match expect {
            Expect::Err => {
                assert!(result.is_err());
                return;
            }
            Expect::Upload { local, remote } => Operation::Upload {
                local,
                remote,
                series,
            },
            Expect::Download { remote } => Operation::Download { remote, series },
            Expect::Noop => Operation::Noop { series },
        };
        assert_eq!(result.unwrap(), vec![expected]);
    }

    #[rstest]
    #[tokio::test]
    async fn build_complex_diff() {
        // One shared, ahead but known only by remote
        // One known only by local
        // One known only by remote

        let shared_record = test_record();
        let local_only = test_record();

        let local_only_20 = test_record();
        let local_only_21 = local_only_20.append(vec![1, 2, 3]).encrypt(&[0; 32].into());
        let local_only_22 = local_only_21.append(vec![1, 2, 3]).encrypt(&[0; 32].into());
        let local_only_23 = local_only_22.append(vec![1, 2, 3]).encrypt(&[0; 32].into());

        let remote_only = test_record();

        let remote_only_20 = test_record();
        let remote_only_21 = remote_only_20.append(vec![2, 3, 2]).encrypt(&[0; 32].into());
        let remote_only_22 = remote_only_21.append(vec![2, 3, 2]).encrypt(&[0; 32].into());
        let remote_only_23 = remote_only_22.append(vec![2, 3, 2]).encrypt(&[0; 32].into());
        let remote_only_24 = remote_only_23.append(vec![2, 3, 2]).encrypt(&[0; 32].into());

        let second_shared = test_record();
        let second_shared_remote_ahead =
            second_shared.append(vec![1, 2, 3]).encrypt(&[0; 32].into());
        let second_shared_remote_ahead2 =
            second_shared_remote_ahead.append(vec![1, 2, 3]).encrypt(&[0; 32].into());

        let third_shared = test_record();
        let third_shared_local_ahead = third_shared.append(vec![1, 2, 3]).encrypt(&[0; 32].into());
        let third_shared_local_ahead2 =
            third_shared_local_ahead.append(vec![1, 2, 3]).encrypt(&[0; 32].into());

        let fourth_shared = test_record();
        let fourth_shared_remote_ahead =
            fourth_shared.append(vec![1, 2, 3]).encrypt(&[0; 32].into());
        let fourth_shared_remote_ahead2 =
            fourth_shared_remote_ahead.append(vec![1, 2, 3]).encrypt(&[0; 32].into());

        let local = vec![
            shared_record.clone(),
            second_shared.clone(),
            third_shared.clone(),
            fourth_shared.clone(),
            fourth_shared_remote_ahead.clone(),
            // single store, only local has it
            local_only.clone(),
            // bigger store, also only known by local
            local_only_20.clone(),
            local_only_21.clone(),
            local_only_22.clone(),
            local_only_23.clone(),
            // another shared store, but local is ahead on this one
            third_shared_local_ahead.clone(),
            third_shared_local_ahead2.clone(),
        ];

        let remote = vec![
            remote_only.clone(),
            remote_only_20.clone(),
            remote_only_21.clone(),
            remote_only_22.clone(),
            remote_only_23.clone(),
            remote_only_24.clone(),
            shared_record.clone(),
            second_shared.clone(),
            third_shared.clone(),
            second_shared_remote_ahead.clone(),
            second_shared_remote_ahead2.clone(),
            fourth_shared.clone(),
            fourth_shared_remote_ahead.clone(),
            fourth_shared_remote_ahead2.clone(),
        ]; // remote knows about the already-synced, and one new record in a new store

        let (_store, diff) = build_test_diff(local, remote).await;
        let operations = SyncEngine::operations(diff).unwrap();

        assert_eq!(operations.len(), 7);

        let mut result_ops = vec![
            // We started with a shared record, but the remote knows of two newer records in the
            // same store
            Operation::Download {
                remote: 2,
                series: RecordSeriesKey::new(
                    second_shared_remote_ahead.host.id,
                    second_shared_remote_ahead.tag.clone(),
                ),
            },
            // We have a shared record, local knows of the first two but not the last
            Operation::Download {
                remote: 2,
                series: RecordSeriesKey::new(
                    fourth_shared_remote_ahead2.host.id,
                    fourth_shared_remote_ahead2.tag.clone(),
                ),
            },
            // Remote knows of a store with a single record that local does not have
            Operation::Download {
                remote: 0,
                series: RecordSeriesKey::new(remote_only.host.id, remote_only.tag.clone()),
            },
            // Remote knows of a store with a bunch of records that local does not have
            Operation::Download {
                remote: 4,
                series: RecordSeriesKey::new(remote_only_20.host.id, remote_only_20.tag.clone()),
            },
            // Local knows of a record in a store that remote does not have
            Operation::Upload {
                local: 0,
                remote: None,
                series: RecordSeriesKey::new(local_only.host.id, local_only.tag.clone()),
            },
            // Local knows of 4 records in a store that remote does not have
            Operation::Upload {
                local: 3,
                remote: None,
                series: RecordSeriesKey::new(local_only_20.host.id, local_only_20.tag.clone()),
            },
            // Local knows of 2 more records in a shared store that remote only has one of
            Operation::Upload {
                local: 2,
                remote: Some(0),
                series: RecordSeriesKey::new(third_shared.host.id, third_shared.tag.clone()),
            },
        ];

        result_ops.sort_by_key(|op| match op {
            Operation::Noop { series } => (0, series.host_id, series.tag.clone()),

            Operation::Upload { series, .. } => (1, series.host_id, series.tag.clone()),

            Operation::Download { series, .. } => (2, series.host_id, series.tag.clone()),
        });

        assert_eq!(result_ops, operations);
    }
}

#[cfg(test)]
mod packfile_sync_tests {
    use std::collections::HashMap;

    use atuin_common::encryption::paseto_v4;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{
        DecryptedData, EncryptedData, Host, HostId, Record, RecordId, RecordVersion,
    };
    use rstest::*;
    use wiremock::matchers::{method, path, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::api_client::{AuthToken, Client, caps_client};
    use crate::packfile::record::PackManifestDataV1;
    use crate::packfile::{PackManifestRecordView, try_pack};
    use crate::record::sqlite_store::SqliteStore;
    use crate::settings::test_local_timeout;

    /// A single fixed encryption key. The specific bytes are arbitrary in these tests -- each one
    /// packs and unpacks with the same key -- so one shared value keeps setup uniform.
    #[fixture]
    pub(super) fn key() -> paseto_v4::Key {
        paseto_v4::Key::from([7u8; 32])
    }

    #[fixture]
    pub(super) async fn server() -> MockServer {
        MockServer::start().await
    }

    /// A [`Client`] pointed at a wiremock server, authenticated with a dummy token.
    pub(super) fn mock_client(addr: &url::Url) -> Client {
        let caps = caps_client(addr, &HashMap::new()).unwrap();
        Client::new(addr.clone(), &AuthToken::Token("t".into()), 30, 30, &HashMap::new(), caps)
            .unwrap()
    }

    /// A fresh in-memory record store.
    pub(super) async fn memory_store() -> SqliteStore {
        SqliteStore::new(":memory:", test_local_timeout()).await.unwrap()
    }

    /// Wrap a prebuilt client in a [`SyncEngine`] for tests.
    pub(super) async fn build_engine(client: Client, store: SqliteStore) -> SyncEngine {
        SyncEngine::builder()
            .store(store)
            .client_source(ClientSource::FromClient(client))
            .build()
            .connect()
            .await
            .unwrap()
    }

    /// Push a contiguous run of `count` encrypted HISTORY records (idx `0..count`).
    pub(super) async fn seed_history(
        store: &SqliteStore,
        host: HostId,
        key: &paseto_v4::Key,
        count: u64,
    ) {
        for idx in 0..count {
            let record = Record::builder()
                .host(Host::new(host))
                .version("v1".into())
                .tag(RecordTag::History)
                .idx(idx)
                .data(DecryptedData(format!("cmd {idx}").into_bytes()))
                .build()
                .encrypt(key);
            store.push(&record).await.unwrap();
        }
    }

    /// Uploader-side artifacts for a packed run of `count` history records: the manifest record and
    /// the packed blob a server would serve.
    pub(super) async fn packed_packfile(
        host: HostId,
        key: &paseto_v4::Key,
        count: u64,
    ) -> (Record<EncryptedData>, Vec<u8>) {
        let (manifest, blob, _ids) = packed_packfile_with_ids(host, key, count).await;
        (manifest, blob)
    }

    /// As [`packed_packfile`], but also returns the ids of the history records the packfile covers (in
    /// idx order) for the re-indexing assertion.
    async fn packed_packfile_with_ids(
        host: HostId,
        key: &paseto_v4::Key,
        count: u64,
    ) -> (Record<EncryptedData>, Vec<u8>, Vec<RecordId>) {
        let up = memory_store().await;
        seed_history(&up, host, key, count).await;
        try_pack(
            &up,
            &RecordSeriesKey::new(host, RecordTag::History),
            Some(PackfileCap {
                version: 1,
                record_count: count,
            }),
        )
        .await
        .unwrap();
        let manifest =
            up.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().unwrap();
        let view = PackManifestRecordView::new(&manifest).unwrap();
        let (blob, ids) = view.pack_records(&up, key.clone()).await.unwrap();
        (manifest, blob, ids)
    }

    /// Mount the common packfile-download mock set: the manifest page (start=0), an empty follow-up
    /// page that ends the loop, the packfile's download URL, and the blob bytes.
    pub(super) async fn mount_packfile(
        server: &MockServer,
        manifest: &Record<EncryptedData>,
        blob: Vec<u8>,
    ) {
        // First page: the manifest. Second page (start advanced): empty -> loop ends.
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .and(query_param("start", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![manifest.clone()]))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(Vec::<Record<EncryptedData>>::new()),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v0/packfiles/{}", manifest.id.0)))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "download_url": format!("{}/download/abc", server.uri()) })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/abc"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(blob))
            .mount(server)
            .await;
    }

    /// REGRESSION: packfile manifests are stored plaintext (no `cek`), so `check_encryption_key`
    /// must never sample one and mis-report `WrongKey`. Before the fix it sampled the first
    /// `(host, tag)` from a HashMap; a `packfile` sample "failed" to decrypt the manifest and
    /// logged the user out on login / aborted sync ~half the time. A remote index whose only tag
    /// is the manifest makes the old sampler deterministically pick it.
    #[rstest]
    #[tokio::test]
    async fn check_encryption_key_ignores_plaintext_packfile_manifests(
        key: paseto_v4::Key,
        #[future] server: MockServer,
    ) {
        let host = HostId(uuid_v7());
        let (manifest, _blob) = packed_packfile(host, &key, 5).await;

        let mut tags = HashMap::new();
        tags.insert(RecordTag::Packfile, manifest.idx);
        let mut hosts = HashMap::new();
        hosts.insert(host, tags);
        let remote_index = RecordStatus { hosts };

        // Serve the manifest if the sampler (wrongly) tries to fetch it.
        let server = server.await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![manifest.clone()]))
            .mount(&server)
            .await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);
        let engine = build_engine(client, memory_store().await).await;

        assert!(
            engine.check_key_against_index(&key, &remote_index).await.is_none(),
            "a plaintext packfile manifest must not be treated as a wrong key"
        );
    }

    /// GUARD: excluding the packfile tag must not weaken real detection -- a genuinely wrong key
    /// against an encrypted history record must still surface as `WrongKey`.
    #[rstest]
    #[tokio::test]
    async fn check_encryption_key_still_detects_a_wrong_key_on_history(
        key: paseto_v4::Key,
        #[future] server: MockServer,
    ) {
        let host = HostId(uuid_v7());
        let store = memory_store().await;
        seed_history(&store, host, &key, 1).await;
        let rec = store.next(&RecordSeriesKey::new(host, RecordTag::History), 0, 1).await.unwrap()
            [0]
        .clone();

        let mut tags = HashMap::new();
        tags.insert(RecordTag::History, rec.idx);
        let mut hosts = HashMap::new();
        hosts.insert(host, tags);
        let remote_index = RecordStatus { hosts };

        let server = server.await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![rec]))
            .mount(&server)
            .await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let engine = build_engine(client, store).await;
        let wrong = paseto_v4::Key::from([9u8; 32]);
        let err = engine.check_key_against_index(&wrong, &remote_index).await;
        assert!(matches!(err, Some(SyncError::WrongKey)), "expected WrongKey, got {err:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn history_download_skips_the_range_a_packfile_covered(
        key: paseto_v4::Key,
        #[future] server: MockServer,
    ) {
        let host = HostId(uuid_v7());

        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = server.await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .and(query_param("tag", RecordTag::Packfile.as_str()))
            .and(query_param("start", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![manifest.clone()]))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .and(query_param("tag", RecordTag::Packfile.as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(Vec::<Record<EncryptedData>>::new()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v0/packfiles/{}", manifest.id.0)))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "download_url": format!("{}/download/abc", server.uri()) })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/abc"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(blob))
            .mount(&server)
            .await;
        // Loose history endpoint: record any hit so we can assert the covered range is skipped.
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .and(query_param("tag", RecordTag::History.as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(Vec::<Record<EncryptedData>>::new()),
            )
            .mount(&server)
            .await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);
        let engine = build_engine(client, down).await;

        // Packfile op first (populates history 0..=2), then the history op.
        engine
            .keyed(&key)
            .sync_download(&RecordSeriesKey::new(host, RecordTag::Packfile), 1)
            .await
            .unwrap();
        engine
            .keyed(&key)
            .sync_download(&RecordSeriesKey::new(host, RecordTag::History), 3)
            .await
            .unwrap();

        // The history download must have started AFTER the packed prefix (idx 2), i.e. never
        // requested start=0 for RecordTag::History.
        let requests = server.received_requests().await.unwrap();
        let requested_history_start_0 = requests.iter().any(|r| {
            r.url.path() == "/api/v0/record/next"
                && r.url.query_pairs().any(|(k, v)| k == "tag" && v == RecordTag::History.as_str())
                && r.url.query_pairs().any(|(k, v)| k == "start" && v == "0")
        });
        assert!(!requested_history_start_0, "packed history range must not be re-requested loose");
    }

    #[rstest]
    #[tokio::test]
    async fn sync_download_does_not_underflow_when_local_head_exceeds_remote(
        key: paseto_v4::Key,
        #[future] server: MockServer,
    ) {
        let host = HostId(uuid_v7());

        // Local store already has HISTORY [0..=4] (head idx 4).
        let down = memory_store().await;
        seed_history(&down, host, &key, 5).await;

        // Server that returns empty for any record page (nothing new to fetch).
        let server = server.await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(Vec::<Record<EncryptedData>>::new()),
            )
            .mount(&server)
            .await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        // remote (2) is BEHIND the live local head (4) -- must not underflow/panic.
        let got = build_engine(client, down)
            .await
            .keyed(&key)
            .sync_download(&RecordSeriesKey::new(host, RecordTag::History), 2)
            .await
            .unwrap();
        assert!(got.is_empty(), "nothing to download when local head already exceeds remote");
    }

    /// Serve `records` split into pages of `serve_size`, keyed on the `start` query param
    /// (`idx >= start ORDER BY idx ASC LIMIT count`, dense). `serve_size` may be smaller than
    /// the client's page size to emulate a server that clamps `count`.
    async fn mount_paged_history(
        server: &MockServer,
        records: &[Record<EncryptedData>],
        serve_size: usize,
    ) {
        for start in (0..records.len()).step_by(serve_size) {
            let end = (start + serve_size).min(records.len());
            Mock::given(method("GET"))
                .and(path("/api/v0/record/next"))
                .and(query_param("tag", RecordTag::History.as_str()))
                .and(query_param("start", start.to_string()))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(records[start..end].to_vec()),
                )
                .mount(server)
                .await;
        }
        // Any start past the end -> empty.
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .and(query_param("tag", RecordTag::History.as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(Vec::<Record<EncryptedData>>::new()),
            )
            .mount(server)
            .await;
    }

    #[rstest]
    #[case::paginated(5, 2, 4)]
    #[case::short_page_recovery(6, 4, 5)]
    #[tokio::test]
    async fn sync_download_reassembles_loose_history(
        key: paseto_v4::Key,
        #[future] server: MockServer,
        #[case] count: u64,
        #[case] page_size: u64,
        #[case] remote: RecordIdx,
    ) {
        let host = HostId(uuid_v7());
        let up = memory_store().await;
        seed_history(&up, host, &key, count).await;
        let all = up.next(&RecordSeriesKey::new(host, RecordTag::History), 0, count).await.unwrap();

        let server = server.await;
        mount_paged_history(&server, &all, 2).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let returned = build_engine(client, down.clone())
            .await
            .with_page_size(NonZeroU64::new(page_size).unwrap())
            .keyed(&key)
            .sync_download(&RecordSeriesKey::new(host, RecordTag::History), remote)
            .await
            .unwrap();

        assert_eq!(
            down.next(&RecordSeriesKey::new(host, RecordTag::History), 0, count)
                .await
                .unwrap()
                .len() as u64,
            count,
            "every page must be reassembled, none skipped"
        );
        assert_eq!(returned.len() as u64, count);
    }

    /// Build `num_packs` contiguous packfiles of `per` history records each for a single host,
    /// returning every manifest paired with the blob a server would serve, plus all covered history
    /// ids in idx order.
    #[fixture]
    async fn packed_packfiles(
        key: paseto_v4::Key,
        #[default(3)] per: u64,
        #[default(2)] num_packs: u64,
    ) -> (HostId, Vec<(Record<EncryptedData>, Vec<u8>)>, Vec<RecordId>) {
        let host = HostId(uuid_v7());
        let up = memory_store().await;
        seed_history(&up, host, &key, per * num_packs).await;
        try_pack(
            &up,
            &RecordSeriesKey::new(host, RecordTag::History),
            Some(PackfileCap {
                version: 1,
                record_count: per,
            }),
        )
        .await
        .unwrap();

        let manifests =
            up.next(&RecordSeriesKey::new(host, RecordTag::Packfile), 0, num_packs).await.unwrap();
        assert_eq!(
            manifests.len() as u64,
            num_packs,
            "fixture expects exactly one manifest per pack"
        );

        let mut packs = Vec::new();
        for manifest in &manifests {
            let view = PackManifestRecordView::new(manifest).unwrap();
            let (blob, _ids) = view.pack_records(&up, key.clone()).await.unwrap();
            packs.push((manifest.clone(), blob));
        }

        let history_ids: Vec<RecordId> = up
            .next(&RecordSeriesKey::new(host, RecordTag::History), 0, per * num_packs)
            .await
            .unwrap()
            .iter()
            .map(|record| record.id)
            .collect();

        (host, packs, history_ids)
    }

    /// Mount `/api/v0/packfiles/{id}` -> a per-manifest download URL, and that URL -> the blob, for
    /// each pack. Each gets a distinct download path so the batch fetches don't alias.
    async fn mount_packfile_blobs(server: &MockServer, packs: &[(Record<EncryptedData>, Vec<u8>)]) {
        for (i, (manifest, blob)) in packs.iter().enumerate() {
            let download_path = format!("/download/{i}");
            Mock::given(method("GET"))
                .and(path(format!("/api/v0/packfiles/{}", manifest.id.0)))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "download_url": format!("{}{}", server.uri(), download_path) })))
                .mount(server)
                .await;
            Mock::given(method("GET"))
                .and(path(download_path))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(blob.clone()))
                .mount(server)
                .await;
        }
    }

    #[rstest]
    #[case::single(1, false)]
    #[case::batch(2, false)]
    #[case::batch_with_poison(2, true)]
    #[tokio::test]
    async fn sync_download_expands_packfiles(
        key: paseto_v4::Key,
        #[future] server: MockServer,
        #[case] num_packs: u64,
        #[case] poison: bool,
    ) {
        let (host, packs, history_ids) = packed_packfiles(key.clone(), 3, num_packs).await;
        let total = num_packs * 3;

        let mut page: Vec<Record<EncryptedData>> = packs.iter().map(|(m, _)| m.clone()).collect();
        if poison {
            page.push(
                Record::builder()
                    .host(Host::new(host))
                    .version(RecordVersion::V1)
                    .tag(RecordTag::Packfile)
                    .idx(total)
                    .data(EncryptedData {
                        raw: "001{not json".into(),
                        cek: String::new(),
                    })
                    .build(),
            );
        }

        let server = server.await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .and(query_param("start", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(Vec::<Record<EncryptedData>>::new()),
            )
            .mount(&server)
            .await;
        mount_packfile_blobs(&server, &packs).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let returned = build_engine(client, down.clone())
            .await
            .keyed(&key)
            .sync_download(&RecordSeriesKey::new(host, RecordTag::Packfile), num_packs)
            .await
            .expect("a permanent per-manifest failure must not fail the tick");

        assert!(
            down.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().is_some()
        );
        let history =
            down.next(&RecordSeriesKey::new(host, RecordTag::History), 0, total).await.unwrap();
        assert_eq!(history.len() as u64, total, "all covered history must be populated");
        assert_eq!(history[0].clone().decrypt(&key).unwrap().data.0, b"cmd 0");
        for id in &history_ids {
            assert!(
                returned.contains(id),
                "expanded history id {id:?} must be returned for indexing"
            );
        }
    }

    /// A synthetic prepared-blob item; `upload_packfiles` never inspects the contents, only ships
    /// them, so real packing isn't needed to exercise the batching.
    fn packed_item() -> PackedPackfile {
        PackedPackfile {
            manifest_id: RecordId(uuid_v7()),
            records: vec![RecordId(uuid_v7())],
            blob: vec![1, 2, 3],
        }
    }

    /// A client pointed at a dead address with short timeouts, for the paths that must not reach the
    /// network (already-local skips, parse failures) or that exercise a transport fault.
    fn dead_client() -> Client {
        let addr: url::Url = "http://127.0.0.1:1/".parse().unwrap();
        let caps = caps_client(&addr, &HashMap::new()).unwrap();
        Client::new(addr, &AuthToken::Token("t".into()), 1, 1, &HashMap::new(), caps).unwrap()
    }

    /// Building the view rejects an inverted plaintext range (`start_idx > end_idx`) -- the
    /// precondition every pack/download caller relies on, so the covered-record count never
    /// underflows and no upload or fetch runs.
    #[rstest]
    #[case::far_apart(100, 5)]
    #[case::adjacent(6, 5)]
    #[case::extreme(u64::MAX, 0)]
    fn an_inverted_range_is_rejected_when_building_the_view(
        #[case] start_idx: u64,
        #[case] end_idx: u64,
    ) {
        let host = HostId(uuid_v7());
        // The packer never emits an inverted range, so craft the manifest directly.
        let data = PackManifestDataV1 {
            host,
            tag: RecordTag::History,
            start_idx,
            end_idx,
        }
        .encode()
        .unwrap();
        let manifest = Record::builder()
            .host(Host::new(host))
            .version(RecordVersion::V1)
            .tag(RecordTag::Packfile)
            .idx(0)
            .data(data)
            .build();

        assert!(
            PackManifestRecordView::new(&manifest).is_err(),
            "inverted manifest range must be rejected, not underflow the count"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn uploads_the_packed_range(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());
        let (manifest, blob, ids) = packed_packfile_with_ids(host, &key, 5).await;

        // The two-step upload: create -> presigned URL, PUT the body, then confirm. `.expect(1)` on
        // each verifies create + put + confirm fire exactly once.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/packfiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "upload_url": format!("{}/upload/abc", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/abc"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/api/v0/packfiles/[^/]+/confirm$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "status": "confirmed" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);
        client
            .upload_packfiles(futures::stream::iter([Ok::<_, eyre::Report>(PackedPackfile {
                manifest_id: manifest.id,
                records: ids,
                blob,
            })]))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn upload_packfiles_transfers_every_item() {
        // Two items each drive create -> put -> confirm exactly once (asserted via `.expect(2)`), so
        // the batching visits every item rather than stopping at the first.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/packfiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "upload_url": format!("{}/upload/abc", server.uri()),
            })))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/abc"))
            .respond_with(ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/api/v0/packfiles/[^/]+/confirm$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "status": "confirmed" })),
            )
            .expect(2)
            .mount(&server)
            .await;

        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);
        client
            .upload_packfiles(futures::stream::iter([
                Ok::<_, eyre::Report>(packed_item()),
                Ok(packed_item()),
            ]))
            .await
            .unwrap();
    }

    /// When the covered range is already local, `download_packed` skips the fetch entirely (dead
    /// address proves no network call) but still returns the covered ids so the id-driven
    /// history.db rebuild can re-index them.
    #[rstest]
    #[tokio::test]
    async fn download_packed_returns_range_ids_when_already_local(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());
        let (manifest, _blob) = packed_packfile(host, &key, 5).await;

        let down = memory_store().await;
        seed_history(&down, host, &key, 5).await;
        let expected_ids: Vec<RecordId> = down
            .next(&RecordSeriesKey::new(host, RecordTag::History), 0, 5)
            .await
            .unwrap()
            .iter()
            .map(|record| record.id)
            .collect();

        let ids = build_engine(dead_client(), down)
            .await
            .keyed(&key)
            .download_packed(&manifest)
            .await
            .unwrap();
        assert_eq!(ids, expected_ids, "range already local -> covered ids returned anyway");
    }

    /// A malformed manifest (unknown version / bad JSON / inverted range) fails at parse before any
    /// network I/O and classifies as PERMANENT, so `expand_manifests` skips it instead of failing
    /// the tick. The skip-and-continue is exercised end-to-end by
    /// `sync_download_skips_a_permanent_failure_within_a_batch`; this pins the classification.
    #[rstest]
    #[case::unknown_version(|_| EncryptedData { raw: "999{}".into(), cek: String::new() })]
    #[case::malformed_body(|_| EncryptedData { raw: "001{not json".into(), cek: String::new() })]
    #[case::inverted_range(|host| PackManifestDataV1 {
        tag: RecordTag::History,
        host,
        start_idx: 100,
        end_idx: 5,
    }.encode().unwrap())]
    #[tokio::test]
    async fn a_malformed_manifest_is_classified_permanent(
        key: paseto_v4::Key,
        #[case] bad_data: impl FnOnce(HostId) -> EncryptedData,
    ) {
        let host = HostId(uuid_v7());
        let bad = Record::builder()
            .host(Host::new(host))
            .version(RecordVersion::V1)
            .tag(RecordTag::Packfile)
            .idx(0)
            .data(bad_data(host))
            .build();

        let err = build_engine(dead_client(), memory_store().await)
            .await
            .keyed(&key)
            .download_packed(&bad)
            .await
            .expect_err("a malformed manifest must not expand");
        assert!(
            err.is_permanent(),
            "the caller must skip this manifest, not fail the tick: {err:?}"
        );
    }

    /// GUARD: a TRANSIENT fault (connection-refused packfile GET) surfaces as `Api` and is NOT
    /// permanent, so it propagates and fails the tick rather than being masked as a per-manifest
    /// skip.
    #[rstest]
    #[tokio::test]
    async fn a_transport_failure_is_not_permanent(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());
        // A valid manifest over a non-empty range, so a fresh store reaches the packfile fetch.
        let manifest = Record::builder()
            .host(Host::new(host))
            .version(RecordVersion::V1)
            .tag(RecordTag::Packfile)
            .idx(0)
            .data(
                PackManifestDataV1 {
                    host,
                    tag: RecordTag::History,
                    start_idx: 0,
                    end_idx: 2,
                }
                .encode()
                .unwrap(),
            )
            .build();

        let result = build_engine(dead_client(), memory_store().await)
            .await
            .keyed(&key)
            .download_packed(&manifest)
            .await;
        assert!(
            matches!(result, Err(PackfileDownloadError::Api(_))),
            "a transient transport fault must surface as Api: {result:?}"
        );
        assert!(
            !result.unwrap_err().is_permanent(),
            "a transient transport fault must propagate, not be skipped"
        );
    }
}

#[cfg(test)]
mod packfile_capability_tests {
    use atuin_common::encryption::paseto_v4;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::caps::{CapServer, CapabilitiesCap, PackfileCap};
    use atuin_domain::record::{
        EncryptedData, HostId, Record, RecordIdx, RecordSeriesKey, RecordTag,
    };
    use rstest::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::Operation;
    use super::packfile_sync_tests::{
        build_engine, key, memory_store, mock_client, mount_packfile, packed_packfile,
        seed_history, server,
    };

    /// Mount `/api/v0/capabilities` returning the given verbatim body string.
    async fn mount_caps_body(server: &MockServer, body: String) {
        Mock::given(method("GET"))
            .and(path("/api/v0/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    /// Build `count` loose encrypted HISTORY records (idx `0..count`) as a server would serve them.
    async fn loose_history(
        host: HostId,
        key: &paseto_v4::Key,
        count: u64,
    ) -> Vec<Record<EncryptedData>> {
        let up = memory_store().await;
        seed_history(&up, host, key, count).await;
        up.next(&RecordSeriesKey::new(host, RecordTag::History), 0, count).await.unwrap()
    }

    /// A single PACKFILE download op covering `remote` manifests from `host`.
    fn packfile_download_op(host: HostId, remote: RecordIdx) -> Operation {
        Operation::Download {
            remote,
            series: RecordSeriesKey::new(host, RecordTag::Packfile),
        }
    }

    #[rstest]
    #[tokio::test]
    async fn packfile_op_runs_when_server_advertises_cap(
        key: paseto_v4::Key,
        #[future] server: MockServer,
    ) {
        let host = HostId(uuid_v7());
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = server.await;
        let caps = CapServer::new()
            .add(CapabilitiesCap { version: 1 })
            .unwrap()
            .add(PackfileCap {
                version: 1,
                record_count: 500,
            })
            .unwrap();
        mount_caps_body(&server, caps.body().to_owned()).await;
        mount_packfile(&server, &manifest, blob).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        build_engine(client, down.clone())
            .await
            .keyed(&key)
            .sync_remote(vec![packfile_download_op(host, 3)])
            .await
            .unwrap();

        // Cap advertised -> the whole packfile op ran: the manifest was persisted and its history
        // was expanded into the store.
        assert!(
            down.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().is_some()
        );
        assert_eq!(
            down.next(&RecordSeriesKey::new(host, RecordTag::History), 0, 3).await.unwrap().len(),
            3
        );
    }

    #[rstest]
    #[tokio::test]
    async fn absent_cap_skips_packfile_but_loose_history_still_syncs(
        key: paseto_v4::Key,
        #[future] server: MockServer,
    ) {
        let host = HostId(uuid_v7());
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = server.await;
        // Caps advertised, but NO PackfileCap -> get_server::<PackfileCap>() == Ok(None).
        let caps = CapServer::new().add(CapabilitiesCap { version: 1 }).unwrap();
        mount_caps_body(&server, caps.body().to_owned()).await;
        // Tag-scoped packfile mocks (not `mount_packfile`, whose start=0 matcher is tag-agnostic and
        // would collide with the history op). They exist so that, were the gate broken, the
        // packfile op would succeed and persist the manifest -- making the negative assertion
        // meaningful.
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .and(query_param("tag", RecordTag::Packfile.as_str()))
            .and(query_param("start", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![manifest.clone()]))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .and(query_param("tag", RecordTag::Packfile.as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(Vec::<Record<EncryptedData>>::new()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v0/packfiles/{}", manifest.id.0)))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "download_url": format!("{}/download/abc", server.uri()) })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/abc"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(blob))
            .mount(&server)
            .await;
        // A parallel loose-history op that the server does serve.
        let loose = loose_history(host, &key, 3).await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .and(query_param("tag", RecordTag::History.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(loose))
            .mount(&server)
            .await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        build_engine(client, down.clone())
            .await
            .keyed(&key)
            .sync_remote(vec![packfile_download_op(host, 3), Operation::Download {
                remote: 3,
                series: RecordSeriesKey::new(host, RecordTag::History),
            }])
            .await
            .unwrap();

        // Packfile op skipped: manifest not persisted. Loose history still synced (no data loss).
        assert!(
            down.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().is_none()
        );
        assert_eq!(
            down.next(&RecordSeriesKey::new(host, RecordTag::History), 0, 3).await.unwrap().len(),
            3
        );
    }

    fn caps_without_packfile() -> String {
        CapServer::new().add(CapabilitiesCap { version: 1 }).unwrap().body().to_owned()
    }

    fn malformed_caps_body() -> String {
        serde_json::json!({
            "version": "x",
            "capabilities": { "sh.atuin.server/records.packfile": { "unexpected": true } }
        })
        .to_string()
    }

    #[rstest]
    #[case::not_fetched(None)]
    #[case::no_packfile_cap(Some(caps_without_packfile()))]
    #[case::malformed(Some(malformed_caps_body()))]
    #[tokio::test]
    async fn bad_cap_skips_packfile_op(
        key: paseto_v4::Key,
        #[future] server: MockServer,
        #[case] caps_body: Option<String>,
    ) {
        let host = HostId(uuid_v7());
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = server.await;
        if let Some(body) = caps_body {
            mount_caps_body(&server, body).await;
        }
        mount_packfile(&server, &manifest, blob).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        build_engine(client, down.clone())
            .await
            .keyed(&key)
            .sync_remote(vec![packfile_download_op(host, 3)])
            .await
            .unwrap();

        assert!(
            down.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().is_none()
        );
        let requests = server.received_requests().await.unwrap();
        assert!(
            !requests.iter().any(|r| r.url.path().starts_with("/api/v0/packfiles")),
            "no /api/v0/packfiles request must be made when packfiles are gated off"
        );
    }
}
