// do a sync :O
use std::{cmp::Ordering, fmt::Write};

use eyre::Result;
use thiserror::Error;

use super::store::Store;
use crate::{api_client::Client, settings::Settings};

use atuin_common::record::{Diff, HostId, RecordId, RecordIdx, RecordStatus};
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
}

#[derive(Debug, Eq, PartialEq)]
pub enum Operation {
    // Either upload or download until the states matches the below
    Upload {
        local: RecordIdx,
        remote: Option<RecordIdx>,
        host: HostId,
        tag: String,
    },
    Download {
        local: Option<RecordIdx>,
        remote: RecordIdx,
        host: HostId,
        tag: String,
    },
    Noop {
        host: HostId,
        tag: String,
    },
}

pub async fn diff(
    settings: &Settings,
    store: &impl Store,
) -> Result<(Vec<Diff>, RecordStatus), SyncError> {
    let client = Client::new(
        &settings.sync_address,
        settings
            .session_token()
            .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?
            .as_str(),
        settings.network_connect_timeout,
        settings.network_timeout,
    )
    .map_err(|e| SyncError::OperationalError { msg: e.to_string() })?;

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
    _store: &impl Store,
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
                })
            }
        };

        operations.push(op);
    }

    // sort them - purely so we have a stable testing order, and can rely on
    // same input = same output
    // We can sort by ID so long as we continue to use UUIDv7 or something
    // with the same properties

    operations.sort_by_key(|op| match op {
        Operation::Noop { host, tag } => (0, *host, tag.clone()),

        Operation::Upload { host, tag, .. } => (1, *host, tag.clone()),

        Operation::Download { host, tag, .. } => (2, *host, tag.clone()),
    });

    Ok(operations)
}

async fn sync_upload(
    store: &impl Store,
    client: &Client<'_>,
    host: HostId,
    tag: String,
    local: RecordIdx,
    remote: Option<RecordIdx>,
) -> Result<i64, SyncError> {
    let remote = remote.unwrap_or(0);
    let expected = local - remote;
    let upload_page_size = 100;
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

    // preload with the first entry if remote does not know of this store
    loop {
        let page = store
            .next(host, tag.as_str(), remote + progress, upload_page_size)
            .await
            .map_err(|e| {
                error!("failed to read upload page: {e:?}");

                SyncError::LocalStoreError { msg: e.to_string() }
            })?;

        client.post_records(&page).await.map_err(|e| {
            error!("failed to post records: {e:?}");

            SyncError::RemoteRequestError { msg: e.to_string() }
        })?;

        pb.set_position(progress);
        progress += page.len() as u64;

        if progress >= expected {
            break;
        }
    }

    pb.finish_with_message("Uploaded records");

    Ok(progress as i64)
}

async fn sync_download(
    store: &impl Store,
    client: &Client<'_>,
    host: HostId,
    tag: String,
    local: Option<RecordIdx>,
    remote: RecordIdx,
) -> Result<Vec<RecordId>, SyncError> {
    let local = local.unwrap_or(0);
    let expected = remote - local;
    let download_page_size = 100;
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

    // preload with the first entry if remote does not know of this store
    loop {
        let page = client
            .next_records(host, tag.clone(), local + progress, download_page_size)
            .await
            .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?;

        store
            .push_batch(page.iter())
            .await
            .map_err(|e| SyncError::LocalStoreError { msg: e.to_string() })?;

        ret.extend(page.iter().map(|f| f.id));

        pb.set_position(progress);
        progress += page.len() as u64;

        if progress >= expected {
            break;
        }
    }

    pb.finish_with_message("Downloaded records");

    Ok(ret)
}

pub async fn sync_remote(
    operations: Vec<Operation>,
    local_store: &impl Store,
    settings: &Settings,
) -> Result<(i64, Vec<RecordId>), SyncError> {
    let client = Client::new(
        &settings.sync_address,
        settings
            .session_token()
            .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?
            .as_str(),
        settings.network_connect_timeout,
        settings.network_timeout,
    )
    .expect("failed to create client");

    let mut uploaded = 0;
    let mut downloaded = Vec::new();

    // this can totally run in parallel, but lets get it working first
    for i in operations {
        match i {
            Operation::Upload {
                host,
                tag,
                local,
                remote,
            } => uploaded += sync_upload(local_store, &client, host, tag, local, remote).await?,

            Operation::Download {
                host,
                tag,
                local,
                remote,
            } => {
                let mut d = sync_download(local_store, &client, host, tag, local, remote).await?;
                downloaded.append(&mut d)
            }

            Operation::Noop { .. } => continue,
        }
    }

    Ok((uploaded, downloaded))
}

