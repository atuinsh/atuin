// do a sync :O
use eyre::Result;

use super::store::Store;
use crate::{api_client::Client, settings::Settings};

use atuin_common::record::{Diff, HostId, RecordId, RecordIndex};

#[derive(Debug, Eq, PartialEq)]
pub enum Operation {
    // Either upload or download until the tail matches the below
    Upload {
        tail: RecordId,
        host: HostId,
        tag: String,
    },
    Download {
        tail: RecordId,
        host: HostId,
        tag: String,
    },
}

pub async fn diff(settings: &Settings, store: &mut impl Store) -> Result<(Vec<Diff>, RecordIndex)> {
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
pub async fn operations(diffs: Vec<Diff>, store: &impl Store) -> Result<Vec<Operation>> {
    let mut operations = Vec::with_capacity(diffs.len());

    for diff in diffs {
        // First, try to fetch the tail
        // If it exists locally, then that means we need to update the remote
        // host until it has the same tail. Ie, upload.
        // If it does not exist locally, that means remote is ahead of us.
        // Therefore, we need to download until our local tail matches
        let record = store.get(diff.tail).await;

        let op = if record.is_ok() {
            // if local has the ID, then we should find the actual tail of this
            // store, so we know what we need to update the remote to.
            let tail = store
                .tail(diff.host, diff.tag.as_str())
                .await?
                .expect("failed to fetch last record, expected tag/host to exist");

            // TODO(ellie) update the diffing so that it stores the context of the current tail
            // that way, we can determine how much we need to upload.
            // For now just keep uploading until tails match

            Operation::Upload {
                tail: tail.id,
                host: diff.host,
                tag: diff.tag,
            }
        } else {
            Operation::Download {
                tail: diff.tail,
                host: diff.host,
                tag: diff.tag,
            }
        };

        operations.push(op);
    }

    // sort them - purely so we have a stable testing order, and can rely on
    // same input = same output
    // We can sort by ID so long as we continue to use UUIDv7 or something
    // with the same properties

    operations.sort_by_key(|op| match op {
        Operation::Upload { tail, host, .. } => ("upload", *host, *tail),
        Operation::Download { tail, host, .. } => ("download", *host, *tail),
    });

    Ok(operations)
}

async fn sync_upload(
    store: &mut impl Store,
    remote_index: &RecordIndex,
    client: &Client<'_>,
    op: (HostId, String, RecordId),
) -> Result<i64> {
    let upload_page_size = 100;
    let mut total = 0;

    // so. we have an upload operation, with the tail representing the state
    // we want to get the remote to
    let current_tail = remote_index.get(op.0, op.1.clone());

    println!(
        "Syncing local {:?}/{}/{:?}, remote has {:?}",
        op.0, op.1, op.2, current_tail
    );

    let start = if let Some(current_tail) = current_tail {
        current_tail
    } else {
        store
            .head(op.0, op.1.as_str())
            .await
            .expect("failed to fetch host/tag head")
            .expect("host/tag not in current index")
            .id
    };

    debug!("starting push to remote from: {:?}", start);

    // we have the start point for sync. it is either the head of the store if
    // the remote has no data for it, or the tail that the remote has
    // we need to iterate from the remote tail, and keep going until
    // remote tail = current local tail

    let mut record = if current_tail.is_some() {
        let r = store.get(start).await.unwrap();
        store.next(&r).await?
    } else {
        Some(store.get(start).await.unwrap())
    };

    let mut buf = Vec::with_capacity(upload_page_size);

    while let Some(r) = record {
        if buf.len() < upload_page_size {
            buf.push(r.clone());
        } else {
            client.post_records(&buf).await?;

            // can we reset what we have? len = 0 but keep capacity
            buf = Vec::with_capacity(upload_page_size);
        }
        record = store.next(&r).await?;

        total += 1;
    }

    if !buf.is_empty() {
        client.post_records(&buf).await?;
    }

    Ok(total)
}

async fn sync_download(
    store: &mut impl Store,
    remote_index: &RecordIndex,
    client: &Client<'_>,
    op: (HostId, String, RecordId),
) -> Result<i64> {
    // TODO(ellie): implement variable page sizing like on history sync
    let download_page_size = 1000;

    let mut total = 0;

    // We know that the remote is ahead of us, so let's keep downloading until both
    // 1) The remote stops returning full pages
    // 2) The tail equals what we expect
    //
    // If (1) occurs without (2), then something is wrong with our index calculation
    // and we should bail.
    let remote_tail = remote_index
        .get(op.0, op.1.clone())
        .expect("remote index does not contain expected tail during download");
    let local_tail = store.tail(op.0, op.1.as_str()).await?;
    //
    // We expect that the operations diff will represent the desired state
    // In this case, that contains the remote tail.
    assert_eq!(remote_tail, op.2);

    println!("Downloading {:?}/{}/{:?} to local", op.0, op.1, op.2);

    let mut records = client
        .next_records(
            op.0,
            op.1.clone(),
            local_tail.map(|r| r.id),
            download_page_size,
        )
        .await?;

    while !records.is_empty() {
        total += std::cmp::min(download_page_size, records.len() as u64);
        store.push_batch(records.iter()).await?;

        if records.last().unwrap().id == remote_tail {
            break;
        }

        records = client
            .next_records(
                op.0,
                op.1.clone(),
                records.last().map(|r| r.id),
                download_page_size,
            )
            .await?;
    }

    Ok(total as i64)
}

pub async fn sync_remote(
    operations: Vec<Operation>,
    remote_index: &RecordIndex,
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
            Operation::Upload { tail, host, tag } => {
                uploaded +=
                    sync_upload(local_store, remote_index, &client, (host, tag, tail)).await?
            }
            Operation::Download { tail, host, tag } => {
                downloaded +=
                    sync_download(local_store, remote_index, &client, (host, tag, tail)).await?
            }
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
            .host(HostId(atuin_common::utils::uuid_v7()))
            .version("v1".into())
            .tag(atuin_common::utils::uuid_v7().simple().to_string())
            .data(EncryptedData {
                data: String::new(),
                content_encryption_key: String::new(),
            })
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
                host: record.host,
                tag: record.tag,
                tail: record.id
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
            .new_child(vec![1, 2, 3])
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
                    tail: remote_ahead.id,
                    host: remote_ahead.host,
                    tag: remote_ahead.tag,
                },
                Operation::Upload {
                    tail: local_ahead.id,
                    host: local_ahead.host,
                    tag: local_ahead.tag,
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
            .new_child(vec![1, 2, 3])
            .encrypt::<PASETO_V4>(&[0; 32]);

        let local_ahead = shared_record
            .new_child(vec![1, 2, 3])
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
                tail: remote_known.id,
                host: remote_known.host,
                tag: remote_known.tag,
            },
            Operation::Download {
                tail: second_shared_remote_ahead.id,
                host: second_shared.host,
                tag: second_shared.tag,
            },
            Operation::Upload {
                tail: local_ahead.id,
                host: local_ahead.host,
                tag: local_ahead.tag,
            },
            Operation::Upload {
                tail: local_known.id,
                host: local_known.host,
                tag: local_known.tag,
            },
        ];

        result_ops.sort_by_key(|op| match op {
            Operation::Upload { tail, host, .. } => ("upload", *host, *tail),
            Operation::Download { tail, host, .. } => ("download", *host, *tail),
        });

        assert_eq!(operations, result_ops);
    }
}
