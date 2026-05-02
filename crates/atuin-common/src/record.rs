use std::collections::HashMap;

use eyre::Result;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct DecryptedData(pub Vec<u8>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncryptedData {
    pub data: String,
    pub content_encryption_key: String,
}

#[derive(Debug, PartialEq, PartialOrd, Ord, Eq)]
pub struct Diff {
    pub host: HostId,
    pub tag: String,
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

new_uuid!(RecordId);
new_uuid!(HostId);

pub type RecordIdx = u64;

/// A single record stored inside of our local database
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TypedBuilder)]
pub struct Record<Data> {
    /// a unique ID
    #[builder(default = RecordId(crate::utils::uuid_v7()))]
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

    /// The type of data we are storing here. Eg, "history"
    pub tag: String,

    /// Some data. This can be anything you wish to store. Use the tag field to know how to handle it.
    pub data: Data,
}

/// Extra data from the record that should be encoded in the data
#[derive(Debug, Copy, Clone)]
pub struct AdditionalData<'a> {
    pub id: &'a RecordId,
    pub idx: &'a u64,
    pub version: &'a str,
    pub tag: &'a str,
    pub host: &'a HostId,
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
}

/// An index representing the current state of the record stores
/// This can be both remote, or local, and compared in either direction
#[derive(Debug, Serialize, Deserialize)]
pub struct RecordStatus {
    // A map of host -> tag -> max(idx)
    pub hosts: HashMap<HostId, HashMap<String, RecordIdx>>,
}

impl Default for RecordStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl Extend<(HostId, String, RecordIdx)> for RecordStatus {
    fn extend<T: IntoIterator<Item = (HostId, String, RecordIdx)>>(&mut self, iter: T) {
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

    pub fn set_raw(&mut self, host: HostId, tag: String, tail_id: RecordIdx) {
        self.hosts.entry(host).or_default().insert(tag, tail_id);
    }

    pub fn get(&self, host: HostId, tag: String) -> Option<RecordIdx> {
        self.hosts.get(&host).and_then(|v| v.get(&tag)).cloned()
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
                match other.get(*host, tag.clone()) {
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
                match self.get(*host, tag.clone()) {
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

pub trait Encryption {
    fn re_encrypt(
        data: EncryptedData,
        ad: AdditionalData,
        old_key: &[u8; 32],
        new_key: &[u8; 32],
    ) -> Result<EncryptedData> {
        let data = Self::decrypt(data, ad, old_key)?;
        Ok(Self::encrypt(data, ad, new_key))
    }
    fn encrypt(data: DecryptedData, ad: AdditionalData, key: &[u8; 32]) -> EncryptedData;
    fn decrypt(data: EncryptedData, ad: AdditionalData, key: &[u8; 32]) -> Result<DecryptedData>;
}

impl Record<DecryptedData> {
    pub fn encrypt<E: Encryption>(self, key: &[u8; 32]) -> Record<EncryptedData> {
        let ad = AdditionalData {
            id: &self.id,
            version: &self.version,
            tag: &self.tag,
            host: &self.host.id,
            idx: &self.idx,
        };
        Record {
            data: E::encrypt(self.data, ad, key),
            id: self.id,
            host: self.host,
            idx: self.idx,
            timestamp: self.timestamp,
            version: self.version,
            tag: self.tag,
        }
    }
}

impl Record<EncryptedData> {
    pub fn decrypt<E: Encryption>(self, key: &[u8; 32]) -> Result<Record<DecryptedData>> {
        let ad = AdditionalData {
            id: &self.id,
            version: &self.version,
            tag: &self.tag,
            host: &self.host.id,
            idx: &self.idx,
        };
        Ok(Record {
            data: E::decrypt(self.data, ad, key)?,
            id: self.id,
            host: self.host,
            idx: self.idx,
            timestamp: self.timestamp,
            version: self.version,
            tag: self.tag,
        })
    }

    pub fn re_encrypt<E: Encryption>(
        self,
        old_key: &[u8; 32],
        new_key: &[u8; 32],
    ) -> Result<Record<EncryptedData>> {
        let ad = AdditionalData {
            id: &self.id,
            version: &self.version,
            tag: &self.tag,
            host: &self.host.id,
            idx: &self.idx,
        };
        Ok(Record {
            data: E::re_encrypt(self.data, ad, old_key, new_key)?,
            id: self.id,
            host: self.host,
            idx: self.idx,
            timestamp: self.timestamp,
            version: self.version,
            tag: self.tag,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::record::{Host, HostId};

    use super::{DecryptedData, Diff, Record, RecordStatus};
    use pretty_assertions::assert_eq;

    fn test_record() -> Record<DecryptedData> {
        Record::builder()
            .host(Host::new(HostId(crate::utils::uuid_v7())))
            .version("v1".into())
            .tag(crate::utils::uuid_v7().simple().to_string())
            .data(DecryptedData(vec![0, 1, 2, 3]))
            .idx(0)
            .build()
    }

    #[test]
    fn record_index() {
        let mut index = RecordStatus::new();
        let record = test_record();

        index.set(record.clone());

        let tail = index.get(record.host.id, record.tag);

        assert_eq!(
            record.idx,
            tail.expect("tail not in store"),
            "tail in store did not match"
        );
    }

    #[test]
    fn record_index_overwrite() {
        let mut index = RecordStatus::new();
        let record = test_record();
        let child = record.append(vec![1, 2, 3]);

        index.set(record.clone());
        index.set(child.clone());

        let tail = index.get(record.host.id, record.tag);

        assert_eq!(
            child.idx,
            tail.expect("tail not in store"),
            "tail in store did not match"
        );
    }

    #[test]
    fn record_index_no_diff() {
        // Here, they both have the same version and should have no diff

        let mut index1 = RecordStatus::new();
        let mut index2 = RecordStatus::new();

        let record1 = test_record();

        index1.set(record1.clone());
        index2.set(record1);

        let diff = index1.diff(&index2);

        assert_eq!(0, diff.len(), "expected empty diff");
    }

    #[test]
    fn record_index_single_diff() {
        // Here, they both have the same stores, but one is ahead by a single record

        let mut index1 = RecordStatus::new();
        let mut index2 = RecordStatus::new();

        let record1 = test_record();
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

    #[test]
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
        let smol_diff_1: Vec<(HostId, String)> =
            diff1.iter().map(|v| (v.host, v.tag.clone())).collect();
        let smol_diff_2: Vec<(HostId, String)> =
            diff1.iter().map(|v| (v.host, v.tag.clone())).collect();

        assert_eq!(smol_diff_1, smol_diff_2);

        // diffing with yourself = no diff
        assert_eq!(index1.diff(&index1).len(), 0);
        assert_eq!(index2.diff(&index2).len(), 0);
    }
}
