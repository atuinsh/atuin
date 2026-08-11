use std::collections::HashMap;

pub use atuin_common::encryption::paseto_v4::{self, EncryptedData};
use eyre::WrapErr;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;
use uuid::Uuid;

mod tag;
pub use tag::RecordTag;

#[derive(Clone, Debug, PartialEq, derive_more::Deref, derive_more::From)]
pub struct DecryptedData(pub Vec<u8>);

#[derive(Debug, PartialEq, PartialOrd, Ord, Eq)]
pub struct Diff {
    pub host: HostId,
    pub tag: RecordTag,
    pub local: Option<RecordIdx>,
    pub remote: Option<RecordIdx>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Host {
    pub id: HostId,
    pub name: String,
}

impl Host {
    pub fn new(id: HostId) -> Self {
        Host {
            id,
            name: String::new(),
        }
    }
}

/// Note that the order of items matters here -- `serde` serializes in order.
#[derive(Debug, Copy, Clone, Serialize)]
pub struct AdditionalData<'a> {
    pub id: RecordId,
    pub idx: u64,
    pub version: &'a str,
    pub tag: &'a str,
    pub host: HostId,
}

impl<'a, Data> From<&'a Record<Data>> for AdditionalData<'a> {
    fn from(value: &'a Record<Data>) -> Self {
        Self {
            id: value.id,
            idx: value.idx,
            host: value.host.id,
            tag: value.tag.as_ref(),
            version: &value.version,
        }
    }
}

new_uuid!(RecordId);
new_uuid!(HostId);

pub type RecordIdx = u64;

/// A single record stored inside of our local database
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TypedBuilder)]
pub struct Record<Data> {
    /// a unique ID
    #[builder(default = RecordId(Uuid::now_v7()))]
    pub id: RecordId,

    /// The integer record ID. This is only unique per (host, tag).
    pub idx: RecordIdx,

    /// The unique ID of the host.
    // TODO(ellie): Optimize the storage here. We use a bunch of IDs, and currently store
    // as strings. I would rather avoid normalization, so store as UUID binary instead of
    // encoding to a string and wasting much more storage.
    pub host: Host,

    /// The creation time in nanoseconds since unix epoch
    #[builder(default = time::OffsetDateTime::now_utc().unix_timestamp_nanos() as u64)]
    pub timestamp: u64,

    /// The version the data in the entry conforms to
    // However we want to track versions for this tag, eg v2
    pub version: String,

    /// The type of data we are storing here. Eg, RecordTag::History
    pub tag: RecordTag,

    /// Some data. This can be anything you wish to store. Use the tag field to know how to handle it.
    pub data: Data,
}

impl<Data> Record<Data> {
    pub fn append(&self, data: Vec<u8>) -> Record<DecryptedData> {
        Record::builder()
            .host(self.host.clone())
            .version(self.version.clone())
            .idx(self.idx + 1)
            .tag(self.tag.clone())
            .data(DecryptedData(data))
            .build()
    }

    pub fn with_data<New>(self, data: New) -> Record<New> {
        Record {
            id: self.id,
            idx: self.idx,
            host: self.host,
            timestamp: self.timestamp,
            version: self.version,
            tag: self.tag,
            data,
        }
    }

    pub fn with_data_clone<New>(&self, data: New) -> Record<New> {
        Record {
            id: self.id,
            idx: self.idx,
            host: self.host.clone(),
            timestamp: self.timestamp,
            version: self.version.clone(),
            tag: self.tag.clone(),
            data,
        }
    }
}

impl Record<DecryptedData> {
    pub fn encrypt(&self, key: &paseto_v4::Key) -> Record<paseto_v4::EncryptedData> {
        let ad = serde_json::to_string(&AdditionalData::from(self))
            .expect("could not serialize implicit assertions");
        let assertion = paseto_v4::ImplicitAssertion::from(ad.as_str());
        self.with_data_clone(paseto_v4::encrypt_sync(&self.data, Some(assertion), key).unwrap())
    }
}

impl Record<paseto_v4::EncryptedData> {
    pub fn decrypt(&self, key: &paseto_v4::Key) -> eyre::Result<Record<DecryptedData>> {
        let ad = serde_json::to_string(&AdditionalData::from(self))
            .expect("could not serialize implicit assertions");
        let assertion = paseto_v4::ImplicitAssertion::from(ad.as_str());
        let data = paseto_v4::decrypt_sync(&self.data, Some(assertion), key)
            .context("could not decrypt entry")?;
        Ok(self.with_data_clone(data.into()))
    }
}

