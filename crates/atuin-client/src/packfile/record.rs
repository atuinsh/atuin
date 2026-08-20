//! The record schema for the packfile-tagged object.

use std::num::NonZeroU8;

use atuin_common::encryption::paseto_v4;
use atuin_common::rmp::decode::{self, Bytes, DecodeError, RmpRead};
use atuin_common::rmp::encode::{self, ByteBuf, EncodeError, RmpWrite, TryEncodeError};
use atuin_domain::record::{
    DecryptedData, EncryptedData, Host, HostId, Record, RecordId, RecordIdx, RecordSeriesKey,
    RecordTag, RecordVersion,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::instrument;
use uuid::Uuid;

use crate::record::sqlite_store::SqliteStore;

fn read_uuid<'a, R>(reader: &mut R) -> Result<Uuid, DecodeError<'a, R::Error>>
where
    R: RmpRead,
{
    let uuid_bytes = decode::read_fixed_binary_array(reader)?;
    Ok(Uuid::from_bytes(uuid_bytes))
}

fn write_uuid<W>(writer: &mut W, uuid: Uuid) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
{
    encode::write_binary_array(writer, uuid.into_bytes())
}

fn read_record<'a>(bytes: &mut Bytes<'a>) -> Result<Record<DecryptedData>, DecodeError<'a>> {
    // Do not reorder these field expressions; the evaluation order matters.
    Ok(Record {
        id: RecordId(read_uuid(bytes)?),
        idx: decode::read_u64(bytes)?,
        host: {
            let id = HostId(read_uuid(bytes)?);
            // TODO(ATU-589): Remove the vestigial `Host::_name` serialization.
            let _name = decode::read_string(bytes)?;
            Host::new(id)
        },
        timestamp: decode::read_u64(bytes)?,
        version: decode::with_str(bytes, RecordVersion::from)?,
        tag: decode::with_str(bytes, RecordTag::from)?,
        data: DecryptedData(decode::read_binary_array(bytes).collect::<Result<_, _>>()?),
    })
}

fn write_record<W>(
    writer: &mut W,
    record: &Record<DecryptedData>,
) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
{
    write_uuid(writer, record.id.0)?;
    encode::write_u64(writer, record.idx)?;
    write_uuid(writer, record.host.id.0)?;
    // TODO(ATU-589): Remove the vestigial `Host::_name` serialization.
    encode::write_str(writer, "")?;
    encode::write_u64(writer, record.timestamp)?;
    encode::write_str(writer, record.version.as_str())?;
    encode::write_str(writer, record.tag.as_str())?;
    encode::write_binary_array(writer, record.data.0.iter().copied())?;
    Ok(())
}

fn read_encrypted_data<'a>(bytes: &mut Bytes<'a>) -> Result<EncryptedData, DecodeError<'a>> {
    // Do not reorder these field expressions; the evaluation order matters.
    Ok(EncryptedData {
        raw: decode::read_string(bytes)?,
        cek: decode::read_string(bytes)?,
    })
}

