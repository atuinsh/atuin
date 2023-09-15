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

#[derive(Debug, PartialEq)]
pub struct Diff {
    pub host: HostId,
    pub tag: String,
    pub tail: RecordId,
}

/// A single record stored inside of our local database
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TypedBuilder)]
pub struct Record<Data> {
    /// a unique ID
    #[builder(default = RecordId(crate::utils::uuid_v7()))]
    pub id: RecordId,

    /// The unique ID of the host.
    // TODO(ellie): Optimize the storage here. We use a bunch of IDs, and currently store
    // as strings. I would rather avoid normalization, so store as UUID binary instead of
    // encoding to a string and wasting much more storage.
    pub host: HostId,

    /// The ID of the parent entry
    // A store is technically just a double linked list
    // We can do some cheating with the timestamps, but should not rely upon them.
    // Clocks are tricksy.
    #[builder(default)]
    pub parent: Option<RecordId>,

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

new_uuid!(RecordId);
new_uuid!(HostId);

/// Extra data from the record that should be encoded in the data
#[derive(Debug, Copy, Clone)]
pub struct AdditionalData<'a> {
    pub id: &'a RecordId,
    pub version: &'a str,
    pub tag: &'a str,
    pub host: &'a HostId,
    pub parent: Option<&'a RecordId>,
}

impl<Data> Record<Data> {
    pub fn new_child(&self, data: Vec<u8>) -> Record<DecryptedData> {
        Record::builder()
            .host(self.host)
            .version(self.version.clone())
            .parent(Some(self.id))
            .tag(self.tag.clone())
            .data(DecryptedData(data))
            .build()
    }
}

/// An index representing the current state of the record stores
/// This can be both remote, or local, and compared in either direction
#[derive(Debug, Serialize, Deserialize)]
pub struct RecordIndex {
    // A map of host -> tag -> tail
    pub hosts: HashMap<HostId, HashMap<String, RecordId>>,
}

impl Default for RecordIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl Extend<(HostId, String, RecordId)> for RecordIndex {
    fn extend<T: IntoIterator<Item = (HostId, String, RecordId)>>(&mut self, iter: T) {
        for (host, tag, tail_id) in iter {
            self.set_raw(host, tag, tail_id);
        }
    }
}

impl RecordIndex {
    pub fn new() -> RecordIndex {
        RecordIndex {
            hosts: HashMap::new(),
        }
    }

    /// Insert a new tail record into the store
    pub fn set(&mut self, tail: Record<DecryptedData>) {
        self.set_raw(tail.host, tail.tag, tail.id)
    }

    pub fn set_raw(&mut self, host: HostId, tag: String, tail_id: RecordId) {
        self.hosts.entry(host).or_default().insert(tag, tail_id);
    }

    pub fn get(&self, host: HostId, tag: String) -> Option<RecordId> {
        self.hosts.get(&host).and_then(|v| v.get(&tag)).cloned()
    }

