//! Codec for record packfiles: `encrypt(zstd(msgpack(records)))`.

use atuin_common::encryption::paseto_v4;
use atuin_domain::record::{DecryptedData, Record, RecordId, RecordIdx};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Serializable stand-in for a decrypted record's payload inside a packfile body.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct PackedData(Vec<u8>);

/// The identity a packfile body's AEAD is bound to: the `(id, idx)` of its manifest record.
///
/// Its JSON serialization (`{manifest_id, manifest_idx}`) *is* the PASETO implicit assertion the
/// body is authenticated against, so a body ciphertext is pinned to exactly one manifest -- the
/// manifest id is a globally-unique v7 UUID. The distinct `manifest_*` field names keep the
/// assertion from ever colliding with a record's own [`atuin_domain::record::AdditionalData`]
/// assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ManifestRef {
    #[serde(rename = "manifest_id")]
    pub id: RecordId,
    #[serde(rename = "manifest_idx")]
    pub idx: RecordIdx,
}

impl ManifestRef {
    /// The JSON the implicit assertion borrows from. Callers bind it to a local before building the
    /// [`paseto_v4::ImplicitAssertion`], which borrows it.
    fn assertion(&self) -> String {
        serde_json::to_string(self).expect("serializing a fixed-shape struct to JSON cannot fail")
    }
}