/// An index representing the current state of the record stores
/// This can be both remote, or local, and compared in either direction
#[derive(Debug, Serialize, Deserialize)]
pub struct RecordStatus {
    // A map of host -> tag -> max(idx)
    pub hosts: HashMap<HostId, HashMap<RecordTag, RecordIdx>>,
}

impl Default for RecordStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl Extend<(HostId, RecordTag, RecordIdx)> for RecordStatus {
    fn extend<T: IntoIterator<Item = (HostId, RecordTag, RecordIdx)>>(&mut self, iter: T) {
        for (host, tag, tail_idx) in iter {
            self.set_raw(host, tag, tail_idx);
        }
    }
}

impl RecordStatus {
    pub fn new() -> RecordStatus {
        RecordStatus {
            hosts: HashMap::new(),
        }
    }

    /// Insert a new tail record into the store
    pub fn set(&mut self, tail: Record<DecryptedData>) {
        self.set_raw(tail.host.id, tail.tag, tail.idx)
    }

    pub fn set_raw(&mut self, host: HostId, tag: RecordTag, tail_id: RecordIdx) {
        self.hosts.entry(host).or_default().insert(tag, tail_id);
    }

    pub fn get(&self, host: HostId, tag: &RecordTag) -> Option<RecordIdx> {
        self.hosts.get(&host).and_then(|v| v.get(tag)).cloned()
    }