fn write_encrypted_data<W>(
    writer: &mut W,
    data: &EncryptedData,
) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
{
    encode::write_str(writer, &data.raw)?;
    encode::write_str(writer, &data.cek)?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum ParsingError {
    #[error("\"{_0}\" is not a packfile tag")]
    WrongTag(RecordTag),
    #[error("failed to find version bytes.")]
    UnknownVersion,
    #[error("invalid body: {_0}")]
    MalformedBody(Box<dyn std::error::Error + Send + Sync>),
    #[error("packfile manifest range is invalid: {_0}")]
    InvalidRange(#[from] InvalidRangeError),
}

/// Structure encoded within the `data` column of the packfile-encoded records.
#[derive(Debug, Clone)]
pub enum PackManifestData {
    /// Version 1 of the manifest.
    V1(PackManifestDataV1),
}

impl PackManifestData {
    #[instrument(level = "trace", skip_all, fields(id = ?record.id, tag = ?record.tag), err)]
    pub fn parse(record: &Record<EncryptedData>) -> Result<Self, ParsingError> {
        if record.tag != RecordTag::Packfile {
            return Err(ParsingError::WrongTag(record.tag.clone()));
        }

        let data: &String = &record.data.raw;

        // When deserializing, the first three bytes are always reserved to identify the version of
        // the manifest.
        if data.starts_with("001") {
            let body = data.get(3..).ok_or(ParsingError::UnknownVersion)?;
            let body: PackManifestDataV1 =
                serde_json::from_str(body).map_err(|e| ParsingError::MalformedBody(Box::new(e)))?;
            // Untrusted data -- let's validate the count so it doesn't bubble down.
            let _ = body.record_count()?;

            Ok(Self::V1(body))
        } else {
            Err(ParsingError::UnknownVersion)
        }
    }

    /// The half-open range of history indices this manifest covers.
    #[must_use]
    pub const fn range(&self) -> std::ops::Range<RecordIdx> {
        let Self::V1(v1) = self;
        v1.start_idx..v1.end_idx + 1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackManifestDataV1 {
    pub host: HostId,
    pub tag: RecordTag,
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

    #[instrument(level = "trace", skip_all, fields(host = ?self.host, tag = ?self.tag), err)]
    pub fn encode(&self) -> Result<EncryptedData, Box<dyn std::error::Error + Send + Sync>> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"001");
        serde_json::to_writer(&mut buf, self).map_err(Box::new)?;
        let data = String::from_utf8(buf).unwrap();

        Ok(EncryptedData {
            raw: data,
            cek: String::new(),
        })
    }
}

/// Why packing records into a body blob failed. Moved here from the former `codec` module when the
/// blocking pack/unpack codec was folded into [`PackManifestRecordView`].
#[derive(Debug, Error)]
pub enum PackError {
    #[error("failed to decrypt a history record: {0}")]
    Decrypt(eyre::Report),
    #[error("failed to serialize the records: {0}")]
    Serialize(#[from] EncodeError),
    #[error("failed to compress the packfile: {0}")]
    Compress(#[from] std::io::Error),
    #[error("failed to encrypt the packfile: {0}")]
    Encrypt(#[from] paseto_v4::EncryptionError),
    #[error("the packing task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Why unpacking a body blob back into records failed. Moved here from the former `codec` module.
#[derive(Debug, Error)]
pub enum UnpackError {
    #[error("failed to deserialize the packfile: {0}")]
    Deserialize(DecodeError<'static>),
    #[error("failed to authenticate and decrypt the packfile: {0}")]
    Decrypt(#[from] paseto_v4::DecryptionError),
    #[error("failed to decompress the packfile: {0}")]
    Decompress(#[from] std::io::Error),
    #[error("the unpacking task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl From<DecodeError<'_>> for UnpackError {
    fn from(e: DecodeError<'_>) -> Self {
        Self::Deserialize(e.into_static())
    }
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

/// A parsed, validated view of a `packfile` manifest record. The manifest body is decoded once at
/// construction and kept alongside the record, so callers read the range and load the covered
/// history without re-parsing.
pub struct PackManifestRecordView<'a> {
    pub record: &'a Record<EncryptedData>,
    pub manifest: PackManifestData,
}

/// An implicit assertion that matches this [`PackManifestRecordView`].
///
/// Do *not* modify this struct. Just like `atuin_domain::record::AdditionalData`, it gets
/// serialized to JSON during encryption, and we rely on the serialization staying the same across
/// versions. Field order, types, and even names all must stay the same!
#[derive(Debug, Serialize)]
struct PackIA<'a> {
    pub manifest_id: RecordId,
    pub manifest_idx: RecordIdx,
    pub manifest_version: &'a str,
    pub host: HostId,
    pub tag: &'a RecordTag,
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
    const ZSTD_ENCODING_LEVEL: NonZeroU8 = NonZeroU8::new(12).unwrap();

    pub fn new(record: &'a Record<EncryptedData>) -> Result<Self, ParsingError> {
        let manifest = PackManifestData::parse(record)?;
        Ok(Self { record, manifest })
    }

    /// The range of history this manifest covers. Validated when the view was built.
    #[must_use]
    pub const fn range(&self) -> std::ops::Range<RecordIdx> {
        self.manifest.range()
    }

    #[instrument(level = "trace", skip_all, fields(id = ?self.record.id), err)]
    pub async fn load_encrypted_packed_records(
        &self,
        store: &SqliteStore,
    ) -> Result<Vec<Record<EncryptedData>>, RecordLoadingError> {
        let range = self.range();
        let count = range.end - range.start;

        let run = store
            .next(
                &RecordSeriesKey::new(self.record.host.id, RecordTag::History),
                range.start,
                count,
            )
            .await
            .map_err(RecordLoadingError::StoreError)?;

        Ok(run)
    }

    /// Asynchronously pack the records enclosed by this manifest.
    #[instrument(level = "trace", skip_all, fields(id = ?self.record.id), err)]
    pub async fn pack_records(
        &self,
        store: &SqliteStore,
        key: paseto_v4::Key,
    ) -> Result<(Vec<u8>, Vec<RecordId>), PackingError> {
        let encrypted_records = self.load_encrypted_packed_records(store).await?;
        let ia = self.ia().json();

        tokio::task::spawn_blocking(move || {
            // First we need to decrypt the encrypted records. Order of magnitude is about 1000 records.
            let record_ids = encrypted_records.iter().map(|r| r.id);

            let mut buf = ByteBuf::new();
            encode::try_write_array(
                &mut buf,
                encrypted_records.iter().map(|r| r.decrypt(&key)),
                |writer, record| write_record(writer, &record),
            )
            .map_err(|e| match e {
                TryEncodeError::Encode(e) => PackError::Serialize(e),
                TryEncodeError::Inner(e) => PackError::Decrypt(e),
            })?;

            let packed_decrypted = buf.into_vec();
            let compressed = zstd::stream::encode_all(
                packed_decrypted.as_slice(),
                Self::ZSTD_ENCODING_LEVEL.get().into(),
            )?;

            // TODO: One huge PASETO may not be the best choice. @ellie suggests switching to
            // xchacha20-poly1305.
            let encrypted_data = paseto_v4::encrypt_sync(
                &compressed,
                Some(paseto_v4::ImplicitAssertion::from(ia.as_str())),
                &key,
            )?;

            let mut buf = ByteBuf::new();
            write_encrypted_data(&mut buf, &encrypted_data)?;
            let packed_encrypted = buf.into_vec();
            Ok((packed_encrypted, record_ids.collect()))
        })
        .await
        // The child task should never panic -- if it does, we may as well panic ourselves.
        .unwrap()
        .map_err(PackingError::Pack)
    }

    #[instrument(level = "trace", skip_all, fields(id = ?self.record.id), err)]
    pub async fn unpack_records(
        &self,
        packed_bytes: impl AsRef<[u8]> + Send + 'static,
        key: paseto_v4::Key,
    ) -> Result<Vec<Record<EncryptedData>>, UnpackError> {
        let ia = self.ia().json();

        tokio::task::spawn_blocking(move || {
            let mut bytes = Bytes::new(packed_bytes.as_ref());
            let encrypted: paseto_v4::EncryptedData = read_encrypted_data(&mut bytes)?;

            let decrypted = paseto_v4::decrypt_sync(
                &encrypted,
                Some(paseto_v4::ImplicitAssertion::from(ia.as_str())),
                &key,
            )?;

            let decompressed = zstd::stream::decode_all(decrypted.as_slice())?;
            let mut bytes = Bytes::new(decompressed.as_slice());
            let records = decode::read_array(&mut bytes, read_record)
                .map(|result| {
                    result.map(|record| record.map_data(DecryptedData::from).encrypt(&key))
                })
                .collect::<Result<_, _>>()?;
            Ok(records)
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
            tag: &self.record.tag,
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::Host;
    use rstest::{fixture, rstest};
    use uuid::Uuid;

    use super::*;

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

    /// Do *not* modify this test if it fails! It means the serialization of [`PackIA`] changed
    /// which **must not happen**.
    #[rstest]
    #[case(
        PackIA {
            manifest_id: RecordId(Uuid::from_bytes([
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
            ])),
            manifest_idx: 12345678910111213141_u64,
            manifest_version: "  this is the \0\0\0 manifest version\n",
            host: HostId(Uuid::from_bytes([
                10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
            ])),
            tag: &RecordTag::Other("@@ \0 TAG\0".to_owned()),
        },
        r#"{"manifest_id":"01020304-0506-0708-090a-0b0c0d0e0f10","manifest_idx":12345678910111213141,"manifest_version":"  this is the \u0000\u0000\u0000 manifest version\n","host":"0a141e28-323c-4650-5a64-6e78828c96a0","tag":"@@ \u0000 TAG\u0000"}"#
    )]
    fn pack_ia_serialization_is_stable(#[case] value: PackIA, #[case] expected: &str) {
        assert_eq!(serde_json::to_string(&value).unwrap(), expected);
    }
}
