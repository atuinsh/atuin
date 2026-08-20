//! The core sync engine that Atuin uses.
//!
//! This is the library that handles syncing records between a client and a server.
//!
//! TODO(markovejnovic): Migrate this outside of `record/`, since it handles a lot more than just
//!                      records.
//!
//! > do a sync :O
use std::cmp::Ordering;
use std::fmt::Write;

use atuin_common::encryption::paseto_v4;
use atuin_common::sync::EagerFutureCell;
use atuin_domain::caps::PackfileCap;
use atuin_domain::record::{Diff, RecordId, RecordIdx, RecordSeriesKey, RecordStatus, RecordTag};
use eyre::Result;
use futures::{StreamExt, stream};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use thiserror::Error;
use tokio::runtime::Handle;
use tracing::instrument;

use super::sqlite_store::SqliteStore;
use crate::api_client::Client;

mod builder;
mod packfile;
pub use builder::{ClientSource, SyncEngineBuilder, SyncEngineInit};

/// How many packfile blobs to transfer (upload or download) concurrently within a single page.
const MAX_CONCURRENT_PACKFILE_TRANSFERS: usize = 16;

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
}

/// A [`SyncEngine`] paired with an encryption key, for the operations that encrypt or decrypt.
/// Obtained from [`SyncEngine::keyed`].
pub struct Keyed<'k> {
    engine: &'k SyncEngine,
    key: &'k paseto_v4::Key,
    /// The result of verifying `key` against the remote. Read via [`Self::key_valid`].
    key_check: EagerFutureCell<Option<SyncError>>,
}

impl SyncEngine {
    /// Pair this engine with an encryption `key` to run the crypto-touching sync operations.
    pub fn keyed<'k>(&'k self, key: &'k paseto_v4::Key) -> Keyed<'k> {
        let engine = self.clone();
        let key_for_check = key.clone();
        let key_check = EagerFutureCell::new(
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

        let records = match self.client.next_records(&series, 0, 1).await {
            Ok(records) => records,
            Err(e) => return Some(SyncError::RemoteRequestError { msg: e.to_string() }),
        };

        let record = records.into_iter().next()?;

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
            Operation::Noop { series } => (0u8, series.host, 0u8, series.tag.clone()),
            Operation::Upload { series, .. } => (1u8, series.host, 0u8, series.tag.clone()),
            Operation::Download { series, .. } => {
                // Packfile manifests must expand before the history download runs, as that
                // `sync_download` will dedupe will have a chance at avoiding unnecessary downloads.
                let tag_priority = if series.tag == RecordTag::Packfile {
                    0u8
                } else {
                    1u8
                };
                (2u8, series.host, tag_priority, series.tag.clone())
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
        fields(host = ?series.host, tag = ?series.tag, local, remote = ?remote, page_size),
        err
    )]
    async fn sync_upload(
        &self,
        series: &RecordSeriesKey,
        local: RecordIdx,
        remote: Option<RecordIdx>,
        page_size: u64,
    ) -> Result<u64, SyncError> {
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

        println!("Uploading {} records to {}/{}", expected, series.host.0.as_simple(), series.tag);

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
                let mut uploads = stream::iter(0..page.len())
                    .map(|i| self.upload_packed(&page[i]))
                    .buffered(MAX_CONCURRENT_PACKFILE_TRANSFERS);

                while let Some(result) = uploads.next().await {
                    result.map_err(|e| {
                        error!("failed to upload packfile: {e}");
                        SyncError::RemoteRequestError { msg: e.to_string() }
                    })?;
                }
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
        fields(host = ?series.host, tag = ?series.tag, remote = ?remote, page_size),
        err
    )]
    async fn sync_download(
        &self,
        series: &RecordSeriesKey,
        remote: RecordIdx,
        page_size: u64,
    ) -> Result<Vec<RecordId>, SyncError> {
        let store = &self.engine.store;
        let client = &self.engine.client;
        // Scan the database to find the first missing local index, rather than assuming it's one more
        // than the highest local index. A prior packfile op for this host may have expanded a pack
        // whose history landed ABOVE a still-missing index; keying off the highest index would never
        // fetch the hole before it. Start from the actual missing index; records already present above
        // it will be "unnecessarily" redownloaded, but this is a no-op.
        let first_missing_local = store
            .first_gap(series)
            .await
            .map_err(|e| SyncError::LocalStoreError { msg: e.to_string() })?;

        // One higher than the latest record index we have locally. The case described above where we
        // have a hole in the sequence of record indices should not happen in practice; this variable is
        // used to detect that situation so we can print a warning.
        //
        // TODO: This adds a slight runtime cost, but while the packfile feature is new, let's err on
        // the side of catching potential problems.
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
        let mut progress = 0;
        let mut ret = Vec::new();

        println!(
            "Downloading {} records from {}/{}",
            expected,
            series.host.0.as_simple(),
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

        while progress < expected {
            let page = client
                .next_records(series, first_missing_local + progress, page_size)
                .await
                .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?;

            if page.is_empty() {
                break;
            }

            // We commit the packfile's history into the local store before we persist the manifests, so
            // a manifest we recorded always has its associated history.
            if series.tag == RecordTag::Packfile {
                let mut downloads = stream::iter(0..page.len())
                    .map(|i| self.download_packed(&page[i]))
                    .buffered(MAX_CONCURRENT_PACKFILE_TRANSFERS)
                    .enumerate();

                while let Some((i, result)) = downloads.next().await {
                    match result {
                        Ok(expanded) => ret.extend(expanded),
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
            }

            store
                .push_batch(page.iter())
                .await
                .map_err(|e| SyncError::LocalStoreError { msg: e.to_string() })?;

            ret.extend(page.iter().map(|f| f.id));

            progress += page.len() as u64;
            pb.set_position(progress);
        }

        pb.finish_with_message("Downloaded records");

        Ok(ret)
    }

    #[instrument(level = "trace", skip_all, fields(page_size), err)]
    pub async fn sync_remote(
        &self,
        operations: Vec<Operation>,
        page_size: u64,
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
                    uploaded += self.sync_upload(&series, local, remote, page_size).await?
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
                    let mut d = self.sync_download(&series, remote, page_size).await?;
                    downloaded.append(&mut d)
                }

                Operation::Noop { .. } => continue,
            }
        }

        Ok((uploaded, downloaded))
    }