    /// Diff this index with another, likely remote index.
    /// The two diffs can then be reconciled, and the optimal change set calculated
    /// Returns a tuple, with (host, tag, Option(OTHER))
    /// OTHER is set to the value of the idx on the other machine. If it is greater than our index,
    /// then we need to do some downloading. If it is smaller, then we need to do some uploading
    /// Note that we cannot upload if we are not the owner of the record store - hosts can only
    /// write to their own store.
    pub fn diff(&self, other: &Self) -> Vec<Diff> {
        let mut ret = Vec::new();

        // First, we check if other has everything that self has
        for (host, tag_map) in self.hosts.iter() {
            for (tag, idx) in tag_map.iter() {
                match other.get(*host, tag) {
                    // The other store is all up to date! No diff.
                    Some(t) if t.eq(idx) => continue,

                    // The other store does exist, and it is either ahead or behind us. A diff regardless
                    Some(t) => ret.push(Diff {
                        host: *host,
                        tag: tag.clone(),
                        local: Some(*idx),
                        remote: Some(t),
                    }),

                    // The other store does not exist :O
                    None => ret.push(Diff {
                        host: *host,
                        tag: tag.clone(),
                        local: Some(*idx),
                        remote: None,
                    }),
                };
            }
        }

        // At this point, there is a single case we have not yet considered.
        // If the other store knows of a tag that we are not yet aware of, then the diff will be missed

        // account for that!
        for (host, tag_map) in other.hosts.iter() {
            for (tag, idx) in tag_map.iter() {
                match self.get(*host, tag) {
                    // If we have this host/tag combo, the comparison and diff will have already happened above
                    Some(_) => continue,

                    None => ret.push(Diff {
                        host: *host,
                        tag: tag.clone(),
                        remote: Some(*idx),
                        local: None,
                    }),
                };
            }
        }

        // Stability is a nice property to have
        ret.sort();
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atuin_common::encryption::paseto_v4;
    use atuin_common::utils::uuid_v7;

    use super::{DecryptedData, Diff, Record, RecordStatus};
    use pretty_assertions::assert_eq;
    use rstest::{fixture, rstest};
    use uuid::Uuid;

    use paseto_v4::ImplicitAssertion;

    /// Serialize `AdditionalData` into the JSON used as the paseto implicit assertion.
    ///
    /// The returned `String` must outlive any `ImplicitAssertion` borrowed from it, so callers
    /// bind it to a local before constructing the assertion.
    fn assertion(ad: &AdditionalData) -> String {
        serde_json::to_string(ad).expect("failed to serialize additional data")
    }

    #[fixture]
    fn key() -> paseto_v4::Key {
        paseto_v4::Key::new_os_random()
    }

    #[fixture]
    fn data() -> DecryptedData {
        DecryptedData(vec![1, 2, 3, 4])
    }

    #[fixture]
    fn ad_parts() -> (RecordId, HostId) {
        (RecordId(uuid_v7()), HostId(uuid_v7()))
    }

    #[fixture]
    fn test_record() -> Record<DecryptedData> {
        Record::builder()
            .host(Host::new(HostId(Uuid::now_v7())))
            .version("v1".into())
            .tag(RecordTag::Other(Uuid::now_v7().simple().to_string()))
            .data(DecryptedData(vec![0, 1, 2, 3]))
            .idx(0)
            .build()
    }

    #[test]
    fn additional_data_assertion_is_stable() {
        // The JSON below is the PASETO v4 implicit assertion, byte-for-byte. It is part of the
        // on-disk and on-wire format: serde emits fields in declaration order, so the field order
        // of `AdditionalData` (`id, idx, version, tag, host`) is frozen by this assertion. If this
        // string ever changes, every record encrypted by a prior atuin release fails to decrypt.
        let ad = AdditionalData {
            id: RecordId(Uuid::from_u128(1)),
            idx: 7,
            version: "v1",
            tag: "history",
            host: HostId(Uuid::from_u128(2)),
        };
        assert_eq!(
            serde_json::to_string(&ad).unwrap(),
            r#"{"id":"00000000-0000-0000-0000-000000000001","idx":7,"version":"v1","tag":"history","host":"00000000-0000-0000-0000-000000000002"}"#
        );
    }

    #[test]
    fn decrypts_frozen_record_blob() {
        // A record produced by atuin's envelope encryption (PASETO v4 payload + PIE-wrapped CEK),
        // frozen here as a golden fixture. Any change to the encryption format, the implicit
        // assertion bytes, or the `EncryptedData` wire field names shows up as a decryption
        // failure of this blob rather than as silent data loss for existing users. Regenerate this
        // ONLY when the format is *intentionally* migrated.
        let key = paseto_v4::Key::from([0x55u8; 32]);
        let record = Record {
            id: RecordId(Uuid::from_u128(1)),
            idx: 7,
            host: Host::new(HostId(Uuid::from_u128(2))),
            timestamp: 1_687_244_806_000_000,
            version: "v1".to_owned(),
            tag: RecordTag::History,
            data: paseto_v4::EncryptedData {
                raw: "v4.local.cSFhI9n30MfwkZrRAt-YAoxp6DrAMMybmLury7svdFMkapmxQmLQaRzqCfIdanPaQ55VbJjGjqwjst2AnLiBQE9cAQAyH69u2HVHrkaKv7rGtQ".to_owned(),
                cek: r#"{"wpk":"k4.local-wrap.pie.8xXPgrNyliEUy_PnbM3S88Yk8tQQA0HN2o6jyUkGHK5duUEfW-zSCI1kSYRpyPESCK7-5822hPzRAbyZXPRVAbCkoLqqwPJ8_oi8clKEr6u8nJuIQQLHVClvYJmZyZIu","kid":"k4.lid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR0"}"#.to_owned(),
            },
        };

        let decrypted = record
            .decrypt(&key)
            .expect("frozen record blob must still decrypt");
        assert_eq!(decrypted.data.0, [1, 2, 3, 4, 5]);
    }

    #[rstest]
    fn record_index(#[from(test_record)] record: Record<DecryptedData>) {
        let mut index = RecordStatus::new();

        index.set(record.clone());

        let tail = index.get(record.host.id, &record.tag);

        assert_eq!(
            record.idx,
            tail.expect("tail not in store"),
            "tail in store did not match"
        );
    }

    #[rstest]
    fn record_index_overwrite(#[from(test_record)] record: Record<DecryptedData>) {
        let mut index = RecordStatus::new();
        let child = record.append(vec![1, 2, 3]);

        index.set(record.clone());
        index.set(child.clone());

        let tail = index.get(record.host.id, &record.tag);

        assert_eq!(
            child.idx,
            tail.expect("tail not in store"),
            "tail in store did not match"
        );
    }

    #[rstest]
    fn record_index_no_diff(#[from(test_record)] record1: Record<DecryptedData>) {
        // Here, they both have the same version and should have no diff

        let mut index1 = RecordStatus::new();
        let mut index2 = RecordStatus::new();

        index1.set(record1.clone());
        index2.set(record1);

        let diff = index1.diff(&index2);

        assert_eq!(0, diff.len(), "expected empty diff");
    }

    #[rstest]
    fn record_index_single_diff(#[from(test_record)] record1: Record<DecryptedData>) {
        // Here, they both have the same stores, but one is ahead by a single record

        let mut index1 = RecordStatus::new();
        let mut index2 = RecordStatus::new();

        let record2 = record1.append(vec![1, 2, 3]);

        index1.set(record1);
        index2.set(record2.clone());

        let diff = index1.diff(&index2);

        assert_eq!(1, diff.len(), "expected single diff");
        assert_eq!(
            diff[0],
            Diff {
                host: record2.host.id,
                tag: record2.tag,
                remote: Some(1),
                local: Some(0)
            }
        );
    }

    #[rstest]
    fn record_index_multi_diff() {
        // A much more complex case, with a bunch more checks
        let mut index1 = RecordStatus::new();
        let mut index2 = RecordStatus::new();

        let store1record1 = test_record();
        let store1record2 = store1record1.append(vec![1, 2, 3]);

        let store2record1 = test_record();
        let store2record2 = store2record1.append(vec![1, 2, 3]);

        let store3record1 = test_record();

        let store4record1 = test_record();

        // index1 only knows about the first two entries of the first two stores
        index1.set(store1record1);
        index1.set(store2record1);

        // index2 is fully up to date with the first two stores, and knows of a third
        index2.set(store1record2);
        index2.set(store2record2);
        index2.set(store3record1);

        // index1 knows of a 4th store
        index1.set(store4record1);

        let diff1 = index1.diff(&index2);
        let diff2 = index2.diff(&index1);

        // both diffs the same length
        assert_eq!(4, diff1.len());
        assert_eq!(4, diff2.len());

        dbg!(&diff1, &diff2);

        // both diffs should be ALMOST the same. They will agree on which hosts and tags
        // require updating, but the "other" value will not be the same.
        let smol_diff_1: Vec<(HostId, RecordTag)> =
            diff1.iter().map(|v| (v.host, v.tag.clone())).collect();
        let smol_diff_2: Vec<(HostId, RecordTag)> =
            diff1.iter().map(|v| (v.host, v.tag.clone())).collect();

        assert_eq!(smol_diff_1, smol_diff_2);

        // diffing with yourself = no diff
        assert_eq!(index1.diff(&index1).len(), 0);
        assert_eq!(index2.diff(&index2).len(), 0);
    }

    #[rstest]
    fn round_trip(key: paseto_v4::Key, data: DecryptedData, ad_parts: (RecordId, HostId)) {
        let (rid, hid) = ad_parts;
        let ad = AdditionalData {
            id: rid,
            version: "v0",
            tag: "kv",
            host: hid,
            idx: 0,
        };
        let aj = assertion(&ad);

        let encrypted =
            paseto_v4::encrypt_sync(&data.0, Some(ImplicitAssertion::from(aj.as_str())), &key)
                .unwrap();
        let decrypted =
            paseto_v4::decrypt_sync(&encrypted, Some(ImplicitAssertion::from(aj.as_str())), &key)
                .unwrap();
        assert_eq!(DecryptedData(decrypted), data);
    }

    #[rstest]
    fn same_entry_different_output(
        key: paseto_v4::Key,
        data: DecryptedData,
        ad_parts: (RecordId, HostId),
    ) {
        let (rid, hid) = ad_parts;
        let ad = AdditionalData {
            id: rid,
            version: "v0",
            tag: "kv",
            host: hid,
            idx: 0,
        };
        let aj = assertion(&ad);

        let encrypted =
            paseto_v4::encrypt_sync(&data.0, Some(ImplicitAssertion::from(aj.as_str())), &key)
                .unwrap();
        let encrypted2 =
            paseto_v4::encrypt_sync(&data.0, Some(ImplicitAssertion::from(aj.as_str())), &key)
                .unwrap();

        assert_ne!(
            encrypted.raw, encrypted2.raw,
            "re-encrypting the same contents should have different output due to key randomization"
        );
    }

    #[rstest]
    fn cannot_decrypt_different_key(
        key: paseto_v4::Key,
        data: DecryptedData,
        ad_parts: (RecordId, HostId),
    ) {
        let fake_key = paseto_v4::Key::new_os_random();

        let (rid, hid) = ad_parts;
        let ad = AdditionalData {
            id: rid,
            version: "v0",
            tag: "kv",
            host: hid,
            idx: 0,
        };
        let aj = assertion(&ad);

        let encrypted =
            paseto_v4::encrypt_sync(&data.0, Some(ImplicitAssertion::from(aj.as_str())), &key)
                .unwrap();
        let error = paseto_v4::decrypt_sync(
            &encrypted,
            Some(ImplicitAssertion::from(aj.as_str())),
            &fake_key,
        )
        .unwrap_err();
        let message = error.to_string();

        assert!(
            message.contains("bad key"),
            "unexpected error message: {message}"
        );
        assert!(
            message.contains(&format!(
                "encrypted key id: {}, given decryption key: {}",
                key.key_id(),
                fake_key.key_id()
            )),
            "unexpected error message: {message}"
        );
    }

    #[rstest]
    fn cannot_decrypt_cek_with_missing_footer_contents() {
        let key = paseto_v4::Key::new_os_random();

        // A CEK JSON that is valid JSON but missing the required `wpk`/`kid` fields. Routed
        // through the public `decrypt_sync`, which decrypts the CEK before it touches `raw`.
        let encrypted = paseto_v4::EncryptedData {
            raw: String::new(),
            cek: "{}".to_owned(),
        };

        let error =
            paseto_v4::decrypt_sync(&encrypted, None::<ImplicitAssertion>, &key).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to deserialize the given key"),
            "unexpected error message: {error}"
        );
    }

