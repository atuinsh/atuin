use atuin_client::history::{
    HISTORY_TAG, HISTORY_VERSION,
    store::{HistoryRecord, build_history_repair_replacement},
};
use atuin_client::record::encryption::PASETO_V4;
use atuin_common::{
    record::{Host, HostId, Record, RecordId},
    utils::uuid_v7,
};

mod common;

/// Build an encrypted history record with arbitrary payload, using `key`.
/// Useful in tests to simulate records encrypted with a "wrong" key, so we can
/// exercise the repair path.
fn encrypted_history_record(
    key: &[u8; 32],
    host: HostId,
    idx: u64,
) -> Record<atuin_common::record::EncryptedData> {
    let payload = HistoryRecord::Delete("dummy-history-id".to_string().into())
        .serialize()
        .unwrap();

    let record = Record::builder()
        .id(RecordId(uuid_v7()))
        .idx(idx)
        .host(Host::new(host))
        .tag(HISTORY_TAG.to_string())
        .version(HISTORY_VERSION.to_string())
        .timestamp(0)
        .data(payload)
        .build();

    record.encrypt::<PASETO_V4>(key)
}

/// Repair endpoint must refuse to touch rows owned by another user.
///
/// We register two users, have user A push a record, then have user B attempt
/// to repair that same record ID. Afterwards, user A pulls the record back and
/// checks the encrypted payload is bit-for-bit identical to what they pushed.
#[tokio::test]
async fn repair_endpoint_rejects_cross_user() {
    let path = format!("/{}", uuid_v7().as_simple());
    let (address, shutdown, server) = common::start_server(&path).await;

    let user_a = common::register(&address).await;
    let user_b = common::register(&address).await;

    let key = [0x11; 32];
    let host = HostId(uuid_v7());

    // User A uploads a single record they own.
    let original = encrypted_history_record(&key, host, 0);
    user_a.post_records(&[original.clone()]).await.unwrap();

    // User B tries to repair A's record by posting a replacement that claims
    // the same record id. On the server this is identified by (user_id, client_id)
    // so B's request should affect zero rows.
    let replacement = build_history_repair_replacement(&original, &key).unwrap();
    user_b.repair_records(&[replacement.clone()]).await.unwrap();

    // A pulls their record back. It must be exactly what they pushed.
    let pulled = user_a
        .next_records(host, HISTORY_TAG.to_string(), 0, 10)
        .await
        .unwrap();

    assert_eq!(pulled.len(), 1, "expected A to still see their one record");
    assert_eq!(pulled[0].id, original.id);
    assert_eq!(
        pulled[0].data.data, original.data.data,
        "data column must not have been overwritten by user B"
    );
    assert_eq!(
        pulled[0].data.content_encryption_key, original.data.content_encryption_key,
        "cek column must not have been overwritten by user B"
    );

    shutdown.send(()).unwrap();
    server.await.unwrap();
}

/// Exercise the "subsequent host" path of `atuin store repair`.
///
/// Host A has already repaired the server, so when host B (represented here by
/// the same user from a second perspective) fetches the record by `(host, tag, idx)`,
/// it should come back decryptable with the good key. The command's
/// `resolve_replacement` takes the "server already clean" branch in that case
/// instead of generating a new replacement.
#[tokio::test]
async fn repair_endpoint_second_host_pulls_existing_fix() {
    let path = format!("/{}", uuid_v7().as_simple());
    let (address, shutdown, server) = common::start_server(&path).await;

    let user = common::register(&address).await;

    let bad_key = [0x42; 32];
    let good_key = [0x99; 32];
    let host = HostId(uuid_v7());

    // Host A uploads a bad record, then repairs it.
    let bad = encrypted_history_record(&bad_key, host, 0);
    user.post_records(&[bad.clone()]).await.unwrap();
    let fix = build_history_repair_replacement(&bad, &good_key).unwrap();
    user.repair_records(&[fix.clone()]).await.unwrap();

    // Host B fetches by (host, tag, idx). The repair-flow first asks the server
    // for whatever is at this coordinate; if it decrypts with the current key,
    // no new repair push is needed.
    let on_server = user
        .next_records(host, HISTORY_TAG.to_string(), 0, 1)
        .await
        .unwrap();
    let remote = on_server
        .into_iter()
        .find(|r| r.idx == bad.idx)
        .expect("server should still have the record at idx=0");

    assert_eq!(remote.id, bad.id);
    remote
        .decrypt::<PASETO_V4>(&good_key)
        .expect("host B should find the server's record already decryptable");

    shutdown.send(()).unwrap();
    server.await.unwrap();
}

