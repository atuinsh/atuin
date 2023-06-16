use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

/// A single record stored inside of our local database
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TypedBuilder)]
pub struct Record {
    /// a unique ID
    #[builder(default = crate::utils::uuid_v7().as_simple().to_string())]
    pub id: String,

    /// The unique ID of the host.
    // TODO(ellie): Optimize the storage here. We use a bunch of IDs, and currently store
    // as strings. I would rather avoid normalization, so store as UUID binary instead of
    // encoding to a string and wasting much more storage.
    pub host: String,

    /// The ID of the parent entry
    // A store is technically just a double linked list
    // We can do some cheating with the timestamps, but should not rely upon them.
    // Clocks are tricksy.
    #[builder(default)]
    pub parent: Option<String>,

    /// The creation time in nanoseconds since unix epoch
    #[builder(default = chrono::Utc::now().timestamp_nanos() as u64)]
    pub timestamp: u64,

    /// The version the data in the entry conforms to
    // However we want to track versions for this tag, eg v2
    pub version: String,

    /// The type of data we are storing here. Eg, "history"
    pub tag: String,

    /// Some data. This can be anything you wish to store. Use the tag field to know how to handle it.
    pub data: Vec<u8>,
}

impl Record {
    pub fn new_child(&self, data: Vec<u8>) -> Record {
        Record::builder()
            .host(self.host.clone())
            .version(self.version.clone())
            .parent(Some(self.id.clone()))
            .tag(self.tag.clone())
            .data(data)
            .build()
    }
}
