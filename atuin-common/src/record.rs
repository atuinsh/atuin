use serde::{Deserialize, Serialize};

/// A single record stored inside of our local database
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: String, // a unique ID

    // TODO(ellie): Optimize the storage here. We use a bunch of IDs, and currently store
    // as strings. I would rather avoid normalization, so store as UUID binary instead of
    // encoding to a string and wasting much more storage.
    pub host: String,

    // A store is technically just a double linked list
    // We can do some cheating with the timestamps, but should not rely upon them.
    // Clocks are tricksy.
    pub parent: Option<String>,

    pub timestamp: u64,

    // However we want to track versions for this tag, eg v2
    pub version: String,

    /// The type of data we are storing here. Eg, "history"
    pub tag: String,

    /// Some data. This can be anything you wish to store. Use the tag field to know how to handle it.
    pub data: Vec<u8>,
}

impl Record {
    pub fn new(
        host: String,
        version: String,
        tag: String,
        parent: Option<String>,
        data: Vec<u8>,
    ) -> Record {
        let id = crate::utils::uuid_v7().as_simple().to_string();
        let timestamp = chrono::Utc::now();

        Record {
            id,
            host,
            parent,
            timestamp: timestamp.timestamp_nanos() as u64,
            version,
            tag,
            data,
        }
    }

    pub fn new_child(&self, data: Vec<u8>) -> Record {
        Self::new(
            self.host.clone(),
            self.version.clone(),
            self.tag.clone(),
            Some(self.id.clone()),
            data,
        )
    }
}