    #[rstest]
    fn cannot_decrypt_different_id(
        key: paseto_v4::Key,
        data: DecryptedData,
        ad_parts: (RecordId, HostId),
    ) {
        let (rid, hid) = ad_parts;
        let ad = AdditionalData {
            id: rid,
            version: "v0",
            tag: "kv",
            host: hid,
            idx: 0,
        };
        let aj = assertion(&ad);

        let encrypted =
            paseto_v4::encrypt_sync(&data.0, Some(ImplicitAssertion::from(aj.as_str())), &key)
                .unwrap();

        let ad = AdditionalData {
            id: RecordId(uuid_v7()),
            ..ad
        };
        let aj = assertion(&ad);
        let _ =
            paseto_v4::decrypt_sync(&encrypted, Some(ImplicitAssertion::from(aj.as_str())), &key)
                .unwrap_err();
    }

    #[rstest]
    fn re_encrypt_round_trip(
        key: paseto_v4::Key,
        data: DecryptedData,
        ad_parts: (RecordId, HostId),
    ) {
        let key1 = key;
        let key2 = paseto_v4::Key::new_os_random();

        let (rid, hid) = ad_parts;
        let ad = AdditionalData {
            id: rid,
            version: "v0",
            tag: "kv",
            host: hid,
            idx: 0,
        };
        let aj = assertion(&ad);

        let encrypted1 =
            paseto_v4::encrypt_sync(&data.0, Some(ImplicitAssertion::from(aj.as_str())), &key1)
                .unwrap();
        let encrypted2 = paseto_v4::reencrypt_sync(&encrypted1, &key1, &key2).unwrap();

        // we only re-encrypt the content keys
        assert_eq!(encrypted1.raw, encrypted2.raw);
        assert_ne!(encrypted1.cek, encrypted2.cek);

        let decrypted = paseto_v4::decrypt_sync(
            &encrypted2,
            Some(ImplicitAssertion::from(aj.as_str())),
            &key2,
        )
        .unwrap();

        assert_eq!(DecryptedData(decrypted), data);
    }