/// Batch push: user repairs many records in a single POST. Mirrors what
/// `atuin store repair` does internally when the bad-record set is larger
/// than a single page.
#[tokio::test]
async fn repair_endpoint_handles_batch_push() {
    let path = format!("/{}", uuid_v7().as_simple());
    let (address, shutdown, server) = common::start_server(&path).await;

    let user = common::register(&address).await;

    let bad_key = [0x42; 32];
    let good_key = [0x99; 32];
    let host = HostId(uuid_v7());

    // Upload 25 undecryptable records in one chain.
    let bad: Vec<_> = (0..25)
        .map(|i| encrypted_history_record(&bad_key, host, i))
        .collect();
    user.post_records(&bad).await.unwrap();

    // Generate and post repairs in a single batch.
    let replacements: Vec<_> = bad
        .iter()
        .map(|b| build_history_repair_replacement(b, &good_key).unwrap())
        .collect();
    user.repair_records(&replacements).await.unwrap();

    // Every record should now decrypt cleanly with the good key, and the
    // idx chain should still run 0..25 with no gaps.
    let pulled = user
        .next_records(host, HISTORY_TAG.to_string(), 0, 100)
        .await
        .unwrap();
    assert_eq!(pulled.len(), 25);
    for (i, r) in pulled.iter().enumerate() {
        assert_eq!(r.idx, i as u64, "idx chain must be preserved");
        r.clone()
            .decrypt::<PASETO_V4>(&good_key)
            .unwrap_or_else(|_| panic!("record at idx={i} should decrypt"));
    }

    shutdown.send(()).unwrap();
    server.await.unwrap();
}

/// The happy path: a user repairs their own record and the server accepts it.
///
/// This is the per-host repair flow on a simulated "surgery host" — we push a
/// record encrypted with a bad key (simulating the botched-login scenario),
/// generate a repair replacement encrypted with the good key, send it to the
/// server, and verify the server now returns a record that decrypts cleanly.
#[tokio::test]
async fn repair_endpoint_fixes_own_record() {
    let path = format!("/{}", uuid_v7().as_simple());
    let (address, shutdown, server) = common::start_server(&path).await;

    let user = common::register(&address).await;

    let bad_key = [0x42; 32];
    let good_key = [0x99; 32];
    let host = HostId(uuid_v7());

    // Upload an "undecryptable" record (encrypted with bad_key).
    let bad = encrypted_history_record(&bad_key, host, 0);
    user.post_records(&[bad.clone()]).await.unwrap();

    // Sanity: server has the bad version and it does not decrypt with good_key.
    let pulled = user
        .next_records(host, HISTORY_TAG.to_string(), 0, 10)
        .await
        .unwrap();
    assert_eq!(pulled.len(), 1);
    assert!(
        pulled[0].clone().decrypt::<PASETO_V4>(&good_key).is_err(),
        "precondition: server's bad record should not decrypt with good_key"
    );

    // Build and post a replacement encrypted with good_key.
    let replacement = build_history_repair_replacement(&bad, &good_key).unwrap();
    user.repair_records(&[replacement.clone()]).await.unwrap();

    // Pull again - should now decrypt with good_key.
    let pulled = user
        .next_records(host, HISTORY_TAG.to_string(), 0, 10)
        .await
        .unwrap();
    assert_eq!(pulled.len(), 1);
    assert_eq!(pulled[0].id, bad.id);
    assert_eq!(
        pulled[0].idx, bad.idx,
        "repair must not change idx — the whole point is preserving the chain"
    );
    let decrypted = pulled[0]
        .clone()
        .decrypt::<PASETO_V4>(&good_key)
        .expect("repaired record should decrypt with good_key");
    match HistoryRecord::deserialize(&decrypted.data, HISTORY_VERSION).unwrap() {
        HistoryRecord::Delete(_) => {}
        other => panic!("expected Delete variant, got {other:?}"),
    }

    shutdown.send(()).unwrap();
    server.await.unwrap();
}