    /// Diff this index with another, likely remote index.
    /// The two diffs can then be reconciled, and the optimal change set calculated
    /// Returns a tuple, with (host, tag, Option(OTHER))
    /// OTHER is set to the value of the tail on the other machine. For example, if the
    /// other machine has a different tail, it will be the differing tail. This is useful to
    /// check if the other index is ahead of us, or behind.
    /// If the other index does not have the (host, tag) pair, then the other value will be None.
    pub fn diff(&self, other: &Self) -> Vec<Diff> {
        let mut ret = Vec::new();

        // First, we check if other has everything that self has
        for (host, tag_map) in self.hosts.iter() {
            for (tag, tail) in tag_map.iter() {
                match other.get(*host, tag.clone()) {
                    // The other store is all up to date! No diff.
                    Some(t) if t.eq(tail) => continue,

                    // The other store does exist, but it is either ahead or behind us. A diff regardless
                    Some(t) => ret.push(Diff {
                        host: *host,
                        tag: tag.clone(),
                        tail: t,
                    }),

                    // The other store does not exist :O
                    None => ret.push(Diff {
                        host: *host,
                        tag: tag.clone(),
                        tail: *tail,
                    }),
                };
            }
        }

        // At this point, there is a single case we have not yet considered.
        // If the other store knows of a tag that we are not yet aware of, then the diff will be missed

        // account for that!
        for (host, tag_map) in other.hosts.iter() {
            for (tag, tail) in tag_map.iter() {
                match self.get(*host, tag.clone()) {
                    // If we have this host/tag combo, the comparison and diff will have already happened above
                    Some(_) => continue,

                    None => ret.push(Diff {
                        host: *host,
                        tag: tag.clone(),
                        tail: *tail,
                    }),
                };
            }
        }

        ret.sort_by(|a, b| (a.host, a.tag.clone(), a.tail).cmp(&(b.host, b.tag.clone(), b.tail)));
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
            host: &self.host,
            parent: self.parent.as_ref(),
        };
        Record {
            data: E::encrypt(self.data, ad, key),
            id: self.id,
            host: self.host,
            parent: self.parent,
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
            host: &self.host,
            parent: self.parent.as_ref(),
        };
        Ok(Record {
            data: E::decrypt(self.data, ad, key)?,
            id: self.id,
            host: self.host,
            parent: self.parent,
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
            host: &self.host,
            parent: self.parent.as_ref(),
        };
        Ok(Record {
            data: E::re_encrypt(self.data, ad, old_key, new_key)?,
            id: self.id,
            host: self.host,
            parent: self.parent,
            timestamp: self.timestamp,
            version: self.version,
            tag: self.tag,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::record::HostId;

    use super::{DecryptedData, Diff, Record, RecordIndex};
    use pretty_assertions::assert_eq;

    fn test_record() -> Record<DecryptedData> {
        Record::builder()
            .host(HostId(crate::utils::uuid_v7()))
            .version("v1".into())
            .tag(crate::utils::uuid_v7().simple().to_string())
            .data(DecryptedData(vec![0, 1, 2, 3]))
            .build()
    }

    #[test]
    fn record_index() {
        let mut index = RecordIndex::new();
        let record = test_record();

        index.set(record.clone());

        let tail = index.get(record.host, record.tag);

        assert_eq!(
            record.id,
            tail.expect("tail not in store"),
            "tail in store did not match"
        );
    }

    #[test]
    fn record_index_overwrite() {
        let mut index = RecordIndex::new();
        let record = test_record();
        let child = record.new_child(vec![1, 2, 3]);

        index.set(record.clone());
        index.set(child.clone());

        let tail = index.get(record.host, record.tag);

        assert_eq!(
            child.id,
            tail.expect("tail not in store"),
            "tail in store did not match"
        );
    }

    #[test]
    fn record_index_no_diff() {
        // Here, they both have the same version and should have no diff

        let mut index1 = RecordIndex::new();
        let mut index2 = RecordIndex::new();

        let record1 = test_record();

        index1.set(record1.clone());
        index2.set(record1);

        let diff = index1.diff(&index2);

        assert_eq!(0, diff.len(), "expected empty diff");
    }

    #[test]
    fn record_index_single_diff() {
        // Here, they both have the same stores, but one is ahead by a single record

        let mut index1 = RecordIndex::new();
        let mut index2 = RecordIndex::new();

        let record1 = test_record();
        let record2 = record1.new_child(vec![1, 2, 3]);

        index1.set(record1);
        index2.set(record2.clone());

        let diff = index1.diff(&index2);

        assert_eq!(1, diff.len(), "expected single diff");
        assert_eq!(
            diff[0],
            Diff {
                host: record2.host,
                tag: record2.tag,
                tail: record2.id
            }
        );
    }

    #[test]
    fn record_index_multi_diff() {
        // A much more complex case, with a bunch more checks
        let mut index1 = RecordIndex::new();
        let mut index2 = RecordIndex::new();

        let store1record1 = test_record();
        let store1record2 = store1record1.new_child(vec![1, 2, 3]);

        let store2record1 = test_record();
        let store2record2 = store2record1.new_child(vec![1, 2, 3]);

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