pub async fn sync(
    settings: &Settings,
    store: &impl Store,
) -> Result<(i64, Vec<RecordId>), SyncError> {
    let (diff, _) = diff(settings, store).await?;
    let operations = operations(diff, store).await?;
    let (uploaded, downloaded) = sync_remote(operations, store, settings).await?;

    Ok((uploaded, downloaded))
}

#[cfg(test)]
mod tests {
    use atuin_common::record::{Diff, EncryptedData, HostId, Record};
    use pretty_assertions::assert_eq;

    use crate::{
        record::{
            encryption::PASETO_V4,
            sqlite_store::SqliteStore,
            store::Store,
            sync::{self, Operation},
        },
        settings::test_local_timeout,
    };

    fn test_record() -> Record<EncryptedData> {
        Record::builder()
            .host(atuin_common::record::Host::new(HostId(
                atuin_common::utils::uuid_v7(),
            )))
            .version("v1".into())
            .tag(atuin_common::utils::uuid_v7().simple().to_string())
            .data(EncryptedData {
                data: String::new(),
                content_encryption_key: String::new(),
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
                tag: record.tag,
                local: record.idx,
                remote: None,
            }
        );
    }

    #[tokio::test]
    async fn build_two_way_diff() {
        // a diff where local is ahead of remote for one, and remote for
        // another. One upload, one download

        let shared_record = test_record();
        let remote_ahead = test_record();

        let local_ahead = shared_record
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);

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
                    tag: local_ahead.tag,
                    local: 1,
                    remote: Some(0),
                },
                // Or in other words, remote knows of a record in an entirely new store (tag)
                Operation::Download {
                    host: remote_ahead.host.id,
                    tag: remote_ahead.tag,
                    local: None,
                    remote: 0,
                },
            ]
        );
    }

    #[tokio::test]
    async fn build_complex_diff() {
        // One shared, ahead but known only by remote
        // One known only by local
        // One known only by remote

        let shared_record = test_record();
        let local_only = test_record();

        let local_only_20 = test_record();
        let local_only_21 = local_only_20
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);
        let local_only_22 = local_only_21
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);
        let local_only_23 = local_only_22
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);

        let remote_only = test_record();

        let remote_only_20 = test_record();
        let remote_only_21 = remote_only_20
            .append(vec![2, 3, 2])
            .encrypt::<PASETO_V4>(&[0; 32]);
        let remote_only_22 = remote_only_21
            .append(vec![2, 3, 2])
            .encrypt::<PASETO_V4>(&[0; 32]);
        let remote_only_23 = remote_only_22
            .append(vec![2, 3, 2])
            .encrypt::<PASETO_V4>(&[0; 32]);
        let remote_only_24 = remote_only_23
            .append(vec![2, 3, 2])
            .encrypt::<PASETO_V4>(&[0; 32]);

        let second_shared = test_record();
        let second_shared_remote_ahead = second_shared
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);
        let second_shared_remote_ahead2 = second_shared_remote_ahead
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);

        let third_shared = test_record();
        let third_shared_local_ahead = third_shared
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);
        let third_shared_local_ahead2 = third_shared_local_ahead
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);

        let fourth_shared = test_record();
        let fourth_shared_remote_ahead = fourth_shared
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);
        let fourth_shared_remote_ahead2 = fourth_shared_remote_ahead
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);

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
                tag: second_shared_remote_ahead.tag,
            },
            // We have a shared record, local knows of the first two but not the last
            Operation::Download {
                local: Some(1),
                remote: 2,
                host: fourth_shared_remote_ahead2.host.id,
                tag: fourth_shared_remote_ahead2.tag,
            },
            // Remote knows of a store with a single record that local does not have
            Operation::Download {
                local: None,
                remote: 0,
                host: remote_only.host.id,
                tag: remote_only.tag,
            },
            // Remote knows of a store with a bunch of records that local does not have
            Operation::Download {
                local: None,
                remote: 4,
                host: remote_only_20.host.id,
                tag: remote_only_20.tag,
            },
            // Local knows of a record in a store that remote does not have
            Operation::Upload {
                local: 0,
                remote: None,
                host: local_only.host.id,
                tag: local_only.tag,
            },
            // Local knows of 4 records in a store that remote does not have
            Operation::Upload {
                local: 3,
                remote: None,
                host: local_only_20.host.id,
                tag: local_only_20.tag,
            },
            // Local knows of 2 more records in a shared store that remote only has one of
            Operation::Upload {
                local: 2,
                remote: Some(0),
                host: third_shared.host.id,
                tag: third_shared.tag,
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