    /// The verdict of verifying this `Keyed`'s key against the remote.
    pub async fn key_valid(&self) -> Option<SyncError> {
        self.key_check.get().await.clone()
    }

    /// Run a full sync: diff local against remote, verify the key can read the remote, resolve the
    /// diff into operations, then apply them.
    #[instrument(level = "trace", skip_all, err)]
    pub async fn sync(&self) -> Result<(u64, Vec<RecordId>), SyncError> {
        let (diff, _remote_index) = self.engine.diff().await?;

        if let Some(err) = self.key_valid().await {
            return Err(err);
        }

        let operations = SyncEngine::operations(diff)?;
        self.sync_remote(operations, 100).await
    }
}

#[cfg(test)]
mod tests {
    use atuin_domain::record::{Diff, EncryptedData, HostId, Record, RecordSeriesKey, RecordTag};
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use crate::record::sqlite_store::SqliteStore;
    use crate::record::sync::{Operation, SyncEngine};
    use crate::settings::test_local_timeout;

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
    #[tokio::test]
    async fn test_basic_diff() {
        // a diff where local is ahead of remote. nothing else.

        let record = test_record();
        let (_store, diff) = build_test_diff(vec![record.clone()], vec![]).await;

        assert_eq!(diff.len(), 1);

        let operations = SyncEngine::operations(diff).unwrap();

        assert_eq!(operations.len(), 1);

        assert_eq!(operations[0], Operation::Upload {
            series: RecordSeriesKey::new(record.host.id, record.tag.clone()),
            local: record.idx,
            remote: None,
        });
    }

