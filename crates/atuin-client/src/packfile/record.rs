//! The record schema for the packfile-tagged object.

use std::num::NonZeroU8;

use atuin_common::encryption::paseto_v4;
use atuin_common::rmp::serde::TryToVecError;
use atuin_domain::record::{
    DecryptedData, EncryptedData, HostId, Record, RecordId, RecordIdx, RecordTag,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::record::sqlite_store::SqliteStore;

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
    WrongTag(RecordTag),
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
        if value.tag != RecordTag::Packfile {
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

/// Why packing records into a body blob failed. Moved here from the former `codec` module when the
/// blocking pack/unpack codec was folded into [`PackManifestRecordView`].
#[derive(Debug, Error)]
pub enum PackError {
    #[error("failed to decrypt a history record: {0}")]
    Decrypt(eyre::Report),
    #[error("failed to serialize the records: {0}")]
    Serialize(#[from] rmp_serde::encode::Error),
    #[error("the record run yielded a different number of records than it reported")]
    BadLength,
    #[error("failed to compress the packfile: {0}")]
    Compress(#[from] std::io::Error),
    #[error("failed to encrypt the packfile: {0}")]
    Encrypt(#[from] paseto_v4::EncryptionError),
    #[error("the packing task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl From<TryToVecError<PackError>> for PackError {
    fn from(value: TryToVecError<PackError>) -> Self {
        match value {
            TryToVecError::Encoding(e) => Self::Serialize(e),
            TryToVecError::Given(e) => e,
            TryToVecError::BadLength => Self::BadLength,
        }
    }
}

/// Why unpacking a body blob back into records failed. Moved here from the former `codec` module.
#[derive(Debug, Error)]
pub enum UnpackError {
    #[error("failed to deserialize the packfile: {0}")]
    Deserialize(#[from] rmp_serde::decode::Error),
    #[error("failed to authenticate and decrypt the packfile: {0}")]
    Decrypt(#[from] paseto_v4::DecryptionError),
    #[error("failed to decompress the packfile: {0}")]
    Decompress(#[from] std::io::Error),
    #[error("the unpacking task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
}

#[derive(Debug, Error)]
pub enum RecordLoadingError {
    #[error("unexpected record store error: {0}")]
    StoreError(eyre::Report),
    #[error("{0}")]
    InvalidRange(#[from] InvalidRangeError),
}

#[derive(Debug, Error)]
pub enum PackingError {
    #[error("failed to load records from the store: {0}")]
    Loading(#[from] RecordLoadingError),
    #[error(transparent)]
    Pack(#[from] PackError),
}

/// Helper structure which is equivalent to DecryptedData, but implements `Serialize`.
///
/// **Careful**: Ensure this never travels over the wire. To prevent this happening in the future,
/// ensure you never directly use this structure and rather you use the explicit conversion functions.
///
/// Furthermore, keep this structure WITHIN this module and **never** leak it as a public interface.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct PackedData(#[serde(with = "serde_bytes")] Vec<u8>);

impl From<DecryptedData> for PackedData {
    fn from(value: DecryptedData) -> Self {
        Self(value.0)
    }
}

impl From<PackedData> for DecryptedData {
    fn from(value: PackedData) -> Self {
        Self(value.0)
    }
}

/// A parsed, validated view of a `packfile` manifest record. The manifest body is decoded once at
/// construction and kept alongside the record, so callers read the range and load the covered
/// history without re-parsing.
pub(crate) struct PackManifestRecordView<'a> {
    pub record: &'a Record<EncryptedData>,
    pub manifest: PackManifestData,
}

/// An implicit assertion that matches this [`PackManifestRecordView`].
#[derive(Debug, Serialize, Deserialize)]
struct PackIA<'a> {
    pub manifest_id: RecordId,
    pub manifest_idx: RecordIdx,
    pub manifest_version: &'a str,
    pub host: HostId,
}

impl PackIA<'_> {
    /// The JSON an [`paseto_v4::ImplicitAssertion`] is built from.
    fn json(&self) -> String {
        serde_json::to_string(self).expect("fixed-layout structure cannot fail serialization")
    }
}

impl<'a> PackManifestRecordView<'a> {
    /// Decided on `12` because that's what Claude's experiments showed to be the good trade-off
    /// between compression size and compression speed and would be optimal for DSL/Fiber networks.
    #[allow(unsafe_code)]
    const ZSTD_ENCODING_LEVEL: Option<NonZeroU8> =
        Some(unsafe { NonZeroU8::new(12).unwrap_unchecked() });

    pub fn new(record: &'a Record<EncryptedData>) -> Result<Self, LoadingError> {
        let manifest = PackManifestData::try_from(record)?;
        Ok(Self { record, manifest })
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
    ) -> Result<std::vec::IntoIter<Record<EncryptedData>>, RecordLoadingError> {
        let range = self.range();
        // The range was validated when the view was built, so this shouldn't fail.
        let count = range.record_count()?;

        let run = store
            .next(
                self.record.host.id,
                &RecordTag::History,
                range.start_idx,
                count,
            )
            .await
            .map_err(RecordLoadingError::StoreError)?;

        Ok(run.into_iter())
    }

    /// Asynchronously pack the records enclosed by this manifest.
    pub async fn pack_records(
        &self,
        store: &SqliteStore,
        key: paseto_v4::Key,
    ) -> Result<Vec<u8>, PackingError> {
        let encrypted_records = self.load_encrypted_packed_records(store).await?;
        let ia = self.ia().json();

        tokio::task::spawn_blocking(move || {
            // First we need to decrypt the encrypted records. Order of magnitude is about 1000 records.
            let decrypted_records = encrypted_records
                .map(|r| r.decrypt(&key).map_err(PackError::Decrypt))
                // We now need to convert this into a [`PackedData`] record, which, you will note,
                // is `Serialize` and `Deserialize` unlike the `DecryptedData`.
                .map(|r| r.map(|r| r.map_data(PackedData::from)));

            let packed = atuin_common::rmp::serde::try_to_vec(decrypted_records)?;
            let compressed = zstd::stream::encode_all(
                packed.as_slice(),
                Self::ZSTD_ENCODING_LEVEL.map_or(0, |r| i32::from(r.get())),
            )?;

            let encrypted_data = paseto_v4::encrypt_sync(
                &compressed,
                Some(paseto_v4::ImplicitAssertion::from(ia.as_str())),
                &key,
            )?;

            Ok::<_, PackError>(rmp_serde::to_vec(&encrypted_data)?)
        })
        .await
        // The child task should never panic -- if it does, we may as well panic ourselves.
        .unwrap()
        .map_err(PackingError::Pack)
    }

    pub async fn unpack_records(
        &self,
        packed_bytes: Vec<u8>,
        key: paseto_v4::Key,
    ) -> Result<impl Iterator<Item = Record<DecryptedData>>, UnpackError> {
        let ia = self.ia().json();

        tokio::task::spawn_blocking(move || {
            let encrypted: paseto_v4::EncryptedData = rmp_serde::from_slice(&packed_bytes)?;
            let decrypted = paseto_v4::decrypt_sync(
                &encrypted,
                Some(paseto_v4::ImplicitAssertion::from(ia.as_str())),
                &key,
            )?;
            let decompressed = zstd::stream::decode_all(decrypted.as_slice())?;
            let record_data: Vec<Record<PackedData>> = rmp_serde::from_slice(&decompressed)?;

            Ok(record_data
                .into_iter()
                .map(|r| r.map_data(DecryptedData::from)))
        })
        .await
        // The child task should never panic -- if it does, we may as well panic ourselves.
        .unwrap()
    }

    /// Grab the implicit assertion corresponding to this manifest.
    fn ia(&self) -> PackIA<'a> {
        PackIA {
            manifest_id: self.record.id,
            manifest_idx: self.record.idx,
            manifest_version: self.record.version.as_str(),
            host: self.record.host.id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::Host;
    use rstest::fixture;

    #[fixture]
    fn key() -> paseto_v4::Key {
        paseto_v4::Key::from([7u8; 32])
    }

    /// A run of `n` identical decrypted records (defaults to 3; override per-test with `#[with(N)]`).
    #[fixture]
    fn records(#[default(3)] n: usize) -> Vec<Record<DecryptedData>> {
        let host = Host::new(HostId(uuid_v7()));
        (0..n)
            .map(|i| {
                Record::builder()
                    .host(host.clone())
                    .version("v1".into())
                    .tag("history".into())
                    .idx(i as u64)
                    .data(DecryptedData(b"ls -la /very/repetitive/path".to_vec()))
                    .build()
            })
            .collect()
    }
}
