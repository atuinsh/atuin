//! Syncs a packed history range in both directions: ship a manifest's blob to the remote, and
//! expand a remote manifest back into the local store.
//!
//! These live on [`Keyed`] rather than [`SyncEngine`](super::SyncEngine) because they encrypt and
//! decrypt with the session key. The lower-level packing machinery stays in [`crate::packfile`];
//! this is only the sync integration (fetch/store/upload through the engine's client and store).

use atuin_domain::record::{EncryptedData, Record, RecordId, RecordSeriesKey, RecordTag};
use thiserror::Error;
use tracing::instrument;

use super::Keyed;
use crate::packfile::record::{PackManifestRecordView, PackingError, ParsingError, UnpackError};

#[derive(Debug, Error)]
pub(super) enum UploadError {
    #[error("failed to load the packfile manifest: {0}")]
    PackManifest(#[from] ParsingError),

    #[error(transparent)]
    Pack(#[from] PackingError),

    #[error("packfile upload failed: {0}")]
    Api(eyre::Report),
}

#[derive(Debug, Error)]
pub(super) enum DownloadError {
    #[error("failed to load the packfile manifest: {0}")]
    PackManifest(#[from] ParsingError),

    #[error("packfile download failed: {0}")]
    Api(eyre::Report),

    #[error(transparent)]
    Unpack(#[from] UnpackError),

    #[error("failed to store the unpacked history: {0}")]
    Store(eyre::Report),
}

impl DownloadError {
    /// Whether repeating the same download operation would definitively fail.
    pub(super) fn is_permanent(&self) -> bool {
        match self {
            Self::PackManifest(_) => true,
            Self::Unpack(_) => true,
            Self::Api(_) | Self::Store(_) => false,
        }
    }
}

impl Keyed<'_> {
    /// Build and upload the packfile blob for a single `packfile` manifest record.
    #[instrument(level = "trace", skip_all, fields(id = ?manifest.id), err)]
    pub(super) async fn upload_packed(
        &self,
        manifest: &Record<EncryptedData>,
    ) -> Result<(), UploadError> {
        let view = PackManifestRecordView::new(manifest)?;

        let (blob, ids) = view.pack_records(&self.engine.store, self.key.clone()).await?;

        self.engine
            .client
            .upload_packfile(view.record.id, &ids, blob)
            .await
            .map_err(UploadError::Api)
    }

    /// Fetch, unpack, and locally store the history covered by a single `packfile` manifest.
    ///
    /// Returns the ids of the history records the manifest's range covers, whether they were just
    /// inserted or were already present locally.
    #[instrument(level = "trace", skip_all, fields(id = ?manifest.id), err)]
    pub(super) async fn download_packed(
        &self,
        manifest: &Record<EncryptedData>,
    ) -> Result<Vec<RecordId>, DownloadError> {
        let view = PackManifestRecordView::new(manifest)?;
        let store = &self.engine.store;

        // Skip if we already have the whole range (history is contiguous, packfiles are prefixes).
        let head = store
            .last(&RecordSeriesKey::new(view.record.host.id, RecordTag::History))
            .await
            .map_err(DownloadError::Store)?;
        if let Some(head) = head
            && head.idx >= view.range().end - 1
        {
            // Range already available locally. Return the IDs.
            let existing = view
                .load_encrypted_packed_records(store)
                .await
                .map_err(|e| DownloadError::Store(e.into()))?;
            return Ok(existing.iter().map(|r| r.id).collect());
        }

        let blob = self
            .engine
            .client
            .download_packfile(view.record.id)
            .await
            .map_err(DownloadError::Api)?;

        let records = view.unpack_records(blob, self.key.clone()).await?;
        let ids: Vec<RecordId> = records.iter().map(|record| record.id).collect();

        store.push_batch(records.iter()).await.map_err(DownloadError::Store)?;

        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use atuin_common::encryption::paseto_v4;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::caps::PackfileCap;
    use atuin_domain::record::{DecryptedData, Host, HostId, Record, RecordVersion};
    use rstest::*;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::api_client::{AuthToken, Client, caps_client};
    use crate::packfile::record::{PackManifestDataV1, PackManifestRecordView};
    use crate::packfile::try_pack;
    use crate::record::sqlite_store::SqliteStore;
    use crate::record::sync::{ClientSource, SyncEngine};
    use crate::settings::test_local_timeout;

    /// A single fixed encryption key. The specific bytes are arbitrary in these tests -- each one
    /// packs and unpacks with the same key -- so one shared value keeps setup uniform.
    #[fixture]
    fn key() -> paseto_v4::Key {
        paseto_v4::Key::from([7u8; 32])
    }

    /// A [`Client`] pointed at a wiremock server, authenticated with a dummy token.
    fn mock_client(addr: &url::Url) -> Client {
        let caps = caps_client(addr, &HashMap::new()).unwrap();
        Client::new(addr.clone(), &AuthToken::Token("t".into()), 30, 30, &HashMap::new(), caps)
            .unwrap()
    }

    /// A fresh in-memory record store.
    async fn memory_store() -> SqliteStore {
        SqliteStore::new(":memory:", test_local_timeout()).await.unwrap()
    }

    /// A [`SyncEngine`] wrapping a prebuilt client and store, for calling the packfile methods.
    async fn build_engine(client: Client, store: SqliteStore) -> SyncEngine {
        SyncEngine::builder()
            .store(store)
            .client_source(ClientSource::FromClient(client))
            .build()
            .connect()
            .await
            .unwrap()
    }

    /// Push a contiguous run of `count` encrypted HISTORY records (idx `0..count`).
    async fn seed_history(store: &SqliteStore, host: HostId, key: &paseto_v4::Key, count: u64) {
        for idx in 0..count {
            let record = Record::builder()
                .host(Host::new(host))
                .version("v1".into())
                .tag(RecordTag::History)
                .idx(idx)
                .data(DecryptedData(format!("command number {idx}").into_bytes()))
                .build()
                .encrypt(key);
            store.push(&record).await.unwrap();
        }
    }

    /// Building the view rejects an inverted plaintext range (`start_idx > end_idx`) regardless of
    /// sync direction -- the precondition every `upload_packed`/`download_packed` caller relies on,
    /// so the covered-record count never underflows and no upload or fetch runs.
    #[rstest]
    #[case::far_apart(100, 5)]
    #[case::adjacent(6, 5)]
    #[case::extreme(u64::MAX, 0)]
    fn an_inverted_range_is_rejected_when_building_the_view(
        #[case] start_idx: u64,
        #[case] end_idx: u64,
    ) {
        let host = HostId(uuid_v7());

        // A corrupt/tampered manifest whose plaintext range is inverted (start_idx > end_idx). The
        // packer never emits this, so craft the record directly.
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
        let store = memory_store().await;

        // Seed history, then let the packer author a manifest over it.
        seed_history(&store, host, &key, 5).await;
        try_pack(
            &store,
            &RecordSeriesKey::new(host, RecordTag::History),
            Some(PackfileCap {
                version: 1,
                record_count: 5,
            }),
        )
        .await
        .unwrap();
        let manifest = store
            .last(&RecordSeriesKey::new(host, RecordTag::Packfile))
            .await
            .unwrap()
            .expect("packer should have written a manifest");

        // Mock the two-step upload: create_packfile -> presigned URL, then the PUT.
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
        // After the PUT, the client confirms the upload so the server verifies the
        // body landed and marks it downloadable (no reliance on the store webhook).
        Mock::given(method("POST"))
            .and(path_regex(r"^/api/v0/packfiles/[^/]+/confirm$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "confirmed",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        // `.expect(1)` on all three mocks verifies create_packfile + put_packfile +
        // confirm_packfile each fired exactly once.
        build_engine(client, store).await.keyed(&key).upload_packed(&manifest).await.unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn download_packed_populates_history_from_the_packfile(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());

        // The UPLOADER's store: seed history, author a manifest, build the blob it would ship.
        let up = memory_store().await;
        seed_history(&up, host, &key, 5).await;
        try_pack(
            &up,
            &RecordSeriesKey::new(host, RecordTag::History),
            Some(PackfileCap {
                version: 1,
                record_count: 5,
            }),
        )
        .await
        .unwrap();
        let manifest =
            up.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().unwrap();
        let (blob, _) = PackManifestRecordView::new(&manifest)
            .unwrap()
            .pack_records(&up, key.clone())
            .await
            .unwrap();

        // Mock the download: manifest id -> download_url -> blob bytes.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v0/packfiles/{}", manifest.id.0)))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "download_url": format!("{}/download/abc", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/abc"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(blob))
            .mount(&server)
            .await;

        // The DOWNLOADER's fresh store.
        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let ids = build_engine(client, down.clone())
            .await
            .keyed(&key)
            .download_packed(&manifest)
            .await
            .unwrap();
        assert_eq!(ids.len(), 5, "all five history records populated");

        // History is present locally and decrypts to the same commands.
        let got = down.next(&RecordSeriesKey::new(host, RecordTag::History), 0, 5).await.unwrap();
        assert_eq!(got.len(), 5);
        let first = got[0].clone().decrypt(&key).unwrap();
        assert_eq!(first.data.0, b"command number 0");
    }

    #[rstest]
    #[tokio::test]
    async fn download_packed_returns_range_ids_when_already_local(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());
        let up = memory_store().await;
        seed_history(&up, host, &key, 5).await;
        try_pack(
            &up,
            &RecordSeriesKey::new(host, RecordTag::History),
            Some(PackfileCap {
                version: 1,
                record_count: 5,
            }),
        )
        .await
        .unwrap();
        let manifest =
            up.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().unwrap();

        // Downloader that already HAS the history the manifest covers.
        let down = memory_store().await;
        seed_history(&down, host, &key, 5).await;
        let expected_ids: Vec<RecordId> = down
            .next(&RecordSeriesKey::new(host, RecordTag::History), 0, 5)
            .await
            .unwrap()
            .iter()
            .map(|record| record.id)
            .collect();
        assert_eq!(expected_ids.len(), 5, "sanity: seeded ids captured");

        // No server needed: the skip must happen before any network call.
        let sync_addr: url::Url = "http://127.0.0.1:1/".parse().unwrap();
        let caps = caps_client(&sync_addr, &HashMap::new()).unwrap();
        let client = Client::new(
            sync_addr.clone(),
            &AuthToken::Token("t".into()),
            1,
            1,
            &HashMap::new(),
            caps,
        )
        .unwrap();

        let ids =
            build_engine(client, down).await.keyed(&key).download_packed(&manifest).await.unwrap();

        // Range already present -> no fetch, but the covered ids are still returned so the
        // id-driven history.db rebuild can re-index them (see download_packed's doc comment).
        assert_eq!(
            ids, expected_ids,
            "range already local -> covered ids returned anyway, for re-indexing"
        );
    }

    /// An unknown-version / malformed / inverted-range manifest fails at
    /// `PackManifestData::try_from` before any network I/O, and reports itself PERMANENT so the
    /// caller skips it rather than failing the tick. A valid manifest still expands afterwards.
    #[rstest]
    #[case::unknown_version(|_| EncryptedData {
        raw: "999{}".into(),
        cek: String::new(),
    })]
    #[case::malformed_body(|_| EncryptedData {
        raw: "001{not json".into(),
        cek: String::new(),
    })]
    #[case::inverted_range(|host| {
        PackManifestDataV1 {
            tag: RecordTag::History,
            host,
            start_idx: 100,
            end_idx: 5,
        }.encode()
        .unwrap()
    })]
    #[tokio::test]
    async fn a_malformed_manifest_fails_permanently_and_does_not_block_a_valid_one(
        key: paseto_v4::Key,
        #[case] bad_data: impl FnOnce(HostId) -> EncryptedData,
    ) {
        let host = HostId(uuid_v7());

        // A VALID packfile for `host` over three history records, built exactly like
        // `download_packed_populates_history_from_the_packfile`.
        let up = memory_store().await;
        seed_history(&up, host, &key, 3).await;
        try_pack(
            &up,
            &RecordSeriesKey::new(host, RecordTag::History),
            Some(PackfileCap {
                version: 1,
                record_count: 3,
            }),
        )
        .await
        .unwrap();
        let good =
            up.last(&RecordSeriesKey::new(host, RecordTag::Packfile)).await.unwrap().unwrap();
        let (blob, _) = PackManifestRecordView::new(&good)
            .unwrap()
            .pack_records(&up, key.clone())
            .await
            .unwrap();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v0/packfiles/{}", good.id.0)))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "download_url": format!("{}/download/abc", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/abc"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(blob))
            .mount(&server)
            .await;

        // A PERMANENT manifest carrying the case's bad body: it fails at `PackManifestData::try_from`
        // (unknown version / malformed JSON / inverted range) before any network I/O, so it needs
        // no mock of its own.
        let bad = Record::builder()
            .host(Host::new(host))
            .version(RecordVersion::V1)
            .tag(RecordTag::Packfile)
            .idx(0)
            .data(bad_data(host))
            .build();

        let down = memory_store().await;
        let addr: url::Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);
        let engine = build_engine(client, down.clone()).await;

        let err = engine
            .keyed(&key)
            .download_packed(&bad)
            .await
            .expect_err("a malformed manifest must not expand");
        assert!(
            err.is_permanent(),
            "the caller must be told to skip this manifest, not fail the tick: {err:?}"
        );

        let ids = engine
            .keyed(&key)
            .download_packed(&good)
            .await
            .expect("the valid manifest still expands");
        assert_eq!(ids.len(), 3, "the valid manifest's three records expand");

        let got = down.next(&RecordSeriesKey::new(host, RecordTag::History), 0, 3).await.unwrap();
        assert_eq!(got.len(), 3, "the valid packfile's history is present locally");
    }

    /// GUARD: a TRANSIENT/systemic fault (here a connection-refused packfile GET) must still
    /// propagate and fail the tick -- it must NOT be masked as a per-manifest skip.
    #[rstest]
    #[tokio::test]
    async fn a_transport_failure_is_not_permanent(key: paseto_v4::Key) {
        let host = HostId(uuid_v7());

        // A VALID manifest that parses to V1 over a non-empty range.
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

        // FRESH store: the range is NOT already local, so download_packed reaches the packfile fetch.
        let down = memory_store().await;

        // A client pointed at a dead address with short timeouts: the packfile GET fails with a
        // transport error (connection refused), which is TRANSIENT and must propagate.
        let sync_addr: url::Url = "http://127.0.0.1:1/".parse().unwrap();
        let caps = caps_client(&sync_addr, &HashMap::new()).unwrap();
        let client = Client::new(
            sync_addr.clone(),
            &AuthToken::Token("t".into()),
            1,
            1,
            &HashMap::new(),
            caps,
        )
        .unwrap();

        let result = build_engine(client, down).await.keyed(&key).download_packed(&manifest).await;
        assert!(
            matches!(result, Err(DownloadError::Api(_))),
            "a transient transport fault must surface as Api: {result:?}"
        );
        assert!(
            !result.unwrap_err().is_permanent(),
            "a transient transport fault must propagate, not be skipped"
        );
    }
}