    #[rstest]
    #[tokio::test]
    async fn build_two_way_diff() {
        // a diff where local is ahead of remote for one, and remote for
        // another. One upload, one download

        let shared_record = test_record();
        let remote_ahead = test_record();

        let local_ahead = shared_record.append(vec![1, 2, 3]).encrypt(&[0; 32].into());

        assert_eq!(local_ahead.idx, 1);

        let local = vec![shared_record.clone(), local_ahead.clone()]; // local knows about the already synced, and something newer in the same store
        let remote = vec![shared_record.clone(), remote_ahead.clone()]; // remote knows about the already-synced, and one new record in a new store

        let (_store, diff) = build_test_diff(local, remote).await;
        let operations = SyncEngine::operations(diff).unwrap();

        assert_eq!(operations.len(), 2);

        assert_eq!(operations, vec![
            // Or in otherwords, local is ahead by one
            Operation::Upload {
                series: RecordSeriesKey::new(local_ahead.host.id, local_ahead.tag.clone()),
                local: 1,
                remote: Some(0),
            },
            // Or in other words, remote knows of a record in an entirely new store (tag)
            Operation::Download {
                series: RecordSeriesKey::new(remote_ahead.host.id, remote_ahead.tag.clone()),
                remote: 0,
            },
        ]);
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
            Operation::Noop { series } => (0, series.host, series.tag.clone()),

            Operation::Upload { series, .. } => (1, series.host, series.tag.clone()),

            Operation::Download { series, .. } => (2, series.host, series.tag.clone()),
        });

        assert_eq!(result_ops, operations);
    }
}

#[cfg(test)]
mod packfile_download_tests {
    use std::collections::HashMap;

    use atuin_common::encryption::paseto_v4;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{
        DecryptedData, EncryptedData, Host, HostId, Record, RecordId, RecordVersion,
    };
    use rstest::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::api_client::{AuthToken, Client, caps_client};
    use crate::packfile::{PackManifestRecordView, try_pack};
    use crate::record::sqlite_store::SqliteStore;
    use crate::settings::test_local_timeout;

    /// A single fixed encryption key. The specific bytes are arbitrary in these tests -- each one
    /// packs and unpacks with the same key -- so one shared value keeps setup uniform.
    #[fixture]
    fn key() -> paseto_v4::Key {
        paseto_v4::Key::from([7u8; 32])
    }

