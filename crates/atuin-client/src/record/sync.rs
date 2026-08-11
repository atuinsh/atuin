// do a sync :O
use std::{cmp::Ordering, fmt::Write, sync::Arc};

use eyre::Result;
use thiserror::Error;

use super::sqlite_store::SqliteStore;
use crate::{
    api_client::Client,
    packfile::{download_packed, upload_packed},
    settings::Settings,
};

use atuin_common::encryption::paseto_v4;
use atuin_domain::caps::{CapClient, PackfileCap};
use atuin_domain::record::{Diff, HostId, RecordId, RecordIdx, RecordStatus, RecordTag};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("the local store is ahead of the remote, but for another host. has remote lost data?")]
    LocalAheadOtherHost,

    #[error("an issue with the local database occurred: {msg:?}")]
    LocalStoreError { msg: String },

    #[error("something has gone wrong with the sync logic: {msg:?}")]
    SyncLogicError { msg: String },

    #[error("operational error: {msg:?}")]
    OperationalError { msg: String },

    #[error("a request to the sync server failed: {msg:?}")]
    RemoteRequestError { msg: String },

    #[error(
        "the encryption key on this machine does not match the data on the server. \
         this usually means a new machine was set up without copying the existing key. \
         to fix: run `atuin key` on a machine that already syncs correctly, then run \
         `atuin store rekey <key>` on this machine with the value from the other machine"
    )]
    WrongKey,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Operation {
    // Either upload or download until the states matches the below
    Upload {
        local: RecordIdx,
        remote: Option<RecordIdx>,
        host: HostId,
        tag: RecordTag,
    },
    Download {
        local: Option<RecordIdx>,
        remote: RecordIdx,
        host: HostId,
        tag: RecordTag,
    },
    Noop {
        host: HostId,
        tag: RecordTag,
    },
}

pub async fn build_client(settings: &Settings, caps: Arc<CapClient>) -> Result<Client, SyncError> {
    Client::new(
        settings.sync_address.clone(),
        settings
            .sync_auth_token()
            .await
            .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?,
        settings.network_connect_timeout,
        settings.network_timeout,
        &settings.extra_headers,
        caps,
    )
    .map_err(|e| SyncError::OperationalError { msg: e.to_string() })
}

pub async fn diff(
    client: &Client,
    store: &SqliteStore,
) -> Result<(Vec<Diff>, RecordStatus), SyncError> {
    let local_index = store
        .status()
        .await
        .map_err(|e| SyncError::LocalStoreError { msg: e.to_string() })?;

    let remote_index = client
        .record_status()
        .await
        .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?;

    let diff = local_index.diff(&remote_index);

    Ok((diff, remote_index))
}

// Take a diff, along with a local store, and resolve it into a set of operations.
// With the store as context, we can determine if a tail exists locally or not and therefore if it needs uploading or download.
// In theory this could be done as a part of the diffing stage, but it's easier to reason
// about and test this way
pub async fn operations(
    diffs: Vec<Diff>,
    _store: &SqliteStore,
) -> Result<Vec<Operation>, SyncError> {
    let mut operations = Vec::with_capacity(diffs.len());

    for diff in diffs {
        let op = match (diff.local, diff.remote) {
            // We both have it! Could be either. Compare.
            (Some(local), Some(remote)) => match local.cmp(&remote) {
                Ordering::Equal => Operation::Noop {
                    host: diff.host,
                    tag: diff.tag,
                },
                Ordering::Greater => Operation::Upload {
                    local,
                    remote: Some(remote),
                    host: diff.host,
                    tag: diff.tag,
                },
                Ordering::Less => Operation::Download {
                    local: Some(local),
                    remote,
                    host: diff.host,
                    tag: diff.tag,
                },
            },

            // Remote has it, we don't. Gotta be download
            (None, Some(remote)) => Operation::Download {
                local: None,
                remote,
                host: diff.host,
                tag: diff.tag,
            },

            // We have it, remote doesn't. Gotta be upload.
            (Some(local), None) => Operation::Upload {
                local,
                remote: None,
                host: diff.host,
                tag: diff.tag,
            },

            // something is pretty fucked.
            (None, None) => {
                return Err(SyncError::SyncLogicError {
                    msg: String::from(
                        "diff has nothing for local or remote - (host, tag) does not exist",
                    ),
                });
            }
        };

        operations.push(op);
    }

    // sort them - purely so we have a stable testing order, and can rely on
    // same input = same output
    // We can sort by ID so long as we continue to use UUIDv7 or something
    // with the same properties

    operations.sort_by_key(|op| match op {
        Operation::Noop { host, tag } => (0u8, *host, 0u8, tag.clone()),
        Operation::Upload { host, tag, .. } => (1u8, *host, 0u8, tag.clone()),
        Operation::Download { host, tag, .. } => {
            // Packfile manifests must expand before the history download runs, as that
            // `sync_download` will dedupe will have a chance at avoiding unnecessary downloads.
            let tag_priority = if *tag == RecordTag::Packfile {
                0u8
            } else {
                1u8
            };
            (2u8, *host, tag_priority, tag.clone())
        }
    });

    Ok(operations)
}

