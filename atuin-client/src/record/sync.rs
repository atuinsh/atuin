// do a sync :O
use eyre::Result;
use thiserror::Error;

use super::store::Store;
use crate::{api_client::Client, settings::Settings};

use atuin_common::record::{Diff, HostId, RecordId, RecordIdx, RecordStatus};

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("the local store is ahead of the remote, but for another host. has remote lost data?")]
    LocalAheadOtherHost,

    #[error("some issue with the local database occured")]
    LocalStoreError,

    #[error("something has gone wrong with the sync logic: {msg:?}")]
    SyncLogicError { msg: String },
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
    store: &mut impl Store,
) -> Result<(Vec<Diff>, RecordStatus)> {
    let client = Client::new(
        &settings.sync_address,
        &settings.session_token,
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    let local_index = store.tail_records().await?;
    let remote_index = client.record_index().await?;

    let diff = local_index.diff(&remote_index);

    Ok((diff, remote_index))
}

// Take a diff, along with a local store, and resolve it into a set of operations.
// With the store as context, we can determine if a tail exists locally or not and therefore if it needs uploading or download.
// In theory this could be done as a part of the diffing stage, but it's easier to reason
// about and test this way
pub async fn operations(diffs: Vec<Diff>, store: &impl Store) -> Result<Vec<Operation>, SyncError> {
    let mut operations = Vec::with_capacity(diffs.len());
    let host = Settings::host_id().expect("got to record sync without a host id; abort");

    for diff in diffs {
        // First, try to fetch the tail
        // If it exists locally, then that means we need to update the remote
        // host until it has the same tail. Ie, upload.
        // If it does not exist locally, that means remote is ahead of us.
        // Therefore, we need to download until our local tail matches
        let last = store
            .last(diff.host, diff.tag.as_str())
            .await
            .map_err(|_| SyncError::LocalStoreError)?;

        let op = match (last, diff.remote) {
            // We both have it! Could be either. Compare.
            (Some(last), Some(remote)) => {
                if last == remote {
                    // between the diff and now, a sync has somehow occured.
                    // regardless, no work is needed!
                    Operation::Noop {
                        host: diff.host,
                        tag: diff.tag,
                    }
                } else if last > remote {
                    Operation::Upload {
                        local: last,
                        remote: Some(remote),
                        host: diff.host,
                        tag: diff.tag,
                    }
                } else {
                    Operation::Download {
                        local: Some(last),
                        remote,
                        host: diff.host,
                        tag: diff.tag,
                    }
                }
            }

            // Remote has it, we don't. Gotta be download
            (None, Some(remote)) => Operation::Download {
                local: None,
                remote,
                host: diff.host,
                tag: diff.tag,
            },

            // We have it, remote doesn't. Gotta be upload.
            (Some(last), None) => Operation::Upload {
                local: last,
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
    store: &mut impl Store,
    client: &Client<'_>,
    host: HostId,
    tag: String,
    local: RecordIdx,
    remote: Option<RecordIdx>,
) -> Result<i64, SyncError> {
    let expected = local - remote.unwrap_or(0);
    let upload_page_size = 100;
    let mut total = 0;

    if expected < 0 {
        return Err(SyncError::SyncLogicError {
            msg: String::from("ran upload, but remote ahead of local"),
        });
    }

    println!(
        "Uploading {} records to {}/{}",
        expected,
        host.0.as_simple().to_string(),
        tag
    );

    // TODO: actually upload lmfao

    Ok(0)
}

async fn sync_download(
    store: &mut impl Store,
    client: &Client<'_>,
    host: HostId,
    tag: String,
    local: Option<RecordIdx>,
    remote: RecordIdx,
) -> Result<i64, SyncError> {
    let expected = remote - local.unwrap_or(0);
    let download_page_size = 100;
    let mut total = 0;

    if expected < 0 {
        return Err(SyncError::SyncLogicError {
            msg: String::from("ran download, but local ahead of remote"),
        });
    }

    println!(
        "Downloading {} records from {}/{}",
        expected,
        host.0.as_simple().to_string(),
        tag
    );

    // TODO: actually upload lmfao

    Ok(0)
}

pub async fn sync_remote(
    operations: Vec<Operation>,
    local_store: &mut impl Store,
    settings: &Settings,
) -> Result<(i64, i64)> {
    let client = Client::new(
        &settings.sync_address,
        &settings.session_token,
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    let mut uploaded = 0;
    let mut downloaded = 0;

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
                downloaded += sync_download(local_store, &client, host, tag, local, remote).await?
            }

            Operation::Noop { .. } => continue,
        }
    }

    Ok((uploaded, downloaded))
}

#[cfg(test)]
mod tests {
    use atuin_common::record::{Diff, EncryptedData, HostId, Record};
    use pretty_assertions::assert_eq;

    use crate::record::{
        encryption::PASETO_V4,
        sqlite_store::SqliteStore,
        store::Store,
        sync::{self, Operation},
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
        let local_store = SqliteStore::new(":memory:")
            .await
            .expect("failed to open in memory sqlite");
        let remote_store = SqliteStore::new(":memory:")
            .await
            .expect("failed to open in memory sqlite"); // "remote"

        for i in local_records {
            local_store.push(&i).await.unwrap();
        }

        for i in remote_records {
            remote_store.push(&i).await.unwrap();
        }

        let local_index = local_store.tail_records().await.unwrap();
        let remote_index = remote_store.tail_records().await.unwrap();

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

        let local = vec![shared_record.clone(), local_ahead.clone()]; // local knows about the already synced, and something newer in the same store
        let remote = vec![shared_record.clone(), remote_ahead.clone()]; // remote knows about the already-synced, and one new record in a new store

        let (store, diff) = build_test_diff(local, remote).await;
        let operations = sync::operations(diff, &store).await.unwrap();

        assert_eq!(operations.len(), 2);

        assert_eq!(
            operations,
            vec![
                Operation::Download {
                    host: remote_ahead.host.id,
                    tag: remote_ahead.tag,
                    local: None,
                    remote: 0,
                },
                Operation::Upload {
                    host: local_ahead.host.id,
                    tag: local_ahead.tag,
                    local: 0,
                    remote: None,
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

        let remote_known = test_record();
        let local_known = test_record();

        let second_shared = test_record();
        let second_shared_remote_ahead = second_shared
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);

        let local_ahead = shared_record
            .append(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);

        let local = vec![
            shared_record.clone(),
            second_shared.clone(),
            local_known.clone(),
            local_ahead.clone(),
        ];

        let remote = vec![
            shared_record.clone(),
            second_shared.clone(),
            second_shared_remote_ahead.clone(),
            remote_known.clone(),
        ]; // remote knows about the already-synced, and one new record in a new store

        let (store, diff) = build_test_diff(local, remote).await;
        let operations = sync::operations(diff, &store).await.unwrap();

        assert_eq!(operations.len(), 4);

        let mut result_ops = vec![
            Operation::Download {
                host: remote_known.host.id,
                tag: remote_known.tag,
                local: Some(second_shared.idx),
                remote: second_shared_remote_ahead.idx,
            },
            Operation::Download {
                host: second_shared.host.id,
                tag: second_shared.tag,
                local: None,
                remote: remote_known.idx,
            },
            Operation::Upload {
                host: local_ahead.host.id,
                tag: local_ahead.tag,
                local: local_ahead.idx,
                remote: Some(shared_record.idx),
            },
            Operation::Upload {
                host: local_known.host.id,
                tag: local_known.tag,
                local: local_known.idx,
                remote: None,
            },
        ];

        result_ops.sort_by_key(|op| match op {
            Operation::Noop { host, tag } => (0, *host, tag.clone()),

            Operation::Upload { host, tag, .. } => (1, *host, tag.clone()),

            Operation::Download { host, tag, .. } => (2, *host, tag.clone()),
        });

        assert_eq!(operations, result_ops);
    }
}