    #[fixture]
    async fn server() -> MockServer {
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
    async fn sync_download_expands_packfile_manifests_into_history(
        key: paseto_v4::Key,
        #[future] server: MockServer,
    ) {
        let host = HostId(uuid_v7());

        // Uploader-side artifacts: history + manifest + blob.
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = server.await;
        mount_packfile(&server, &manifest, blob).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        build_engine(client, down.clone())
            .await
            .keyed(&key)
            .sync_download(&RecordSeriesKey::new(host, RecordTag::Packfile), 1, 100)
            .await
            .unwrap();

        // The manifest is stored AND the history it covers was populated.
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
    async fn sync_download_returns_expanded_history_ids_for_indexing(
        key: paseto_v4::Key,
        #[future] server: MockServer,
    ) {
        let host = HostId(uuid_v7());

        let (manifest, blob, history_ids) = packed_packfile_with_ids(host, &key, 3).await;

        let server = server.await;
        mount_packfile(&server, &manifest, blob).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let returned = build_engine(client, down)
            .await
            .keyed(&key)
            .sync_download(&RecordSeriesKey::new(host, RecordTag::Packfile), 1, 100)
            .await
            .unwrap();

        for id in &history_ids {
            assert!(
                returned.contains(id),
                "expanded history id {id:?} must be returned for indexing"
            );
        }
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
            .sync_download(&RecordSeriesKey::new(host, RecordTag::Packfile), 1, 100)
            .await
            .unwrap();
        engine
            .keyed(&key)
            .sync_download(&RecordSeriesKey::new(host, RecordTag::History), 3, 100)
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
            .sync_download(&RecordSeriesKey::new(host, RecordTag::History), 2, 100)
            .await
            .unwrap();
        assert!(got.is_empty(), "nothing to download when local head already exceeds remote");
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
    #[tokio::test]
    async fn sync_download_expands_a_batch_of_packfile_manifests(
        key: paseto_v4::Key,
        #[future] server: MockServer,
        #[future] packed_packfiles: (HostId, Vec<(Record<EncryptedData>, Vec<u8>)>, Vec<RecordId>),
    ) {
        // Two contiguous packfiles (idx 0..=2 and 3..=5) delivered together in one page.
        let (host, packs, history_ids) = packed_packfiles.await;
        let server = server.await;
        let page: Vec<Record<EncryptedData>> = packs.iter().map(|(m, _)| m.clone()).collect();
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
            .sync_download(&RecordSeriesKey::new(host, RecordTag::Packfile), 1, 100)
            .await
            .unwrap();

        // Every batched packfile's history is populated, whichever download finished first.
        assert_eq!(
            down.next(&RecordSeriesKey::new(host, RecordTag::History), 0, 6).await.unwrap().len(),
            6,
            "all history across the batch must be populated"
        );
        for id in &history_ids {
            assert!(
                returned.contains(id),
                "expanded history id {id:?} must be returned for indexing"
            );
        }
    }

    #[rstest]
    #[tokio::test]
    async fn sync_download_skips_a_permanent_failure_within_a_batch(
        key: paseto_v4::Key,
        #[future] server: MockServer,
        #[future] packed_packfiles: (HostId, Vec<(Record<EncryptedData>, Vec<u8>)>, Vec<RecordId>),
    ) {
        // Two valid contiguous packfiles...
        let (host, packs, history_ids) = packed_packfiles.await;

        // ...plus a malformed manifest riding in the same page. It fails to parse before any network
        // I/O (permanent), so it needs no download mock of its own.
        let bad = Record::builder()
            .host(Host::new(host))
            .version(RecordVersion::V1)
            .tag(RecordTag::Packfile)
            .idx(2)
            .data(EncryptedData {
                raw: "001{not json".into(),
                cek: String::new(),
            })
            .build();

        let server = server.await;
        let page: Vec<Record<EncryptedData>> =
            packs.iter().map(|(m, _)| m.clone()).chain(std::iter::once(bad.clone())).collect();
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

        // The permanent per-manifest failure is logged and skipped; the whole tick still succeeds.
        let returned = build_engine(client, down.clone())
            .await
            .keyed(&key)
            .sync_download(&RecordSeriesKey::new(host, RecordTag::Packfile), 2, 100)
            .await
            .expect("a permanent per-manifest failure must not fail the tick");

        // The two valid packfiles in the batch still expand despite the poisoned sibling.
        assert_eq!(
            down.next(&RecordSeriesKey::new(host, RecordTag::History), 0, 6).await.unwrap().len(),
            6,
            "valid packfiles in the batch must still expand"
        );
        for id in &history_ids {
            assert!(
                returned.contains(id),
                "expanded history id {id:?} must be returned for indexing"
            );
        }
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
    use super::packfile_download_tests::{
        build_engine, memory_store, mock_client, mount_packfile, packed_packfile, seed_history,
    };

    /// A single fixed encryption key, matching the sibling packfile tests.
    #[fixture]
    fn key() -> paseto_v4::Key {
        paseto_v4::Key::from([7u8; 32])
    }

    #[fixture]
    async fn server() -> MockServer {
        MockServer::start().await
    }

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
            .sync_remote(vec![packfile_download_op(host, 3)], 100)
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
            .sync_remote(
                vec![packfile_download_op(host, 3), Operation::Download {
                    remote: 3,
                    series: RecordSeriesKey::new(host, RecordTag::History),
                }],
                100,
            )
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

    #[rstest]
    #[tokio::test]
    async fn not_fetched_skips_packfile_op(key: paseto_v4::Key, #[future] server: MockServer) {
        let host = HostId(uuid_v7());
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = server.await;
        mount_packfile(&server, &manifest, blob).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        build_engine(client, down.clone())
            .await
            .keyed(&key)
            .sync_remote(vec![packfile_download_op(host, 3)], 100)
            .await
            .unwrap();

        // Skipped: manifest not persisted, and no packfile endpoint was ever hit.
        assert!(
            down.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().is_none()
        );
        let requests = server.received_requests().await.unwrap();
        assert!(
            !requests.iter().any(|r| r.url.path().starts_with("/api/v0/packfiles")),
            "no /api/v0/packfiles request must be made when packfiles are gated off"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn malformed_cap_skips_packfile_op(key: paseto_v4::Key, #[future] server: MockServer) {
        let host = HostId(uuid_v7());
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = server.await;
        // Advertised, but the value does not deserialize into PackfileCap { version: u32 } ->
        // get_server::<PackfileCap>() == Err(Malformed) -> gate disabled (conservative).
        let body = serde_json::json!({
            "version": "x",
            "capabilities": { "sh.atuin.server/records.packfile": { "unexpected": true } }
        })
        .to_string();
        mount_caps_body(&server, body).await;
        mount_packfile(&server, &manifest, blob).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        build_engine(client, down.clone())
            .await
            .keyed(&key)
            .sync_remote(vec![packfile_download_op(host, 3)], 100)
            .await
            .unwrap();

        assert!(
            down.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().is_none()
        );
    }
}