    #[rstest]
    fn full_record_round_trip(test_record: Record<DecryptedData>) {
        let key = paseto_v4::Key::from([0x55; 32]);
        let encrypted = test_record.encrypt(&key);

        assert!(!encrypted.data.raw.is_empty());
        assert!(!encrypted.data.cek.is_empty());

        let decrypted = encrypted.decrypt(&key).unwrap();

        assert_eq!(decrypted.data.0, [0, 1, 2, 3]);
    }

    #[rstest]
    fn full_record_round_trip_fail(test_record: Record<DecryptedData>) {
        let key = paseto_v4::Key::from([0x55; 32]);
        let encrypted = test_record.encrypt(&key);

        let mut enc1 = encrypted.clone();
        enc1.host = Host::new(HostId(uuid_v7()));
        let _ = enc1
            .decrypt(&key)
            .expect_err("tampering with the host should result in auth failure");

        let mut enc2 = encrypted;
        enc2.id = RecordId(uuid_v7());
        let _ = enc2
            .decrypt(&key)
            .expect_err("tampering with the id should result in auth failure");
    }

    #[test]
    fn record_status_serializes_with_bare_string_tag_keys() {
        let host = HostId(Uuid::from_u128(0xabc));
        let mut status = RecordStatus::new();
        status.set_raw(host, RecordTag::History, 6);
        status.set_raw(host, RecordTag::Other("custom".to_owned()), 2);

        let json = serde_json::to_string(&status).unwrap();
        // Tag keys are bare strings, identical to the pre-refactor String-keyed shape.
        assert!(json.contains(r#""history":6"#), "got {json}");
        assert!(json.contains(r#""custom":2"#), "got {json}");

        let round: RecordStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(round.get(host, &RecordTag::History), Some(6));
        assert_eq!(
            round.get(host, &RecordTag::Other("custom".to_owned())),
            Some(2)
        );
    }
}
