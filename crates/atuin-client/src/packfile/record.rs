//! The record schema for the packfile-tagged object.

use atuin_common::encryption::paseto_v4;
use atuin_domain::record::{DecryptedData, EncryptedData, HostId, Record, RecordId, RecordIdx};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::codec::{ManifestRef, PackError, UnpackError, pack, unpack};
use crate::{history::HISTORY_TAG, record::sqlite_store::SqliteStore};

/// Tag under which the packfile manifest records are stored.
pub const PACKFILE_TAG: &str = "packfile";
/// Version string of a packfile manifest record.
pub const PACKFILE_VERSION: &str = "packfile-v1";

/// Structure encoded within the `data` column of the packfile-encoded records.
#[derive(Debug, Clone)]
pub enum PackManifestData {
    /// Version 1 of the manifest.
    V1(PackManifestDataV1),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackManifestDataV1 {
    /// The first record which is encoded within the packfile, ie. **inclusive** lower bound.
    pub start_idx: RecordIdx,
    /// The last record which is encoded within the packfile, ie. **inclusive** upper bound.
    pub end_idx: RecordIdx,
}

#[derive(Debug, Copy, Clone, derive_more::Display, Error)]
#[display("invalid manifest range: {} < {}", start_idx, end_idx)]
pub struct InvalidRangeError {
    pub start_idx: RecordIdx,
    pub end_idx: RecordIdx,
}

impl PackManifestDataV1 {
    /// Number of records this manifest covers over its inclusive `start_idx..=end_idx` range, or
    /// `None` when the range is inverted (`end_idx < start_idx`).
    pub fn record_count(&self) -> Result<u64, InvalidRangeError> {
        self.end_idx
            .checked_sub(self.start_idx)
            .ok_or(InvalidRangeError {
                start_idx: self.start_idx,
                end_idx: self.end_idx,
            })
            .map(|c| c + 1)
    }
}

#[derive(Debug, Error)]
pub enum LoadingError {
    #[error("\"{_0}\" is not a packfile tag")]
    WrongTag(String),
    #[error("failed to find version bytes.")]
    UnknownVersion,
    #[error("invalid body: {_0}")]
    MalformedBody(Box<dyn std::error::Error + Send + Sync>),
    #[error("packfile manifest range is invalid: {_0}")]
    InvalidRange(#[from] InvalidRangeError),
}

impl TryFrom<&Record<EncryptedData>> for PackManifestData {
    type Error = LoadingError;

    fn try_from(value: &Record<EncryptedData>) -> Result<Self, Self::Error> {
        if value.tag != PACKFILE_TAG {
            return Err(LoadingError::WrongTag(value.tag.clone()));
        }

        let data: &String = &value.data.raw;

        // When deserializing, the first three bytes are always reserved to identify the version of
        // the manifest.
        if data.starts_with("001") {
            let body = data.get(3..).ok_or(LoadingError::UnknownVersion)?;
            let body: PackManifestDataV1 =
                serde_json::from_str(body).map_err(|e| LoadingError::MalformedBody(Box::new(e)))?;
            // Untrusted data -- let's validate the count so it doesn't bubble down.
            let _ = body.record_count()?;

            Ok(PackManifestData::V1(body))
        } else {
            Err(LoadingError::UnknownVersion)
        }
    }
}

#[derive(Debug, Error)]
pub enum StoringError {
    #[error("invalid body: {_0}")]
    InvalidBody(Box<dyn std::error::Error + Send + Sync>),
}

impl TryFrom<&PackManifestDataV1> for EncryptedData {
    type Error = StoringError;

    fn try_from(value: &PackManifestDataV1) -> Result<Self, Self::Error> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"001");
        serde_json::to_writer(&mut buf, value)
            .map_err(|e| StoringError::InvalidBody(Box::new(e)))?;
        let data = String::from_utf8(buf).unwrap();

        Ok(Self {
            raw: data,
            cek: String::new(),
        })
    }
}

impl TryFrom<&PackManifestData> for EncryptedData {
    type Error = StoringError;