/// Why packing records into a body blob failed.
#[derive(Debug, Error)]
pub enum PackError {
    #[error("failed to serialize the records: {0}")]
    Serialize(#[from] rmp_serde::encode::Error),
    #[error("failed to compress the bundle: {0}")]
    Compress(#[from] std::io::Error),
    #[error("failed to encrypt the bundle: {0}")]
    Encrypt(#[from] paseto_v4::EncryptionError),
    #[error("the packing task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Why unpacking a body blob back into records failed.
#[derive(Debug, Error)]
pub enum UnpackError {
    #[error("failed to deserialize the bundle: {0}")]
    Deserialize(#[from] rmp_serde::decode::Error),
    #[error("failed to authenticate and decrypt the bundle: {0}")]
    Decrypt(#[from] paseto_v4::DecryptionError),
    #[error("failed to decompress the bundle: {0}")]
    Decompress(#[from] std::io::Error),
    #[error("the unpacking task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Compress and encrypt `records` into a packed body.
///
/// You **must** pass the associated manifest id this data is getting packed into.
pub async fn pack(
    records: Vec<Record<DecryptedData>>,
    mref: ManifestRef,
    key: &paseto_v4::Key,
) -> Result<Vec<u8>, PackError> {
    let key = key.clone();
    tokio::task::spawn_blocking(move || pack_blocking(&records, mref, &key)).await?
}

fn pack_blocking(
    records: &[Record<DecryptedData>],
    mref: ManifestRef,
    key: &paseto_v4::Key,
) -> Result<Vec<u8>, PackError> {
    let packed: Vec<Record<PackedData>> = records
        .iter()
        .map(|r| r.with_data(PackedData(r.data.0.clone())))
        .collect();
    let encoded = rmp_serde::to_vec(&packed)?;
    let compressed = zstd::stream::encode_all(encoded.as_slice(), 3)?;
    let assertion = mref.assertion();
    let encrypted = paseto_v4::encrypt_sync(
        &compressed,
        Some(paseto_v4::ImplicitAssertion::from(assertion.as_str())),
        key,
    )?;
    Ok(rmp_serde::to_vec(&encrypted)?)
}

/// Reverse of [`pack`].
///
/// You **must** pass the associated manifest id this data is getting unpacked from.
pub async fn unpack(
    bytes: Vec<u8>,
    mref: ManifestRef,
    key: &paseto_v4::Key,
) -> Result<Vec<Record<DecryptedData>>, UnpackError> {
    let key = key.clone();
    tokio::task::spawn_blocking(move || unpack_blocking(&bytes, mref, &key)).await?
}

/// Synchronous core of [`unpack`].
fn unpack_blocking(
    bytes: &[u8],
    mref: ManifestRef,
    key: &paseto_v4::Key,
) -> Result<Vec<Record<DecryptedData>>, UnpackError> {
    let encrypted: paseto_v4::EncryptedData = rmp_serde::from_slice(bytes)?;
    let assertion = mref.assertion();
    let decrypted = paseto_v4::decrypt_sync(
        &encrypted,
        Some(paseto_v4::ImplicitAssertion::from(assertion.as_str())),
        key,
    )?;
    let decompressed = zstd::stream::decode_all(decrypted.as_slice())?;
    let packed: Vec<Record<PackedData>> = rmp_serde::from_slice(&decompressed)?;
    Ok(packed
        .iter()
        .map(|r| r.with_data(DecryptedData(r.data.0.clone())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{Host, HostId, RecordId};
    use proptest::prelude::*;
    use rstest::{fixture, rstest};
    use uuid::Uuid;

    #[fixture]
    fn key() -> paseto_v4::Key {
        paseto_v4::Key::from([7u8; 32])
    }

    #[fixture]
    fn manifest_ref() -> ManifestRef {
        ManifestRef {
            id: RecordId(uuid_v7()),
            idx: 42,
        }
    }

    /// A run of `n` identical records (defaults to 3; override per-test with `#[with(N)]`).
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

    fn arb_record() -> impl Strategy<Value = Record<DecryptedData>> {
        (
            any::<u128>(),
            any::<u64>(),
            any::<u128>(),
            any::<u64>(),
            "[a-z0-9]{1,8}",
            "[a-z0-9]{1,8}",
            prop::collection::vec(any::<u8>(), 0..48),
        )
            .prop_map(|(id, idx, host, timestamp, version, tag, data)| Record {
                id: RecordId(Uuid::from_u128(id)),
                idx,
                host: Host::new(HostId(Uuid::from_u128(host))),
                timestamp,
                version,
                tag,
                data: DecryptedData(data),
            })
    }

    fn arb_manifest_ref() -> impl Strategy<Value = ManifestRef> {
        (any::<u128>(), any::<u64>()).prop_map(|(id, idx)| ManifestRef {
            id: RecordId(Uuid::from_u128(id)),
            idx,
        })
    }

    proptest! {
        #[test]
        fn round_trips(
            records in prop::collection::vec(arb_record(), 0..16),
            manifest_ref in arb_manifest_ref(),
            key in proptest::array::uniform32(any::<u8>()).prop_map(paseto_v4::Key::from),
        ) {
            let bytes = pack_blocking(&records, manifest_ref, &key).unwrap();
            let out = unpack_blocking(&bytes, manifest_ref, &key).unwrap();
            prop_assert_eq!(records, out);
        }

        #[test]
        fn wrong_manifest_ref_fails(
            records in prop::collection::vec(arb_record(), 0..16),
            a in arb_manifest_ref(),
            b in arb_manifest_ref(),
            key in proptest::array::uniform32(any::<u8>()).prop_map(paseto_v4::Key::from),
        ) {
            prop_assume!(a != b);
            let bytes = pack_blocking(&records, a, &key).unwrap();
            prop_assert!(unpack_blocking(&bytes, b, &key).is_err());
        }

        #[test]
        fn wrong_key_fails(
            records in prop::collection::vec(arb_record(), 0..16),
            manifest_ref in arb_manifest_ref(),
            key_a in proptest::array::uniform32(any::<u8>()).prop_map(paseto_v4::Key::from),
            key_b in proptest::array::uniform32(any::<u8>()).prop_map(paseto_v4::Key::from),
        ) {
            prop_assume!(key_a != key_b);
            let bytes = pack_blocking(&records, manifest_ref, &key_a).unwrap();
            prop_assert!(unpack_blocking(&bytes, manifest_ref, &key_b).is_err());
        }
    }

    #[rstest]
    fn compresses_repetitive_records(
        #[with(200)] records: Vec<Record<DecryptedData>>,
        manifest_ref: ManifestRef,
        key: paseto_v4::Key,
    ) {
        let mirror: Vec<Record<PackedData>> = records
            .iter()
            .map(|r| r.with_data(PackedData(r.data.0.clone())))
            .collect();
        let raw = rmp_serde::to_vec(&mirror).unwrap().len();
        let packed = pack_blocking(&records, manifest_ref, &key).unwrap().len();
        assert!(packed * 4 < raw, "packed {packed} should be << raw {raw}");
    }

    #[rstest]
    fn record_ciphertext_is_not_a_valid_body(manifest_ref: ManifestRef, key: paseto_v4::Key) {
        // A normal record's ciphertext is bound to that record's `AdditionalData` implicit
        // assertion -- a different assertion from a body's `{manifest_id, manifest_idx}`. Feeding a
        // record ciphertext to the body codec must fail authentication, never silently decode.
        let record = Record::builder()
            .host(Host::new(HostId(uuid_v7())))
            .version("v1".into())
            .tag("history".into())
            .idx(0)
            .data(DecryptedData(b"not a body".to_vec()))
            .build()
            .encrypt(&key);
        let bytes = rmp_serde::to_vec(&record.data).unwrap();
        assert!(unpack_blocking(&bytes, manifest_ref, &key).is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn async_pack_unpack_round_trips(
        records: Vec<Record<DecryptedData>>,
        manifest_ref: ManifestRef,
        key: paseto_v4::Key,
    ) {
        let bytes = pack(records.clone(), manifest_ref, &key).await.unwrap();
        let out = unpack(bytes, manifest_ref, &key).await.unwrap();
        assert_eq!(records, out);
    }
}