#[allow(
    clippy::too_many_arguments,
    reason = "threading the key for packfile uploads pushes this one param over the limit"
)]
async fn sync_upload(
    store: &SqliteStore,
    client: &Client,
    host: HostId,
    tag: RecordTag,
    local: RecordIdx,
    remote: Option<RecordIdx>,
    page_size: u64,
    key: &paseto_v4::Key,
) -> Result<i64, SyncError> {
    let remote = remote.unwrap_or(0);
    let expected = local - remote;
    let mut progress = 0;

    let pb = ProgressBar::new(expected);
    pb.set_style(ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {human_pos}/{human_len} ({eta})")
        .unwrap()
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap())
        .progress_chars("#>-"));

    println!(
        "Uploading {} records to {}/{}",
        expected,
        host.0.as_simple(),
        tag
    );

    loop {
        let page = store
            .next(host, &tag, remote + progress, page_size)
            .await
            .map_err(|e| {
                error!("failed to read upload page: {e:?}");

                SyncError::LocalStoreError { msg: e.to_string() }
            })?;

        if page.is_empty() {
            break;
        }

        if tag == RecordTag::Packfile {
            for manifest in &page {
                upload_packed(manifest, store, key, client)
                    .await
                    .map_err(|e| {
                        error!("failed to upload packfile: {e:?}");
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

        if progress >= expected {
            break;
        }
    }

    pb.finish_with_message("Uploaded records");

    Ok(progress as i64)
}

#[allow(
    clippy::too_many_arguments,
    reason = "threading the key for packfile downloads pushes this one param over the limit"
)]
async fn sync_download(
    store: &SqliteStore,
    client: &Client,
    host: HostId,
    tag: RecordTag,
    local: Option<RecordIdx>,
    remote: RecordIdx,
    page_size: u64,
    key: &paseto_v4::Key,
) -> Result<Vec<RecordId>, SyncError> {
    // Re-query the current state of the store. A previous `sync_download` with `tag == MANIFEST`
    // may have downloaded new records that we now need to skip downloading.
    let local = match store.last(host, &tag).await {
        Ok(Some(record)) => record.idx.max(local.unwrap_or(0)),
        Ok(None) => local.unwrap_or(0),
        Err(e) => return Err(SyncError::LocalStoreError { msg: e.to_string() }),
    };

    let expected = remote.saturating_sub(local);
    let mut progress = 0;
    let mut ret = Vec::new();

    println!(
        "Downloading {} records from {}/{}",
        expected,
        host.0.as_simple(),
        tag
    );

    let pb = ProgressBar::new(expected);
    pb.set_style(ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {human_pos}/{human_len} ({eta})")
        .unwrap()
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap())
        .progress_chars("#>-"));

    loop {
        let page = client
            .next_records(host, tag.clone(), local + progress, page_size)
            .await
            .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?;

        if page.is_empty() {
            break;
        }

        // We commit the packfile's history into the local store before we persist the manifests, so
        // a manifest we recorded always has its associated history.
        if tag == RecordTag::Packfile {
            for manifest in &page {
                match download_packed(manifest, store, key, client).await {
                    Ok(expanded) => ret.extend(expanded),
                    Err(e) if e.is_permanent() => error!(
                        manifest_id = %manifest.id,
                        host = %manifest.host.id,
                        idx = manifest.idx,
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

        if progress >= expected {
            break;
        }
    }

    pb.finish_with_message("Downloaded records");

    Ok(ret)
}

pub async fn sync_remote(
    client: &Client,
    operations: Vec<Operation>,
    local_store: &SqliteStore,
    page_size: u64,
    key: &paseto_v4::Key,
) -> Result<(i64, Vec<RecordId>), SyncError> {
    let mut uploaded = 0;
    let mut downloaded = Vec::new();

    let packfiles_enabled = matches!(
        client.caps().get_server::<PackfileCap>().await,
        Ok(Some(cap)) if cap.record_count > 0
    );

    // this can totally run in parallel, but lets get it working first
    for i in operations {
        match i {
            Operation::Upload {
                host,
                tag,
                local,
                remote,
            } => {
                if tag == RecordTag::Packfile && !packfiles_enabled {
                    debug!(
                        "server does not advertise PackfileCap; skipping packfile {tag} upload op, loose history covers it"
                    );
                    continue;
                }
                uploaded += sync_upload(
                    local_store,
                    client,
                    host,
                    tag,
                    local,
                    remote,
                    page_size,
                    key,
                )
                .await?
            }

            Operation::Download {
                host,
                tag,
                local,
                remote,
            } => {
                if tag == RecordTag::Packfile && !packfiles_enabled {
                    debug!(
                        "server does not advertise PackfileCap; skipping packfile {tag} download op, loose history covers it"
                    );
                    continue;
                }
                let mut d = sync_download(
                    local_store,
                    client,
                    host,
                    tag,
                    local,
                    remote,
                    page_size,
                    key,
                )
                .await?;
                downloaded.append(&mut d)
            }

            Operation::Noop { .. } => continue,
        }
    }

    Ok((uploaded, downloaded))
}

pub async fn check_encryption_key(
    client: &Client,
    remote_index: &RecordStatus,
    encryption_key: &paseto_v4::Key,
) -> Result<(), SyncError> {
    let sample = remote_index
        .hosts
        .iter()
        .flat_map(|(host, tags)| tags.keys().map(move |tag| (*host, tag.clone())))
        .next();

    let Some((host, tag)) = sample else {
        return Ok(());
    };

    let records = client
        .next_records(host, tag, 0, 1)
        .await
        .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?;

    let Some(record) = records.into_iter().next() else {
        return Ok(());
    };

    record
        .decrypt(encryption_key)
        .map_err(|_| SyncError::WrongKey)?;

    Ok(())
}

pub async fn sync(
    settings: &Settings,
    store: &SqliteStore,
    encryption_key: &paseto_v4::Key,
    caps: Arc<CapClient>,
) -> Result<(i64, Vec<RecordId>), SyncError> {
    let client = build_client(settings, caps).await?;
    let (diff, remote_index) = diff(&client, store).await?;

    // Bail before mutating either side if the local key can't read the remote.
    check_encryption_key(&client, &remote_index, encryption_key).await?;

    let operations = operations(diff, store).await?;
    let (uploaded, downloaded) =
        sync_remote(&client, operations, store, 100, encryption_key).await?;

    Ok((uploaded, downloaded))
}

#[cfg(test)]
mod tests {
    use atuin_domain::record::{Diff, EncryptedData, HostId, Record, RecordTag};
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use crate::{
        record::{
            sqlite_store::SqliteStore,
            sync::{self, Operation},
        },
        settings::test_local_timeout,
    };

    fn test_record() -> Record<EncryptedData> {
        Record::builder()
            .host(atuin_domain::record::Host::new(HostId(
                atuin_common::utils::uuid_v7(),
            )))
            .version("v1".into())
            .tag(RecordTag::Other(
                atuin_common::utils::uuid_v7().simple().to_string(),
            ))
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
        let (store, diff) = build_test_diff(vec![record.clone()], vec![]).await;

        assert_eq!(diff.len(), 1);

        let operations = sync::operations(diff, &store).await.unwrap();

        assert_eq!(operations.len(), 1);

        assert_eq!(
            operations[0],
            Operation::Upload {
                host: record.host.id,
                tag: record.tag.clone(),
                local: record.idx,
                remote: None,
            }
        );
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

        let (store, diff) = build_test_diff(local, remote).await;
        let operations = sync::operations(diff, &store).await.unwrap();

        assert_eq!(operations.len(), 2);

        assert_eq!(
            operations,
            vec![
                // Or in otherwords, local is ahead by one
                Operation::Upload {
                    host: local_ahead.host.id,
                    tag: local_ahead.tag.clone(),
                    local: 1,
                    remote: Some(0),
                },
                // Or in other words, remote knows of a record in an entirely new store (tag)
                Operation::Download {
                    host: remote_ahead.host.id,
                    tag: remote_ahead.tag.clone(),
                    local: None,
                    remote: 0,
                },
            ]
        );
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
        let remote_only_21 = remote_only_20
            .append(vec![2, 3, 2])
            .encrypt(&[0; 32].into());
        let remote_only_22 = remote_only_21
            .append(vec![2, 3, 2])
            .encrypt(&[0; 32].into());
        let remote_only_23 = remote_only_22
            .append(vec![2, 3, 2])
            .encrypt(&[0; 32].into());
        let remote_only_24 = remote_only_23
            .append(vec![2, 3, 2])
            .encrypt(&[0; 32].into());

        let second_shared = test_record();
        let second_shared_remote_ahead =
            second_shared.append(vec![1, 2, 3]).encrypt(&[0; 32].into());
        let second_shared_remote_ahead2 = second_shared_remote_ahead
            .append(vec![1, 2, 3])
            .encrypt(&[0; 32].into());

        let third_shared = test_record();
        let third_shared_local_ahead = third_shared.append(vec![1, 2, 3]).encrypt(&[0; 32].into());
        let third_shared_local_ahead2 = third_shared_local_ahead
            .append(vec![1, 2, 3])
            .encrypt(&[0; 32].into());

        let fourth_shared = test_record();
        let fourth_shared_remote_ahead =
            fourth_shared.append(vec![1, 2, 3]).encrypt(&[0; 32].into());
        let fourth_shared_remote_ahead2 = fourth_shared_remote_ahead
            .append(vec![1, 2, 3])
            .encrypt(&[0; 32].into());

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

        let (store, diff) = build_test_diff(local, remote).await;
        let operations = sync::operations(diff, &store).await.unwrap();

        assert_eq!(operations.len(), 7);

        let mut result_ops = vec![
            // We started with a shared record, but the remote knows of two newer records in the
            // same store
            Operation::Download {
                local: Some(0),
                remote: 2,
                host: second_shared_remote_ahead.host.id,
                tag: second_shared_remote_ahead.tag.clone(),
            },
            // We have a shared record, local knows of the first two but not the last
            Operation::Download {
                local: Some(1),
                remote: 2,
                host: fourth_shared_remote_ahead2.host.id,
                tag: fourth_shared_remote_ahead2.tag.clone(),
            },
            // Remote knows of a store with a single record that local does not have
            Operation::Download {
                local: None,
                remote: 0,
                host: remote_only.host.id,
                tag: remote_only.tag.clone(),
            },
            // Remote knows of a store with a bunch of records that local does not have
            Operation::Download {
                local: None,
                remote: 4,
                host: remote_only_20.host.id,
                tag: remote_only_20.tag.clone(),
            },
            // Local knows of a record in a store that remote does not have
            Operation::Upload {
                local: 0,
                remote: None,
                host: local_only.host.id,
                tag: local_only.tag.clone(),
            },
            // Local knows of 4 records in a store that remote does not have
            Operation::Upload {
                local: 3,
                remote: None,
                host: local_only_20.host.id,
                tag: local_only_20.tag.clone(),
            },
            // Local knows of 2 more records in a shared store that remote only has one of
            Operation::Upload {
                local: 2,
                remote: Some(0),
                host: third_shared.host.id,
                tag: third_shared.tag.clone(),
            },
        ];

        result_ops.sort_by_key(|op| match op {
            Operation::Noop { host, tag } => (0, *host, tag.clone()),

            Operation::Upload { host, tag, .. } => (1, *host, tag.clone()),

            Operation::Download { host, tag, .. } => (2, *host, tag.clone()),
        });

        assert_eq!(result_ops, operations);
    }
}

#[cfg(test)]
mod packfile_download_tests {
    use super::*;
    use std::collections::HashMap;

    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{DecryptedData, EncryptedData, Host, HostId, Record, RecordId};
    use rstest::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::api_client::{AuthToken, Client, caps_client};
    use crate::packfile::PackManifestRecordView;
    use crate::packfile::try_pack;
    use crate::record::sqlite_store::SqliteStore;
    use crate::settings::test_local_timeout;
    use atuin_common::encryption::paseto_v4;

    /// A single fixed encryption key. The specific bytes are arbitrary in these tests -- each one
    /// packs and unpacks with the same key -- so one shared value keeps setup uniform.
    #[fixture]
    fn key() -> paseto_v4::Key {
        paseto_v4::Key::from([7u8; 32])
    }

    /// A [`Client`] pointed at a wiremock server, authenticated with a dummy token.
    pub(super) fn mock_client(addr: &url::Url) -> Client {
        let caps = caps_client(addr, &HashMap::new()).unwrap();
        Client::new(
            addr.clone(),
            AuthToken::Token("t".into()),
            30,
            30,
            &HashMap::new(),
            caps,
        )
        .unwrap()
    }

    /// A fresh in-memory record store.
    pub(super) async fn memory_store() -> SqliteStore {
        SqliteStore::new(":memory:", test_local_timeout())
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
            host,
            Some(PackfileCap {
                version: 1,
                record_count: count,
            }),
            &RecordTag::History,
        )
        .await
        .unwrap();
        let manifest = up.last(host, &RecordTag::Packfile).await.unwrap().unwrap();
        let view = PackManifestRecordView::new(&manifest).unwrap();
        let ids: Vec<RecordId> = view
            .load_encrypted_packed_records(&up)
            .await
            .unwrap()
            .map(|record| record.id)
            .collect();
        let blob = view.pack_records(&up, key.clone()).await.unwrap();
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

    #[rstest]
    #[tokio::test]
    async fn sync_download_expands_packfile_manifests_into_history(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());

        // Uploader-side artifacts: history + manifest + blob.
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = MockServer::start().await;
        mount_packfile(&server, &manifest, blob).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        sync_download(
            &down,
            &client,
            host,
            RecordTag::Packfile,
            None,
            1,
            100,
            &key,
        )
        .await
        .unwrap();

        // The manifest is stored AND the history it covers was populated.
        assert!(
            down.last(host, &RecordTag::Packfile)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            down.next(host, &RecordTag::History, 0, 3)
                .await
                .unwrap()
                .len(),
            3
        );
    }

    #[rstest]
    #[tokio::test]
    async fn sync_download_returns_expanded_history_ids_for_indexing(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());

        let (manifest, blob, history_ids) = packed_packfile_with_ids(host, &key, 3).await;

        let server = MockServer::start().await;
        mount_packfile(&server, &manifest, blob).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let returned = sync_download(
            &down,
            &client,
            host,
            RecordTag::Packfile,
            None,
            1,
            100,
            &key,
        )
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
    async fn history_download_skips_the_range_a_packfile_covered(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());

        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = MockServer::start().await;
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

        // Packfile op first (populates history 0..=2), then the history op.
        sync_download(
            &down,
            &client,
            host,
            RecordTag::Packfile,
            None,
            1,
            100,
            &key,
        )
        .await
        .unwrap();
        sync_download(&down, &client, host, RecordTag::History, None, 3, 100, &key)
            .await
            .unwrap();

        // The history download must have started AFTER the packed prefix (idx 2), i.e. never
        // requested start=0 for RecordTag::History.
        let requests = server.received_requests().await.unwrap();
        let requested_history_start_0 = requests.iter().any(|r| {
            r.url.path() == "/api/v0/record/next"
                && r.url
                    .query_pairs()
                    .any(|(k, v)| k == "tag" && v == RecordTag::History.as_str())
                && r.url.query_pairs().any(|(k, v)| k == "start" && v == "0")
        });
        assert!(
            !requested_history_start_0,
            "packed history range must not be re-requested loose"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn sync_download_does_not_underflow_when_local_head_exceeds_remote(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());

        // Local store already has HISTORY [0..=4] (head idx 4).
        let down = memory_store().await;
        seed_history(&down, host, &key, 5).await;

        // Server that returns empty for any record page (nothing new to fetch).
        let server = MockServer::start().await;
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
        let got = sync_download(
            &down,
            &client,
            host,
            RecordTag::History,
            Some(0),
            2,
            100,
            &key,
        )
        .await
        .unwrap();
        assert!(
            got.is_empty(),
            "nothing to download when local head already exceeds remote"
        );
    }
}

#[cfg(test)]
mod packfile_capability_tests {
    use super::packfile_download_tests::{
        memory_store, mock_client, mount_packfile, packed_packfile, seed_history,
    };
    use super::{Operation, sync_remote};

    use atuin_common::encryption::paseto_v4;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::caps::{CapServer, CapabilitiesCap, PackfileCap};
    use atuin_domain::record::{EncryptedData, HostId, Record, RecordIdx, RecordTag};
    use rstest::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A single fixed encryption key, matching the sibling packfile tests.
    #[fixture]
    fn key() -> paseto_v4::Key {
        paseto_v4::Key::from([7u8; 32])
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
        up.next(host, &RecordTag::History, 0, count).await.unwrap()
    }

    /// A single PACKFILE download op covering `remote` manifests from `host`.
    fn packfile_download_op(host: HostId, remote: RecordIdx) -> Operation {
        Operation::Download {
            local: None,
            remote,
            host,
            tag: RecordTag::Packfile,
        }
    }

    #[rstest]
    #[tokio::test]
    async fn packfile_op_runs_when_server_advertises_cap(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = MockServer::start().await;
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

        sync_remote(
            &client,
            vec![packfile_download_op(host, 3)],
            &down,
            100,
            &key,
        )
        .await
        .unwrap();

        // Cap advertised -> the whole packfile op ran: the manifest was persisted and its history
        // was expanded into the store.
        assert!(
            down.last(host, &RecordTag::Packfile)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            down.next(host, &RecordTag::History, 0, 3)
                .await
                .unwrap()
                .len(),
            3
        );
    }

    #[rstest]
    #[tokio::test]
    async fn absent_cap_skips_packfile_but_loose_history_still_syncs(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = MockServer::start().await;
        // Caps advertised, but NO PackfileCap -> get_server::<PackfileCap>() == Ok(None).
        let caps = CapServer::new()
            .add(CapabilitiesCap { version: 1 })
            .unwrap();
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

        sync_remote(
            &client,
            vec![
                packfile_download_op(host, 3),
                Operation::Download {
                    local: None,
                    remote: 3,
                    host,
                    tag: RecordTag::History,
                },
            ],
            &down,
            100,
            &key,
        )
        .await
        .unwrap();

        // Packfile op skipped: manifest not persisted. Loose history still synced (no data loss).
        assert!(
            down.last(host, &RecordTag::Packfile)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            down.next(host, &RecordTag::History, 0, 3)
                .await
                .unwrap()
                .len(),
            3
        );
    }

    #[rstest]
    #[tokio::test]
    async fn not_fetched_skips_packfile_op(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = MockServer::start().await;
        mount_packfile(&server, &manifest, blob).await;

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        sync_remote(
            &client,
            vec![packfile_download_op(host, 3)],
            &down,
            100,
            &key,
        )
        .await
        .unwrap();

        // Skipped: manifest not persisted, and no packfile endpoint was ever hit.
        assert!(
            down.last(host, &RecordTag::Packfile)
                .await
                .unwrap()
                .is_none()
        );
        let requests = server.received_requests().await.unwrap();
        assert!(
            !requests
                .iter()
                .any(|r| r.url.path().starts_with("/api/v0/packfiles")),
            "no /api/v0/packfiles request must be made when packfiles are gated off"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn malformed_cap_skips_packfile_op(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());
        let (manifest, blob) = packed_packfile(host, &key, 3).await;

        let server = MockServer::start().await;
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

        sync_remote(
            &client,
            vec![packfile_download_op(host, 3)],
            &down,
            100,
            &key,
        )
        .await
        .unwrap();

        assert!(
            down.last(host, &RecordTag::Packfile)
                .await
                .unwrap()
                .is_none()
        );
    }
}
