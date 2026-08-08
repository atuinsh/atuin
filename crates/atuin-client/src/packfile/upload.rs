//! Uploads a packed history range: rebuild the blob from a manifest and ship it.
//!
//! [`upload_packed`] takes a `packfile` manifest record, reads the history run it covers,
//! compresses + encrypts it with the pack codec (bound to the manifest's identity), and PUTs
//! the blob to the presigned URL the server hands back. The manifest record itself is authored by
//! the packer and synced separately; this only ships the bytes.

use atuin_common::encryption::paseto_v4;
use atuin_domain::record::{EncryptedData, Record, RecordId};
use futures::future::try_join_all;
use thiserror::Error;

use crate::{api_client::Client, record::sqlite_store::SqliteStore};

use super::record::{LoadingError, PackBodyError, PackManifestRecordView};

#[derive(Debug, Error)]
pub enum UploadError {
    #[error("failed to load the packfile manifest: {0}")]
    PackManifest(#[from] LoadingError),

    #[error(transparent)]
    Body(#[from] PackBodyError),

    #[error("bundle upload failed: {0}")]
    Api(eyre::Report),
}

/// Build and upload the bundle blob for a single `packfile` manifest record.
///
/// The manifest gives both the source range (`start_idx..=end_idx`) and the identity
/// (its own `id`/`idx`/`host`) that the blob's encryption is bound to. The referenced records must
/// already be on the server -- `create_bundle` bundles ids the server already knows -- so this
/// runs after the loose records for the range have synced.
///
/// Takes an already-parsed [`PackManifestRecordView`]; the caller builds it (and classifies a
/// parse failure -- see [`upload_packed_many`]).
///
/// Returns the server's bundle id.
pub(crate) async fn upload_packed(
    view: PackManifestRecordView<'_>,
    store: &SqliteStore,
    key: &paseto_v4::Key,
    client: &Client<'_>,
) -> Result<RecordId, UploadError> {
    // `pack_body` loads the covered history, decrypts it, and packs it on the blocking pool -- so
    // this stays fully async and never serializes with the other bundles in the same
    // `try_join_all` batch. It hands back the ids the bundle covers, which `create_bundle` needs.
    let (blob, ids) = view.pack_body(store, key).await?;

    let (url, bundle_id) = client
        .create_bundle(view.id(), &ids, blob.len())
        .await
        .map_err(UploadError::Api)?;
    client
        .put_packfile(&url, blob)
        .await
        .map_err(UploadError::Api)?;

    Ok(bundle_id)
}

/// Maximum bundle uploads to run concurrently within one batch.
const UPLOAD_CONCURRENCY: usize = 8;

/// Build and upload the bundle blobs for many `packfile` manifest records.
///
/// The manifests are shipped in bounded concurrent batches of [`UPLOAD_CONCURRENCY`]: each bundle
/// is independent (it carries its own history range and manifest identity), so they need no
/// ordering between them. Returns the server bundle ids in input order. The first failure aborts
/// and propagates -- any bundles already shipped are harmless, since a re-run re-ships the range.
pub async fn upload_packed_many(
    manifests: &[Record<EncryptedData>],
    store: &SqliteStore,
    key: &paseto_v4::Key,
    client: &Client<'_>,
) -> Result<Vec<RecordId>, UploadError> {
    let mut bundle_ids = Vec::with_capacity(manifests.len());
    for batch in manifests.chunks(UPLOAD_CONCURRENCY) {
        // Parsing the manifest into a view happens here so a parse failure surfaces as a
        // `PackManifest` error, which `try_join_all` propagates like any other upload failure.
        let ids = try_join_all(batch.iter().map(|manifest| async move {
            let view = PackManifestRecordView::new(manifest)?;
            upload_packed(view, store, key, client).await
        }))
        .await?;
        bundle_ids.extend(ids);
    }
    Ok(bundle_ids)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{DecryptedData, Host, HostId};
    use rstest::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use atuin_common::encryption::paseto_v4;

    use crate::{
        api_client::AuthToken,
        history::HISTORY_TAG,
        packfile::{PACKFILE_TAG, try_pack},
        settings::test_local_timeout,
    };

    /// A single fixed encryption key. The specific bytes are arbitrary in this test.
    #[fixture]
    fn key() -> paseto_v4::Key {
        paseto_v4::Key::from([7u8; 32])
    }

    /// A [`Client`] pointed at a wiremock server, authenticated with a dummy token.
    fn mock_client(addr: &url::Url) -> Client<'_> {
        Client::new(addr, AuthToken::Token("t".into()), 30, 30, &HashMap::new()).unwrap()
    }

    /// A fresh in-memory record store.
    async fn memory_store() -> SqliteStore {
        SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap()
    }

    /// Push a contiguous run of `count` encrypted HISTORY records (idx `0..count`).
    async fn seed_history(store: &SqliteStore, host: HostId, key: &paseto_v4::Key, count: u64) {
        for idx in 0..count {
            let record = Record::builder()
                .host(Host::new(host))
                .version("v1".into())
                .tag(HISTORY_TAG.to_owned())
                .idx(idx)
                .data(DecryptedData(format!("command number {idx}").into_bytes()))
                .build()
                .encrypt(key);
            store.push(&record).await.unwrap();
        }
    }

    #[rstest]
    #[case::far_apart(100, 5)]
    #[case::adjacent(6, 5)]
    #[case::extreme(u64::MAX, 0)]
    fn an_inverted_range_is_rejected_when_building_the_view(
        #[case] start_idx: u64,
        #[case] end_idx: u64,
    ) {
        use crate::packfile::record::{PACKFILE_VERSION, PackManifestDataV1};

        let host = HostId(uuid_v7());

        // A corrupt/tampered manifest whose plaintext range is inverted (start_idx > end_idx). The
        // packer never emits this, so craft the record directly.
        let data = EncryptedData::try_from(&PackManifestDataV1 { start_idx, end_idx }).unwrap();
        let manifest = Record::builder()
            .host(Host::new(host))
            .version(PACKFILE_VERSION.to_owned())
            .tag(PACKFILE_TAG.to_owned())
            .idx(0)
            .data(data)
            .build();

        // The inverted range is rejected when the view is built -- the precondition every
        // `upload_packed` caller must satisfy -- so the count never underflows and no upload runs.
        assert!(
            PackManifestRecordView::new(&manifest).is_err(),
            "inverted manifest range must be rejected, not underflow the count"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn uploads_the_packed_range(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());
        let store = memory_store().await;

        // Seed history, then let the packer author a manifest over it.
        seed_history(&store, host, &key, 5).await;
        try_pack(&store, host, 1..=5, HISTORY_TAG).await.unwrap();
        let manifest = store
            .last(host, PACKFILE_TAG)
            .await
            .unwrap()
            .expect("packer should have written a manifest");

        // Mock the two-step upload: create_bundle -> presigned URL, then the PUT.
        let server = MockServer::start().await;
        let bundle_id = RecordId(uuid_v7());
        Mock::given(method("POST"))
            .and(path("/api/v0/bundles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "upload_url": format!("{}/upload/abc", server.uri()),
                "bundle_id": bundle_id.0.to_string(),
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

        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let view = PackManifestRecordView::new(&manifest).unwrap();
        let got = upload_packed(view, &store, &key, &client).await.unwrap();

        // `.expect(1)` on both mocks verifies create_bundle + put_packfile each fired once.
        assert_eq!(got, bundle_id);
    }
}
