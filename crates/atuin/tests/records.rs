use atuin_common::record::{EncryptedData, Host, HostId, Record};
use atuin_common::utils::uuid_v7;

mod common;

fn test_record(host: HostId, idx: u64, tag: &str) -> Record<EncryptedData> {
    Record {
        id: atuin_common::record::RecordId(uuid_v7()),
        idx,
        host: Host {
            id: host,
            name: String::new(),
        },
        timestamp: time::OffsetDateTime::now_utc().unix_timestamp_nanos() as u64,
        version: "v0".to_string(),
        tag: tag.to_string(),
        data: EncryptedData {
            data: format!("encrypted-data-{idx}"),
            content_encryption_key: "encrypted-cek".to_string(),
        },
    }
}

#[tokio::test]
async fn record_sync() {
    let path = format!("/{}", uuid_v7().as_simple());
    let (address, shutdown, server) = common::start_server(&path).await;

    let username = uuid_v7().as_simple().to_string();
    let password = uuid_v7().as_simple().to_string();
    let client = common::register_inner(&address, &username, &password).await;

    // a fresh user has no records
    let status = client.record_status().await.unwrap();
    assert!(status.hosts.is_empty());

    // -- UPLOAD --

    let host = HostId(uuid_v7());
    let records = vec![
        test_record(host, 0, "history"),
        test_record(host, 1, "history"),
    ];

    client.post_records(&records).await.unwrap();

    // -- STATUS --

    let status = client.record_status().await.unwrap();
    assert_eq!(status.hosts.len(), 1);
    assert_eq!(status.hosts[&host]["history"], 1);

    // -- DOWNLOAD --

    let downloaded = client
        .next_records(host, "history".to_string(), 0, 10)
        .await
        .unwrap();
    assert_eq!(downloaded, records);

    // paging from a later start index
    let downloaded = client
        .next_records(host, "history".to_string(), 1, 10)
        .await
        .unwrap();
    assert_eq!(downloaded, records[1..]);

    // -- STORE DELETION --

    client.delete_store().await.unwrap();

    let status = client.record_status().await.unwrap();
    assert!(status.hosts.is_empty());

    // -- ERROR MAPPING --

    // a wrong current password maps to the friendly message
    let err = client
        .change_password("wrong-password".to_string(), "irrelevant".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "current password is incorrect");

    // -- ACCOUNT DELETION --

    client.delete().await.unwrap();

    // the session no longer works
    assert!(client.me().await.is_err());

    shutdown.send(()).unwrap();
    server.await.unwrap();
}