    fn try_from(value: &PackManifestData) -> Result<Self, Self::Error> {
        match value {
            PackManifestData::V1(v1) => v1.try_into(),
        }
    }
}

/// Why [`PackManifestRecordView::pack_body`] failed. `Pack` is fully typed ([`PackError`]); `Load`
/// and `Decrypt` wrap the shared store/encryption layers, which still report `eyre`.
#[derive(Debug, Error)]
pub enum PackBodyError {
    #[error("failed to read the history range from the store: {0}")]
    Load(eyre::Report),
    #[error("failed to decrypt a history record: {0}")]
    Decrypt(eyre::Report),
    #[error("failed to pack the bundle: {0}")]
    Pack(#[from] PackError),
}

/// A parsed, validated view of a `packfile` manifest record. The manifest body is decoded once at
/// construction and kept alongside the record, so callers read the range and load the covered
/// history without re-parsing.
pub(crate) struct PackManifestRecordView<'a> {
    record: &'a Record<EncryptedData>,
    manifest: PackManifestData,
}

impl<'a> PackManifestRecordView<'a> {
    pub fn new(record: &'a Record<EncryptedData>) -> Result<Self, LoadingError> {
        let manifest = PackManifestData::try_from(record)?;
        Ok(Self { record, manifest })
    }

    #[must_use]
    pub const fn id(&self) -> RecordId {
        self.record.id
    }

    #[must_use]
    pub const fn idx(&self) -> RecordIdx {
        self.record.idx
    }

    #[must_use]
    pub const fn host_id(&self) -> HostId {
        self.record.host.id
    }

    /// The range of history this manifest covers. Validated when the view was built.
    #[must_use]
    pub const fn range(&self) -> &PackManifestDataV1 {
        let PackManifestData::V1(range) = &self.manifest;
        range
    }

    pub async fn load_encrypted_packed_records(
        &self,
        store: &SqliteStore,
    ) -> eyre::Result<impl Iterator<Item = Record<EncryptedData>>> {
        let range = self.range();
        // The range was validated when the view was built, so this cannot fail here.
        let count = range.record_count()?;

        let run = store
            .next(self.host_id(), HISTORY_TAG, range.start_idx, count)
            .await?;

        Ok(run.into_iter())
    }

    pub async fn load_decrypted_records(
        &self,
        store: &SqliteStore,
        key: &paseto_v4::Key,
    ) -> eyre::Result<impl Iterator<Item = Result<Record<DecryptedData>, eyre::Report>>> {
        Ok(self
            .load_encrypted_packed_records(store)
            .await?
            .map(|r| r.decrypt(key)))
    }

    /// The `(id, idx)` identity this manifest's body ciphertext is bound to.
    #[must_use]
    pub const fn manifest_ref(&self) -> ManifestRef {
        ManifestRef {
            id: self.id(),
            idx: self.idx(),
        }
    }

    /// Load the history this manifest covers, decrypt it, and pack it into an uploadable body.
    ///
    /// Returns the body blob and the ids of the records it covers (which the caller registers with
    /// the server alongside the bundle). Keeps the whole load -> decrypt -> pack pipeline in one
    /// place so callers pass only the view, the store, and the key.
    pub(crate) async fn pack_body(
        &self,
        store: &SqliteStore,
        key: &paseto_v4::Key,
    ) -> Result<(Vec<u8>, Vec<RecordId>), PackBodyError> {
        let decrypted: Vec<Record<DecryptedData>> = self
            .load_decrypted_records(store, key)
            .await
            .map_err(PackBodyError::Load)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(PackBodyError::Decrypt)?;
        let ids = decrypted.iter().map(|record| record.id).collect();
        let blob = pack(decrypted, self.manifest_ref(), key).await?;
        Ok((blob, ids))
    }

    /// Reverse of [`Self::pack_body`]: decode a fetched body blob into its decrypted records,
    /// authenticated against this manifest's identity.
    pub(crate) async fn unpack_body(
        &self,
        blob: Vec<u8>,
        key: &paseto_v4::Key,
    ) -> Result<Vec<Record<DecryptedData>>, UnpackError> {
        unpack(blob, self.manifest_ref(), key).await
    }
}
