/// A single record stored inside of our local database
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: String,

    pub host: String,

    pub timestamp: u64,

    // However we want to track versions for this tag, eg v2
    pub version: String,

    /// The type of data we are storing here. Eg, "history"
    pub tag: String,

    /// Some data. This can be anything you wish to store. Use the tag field to know how to handle it.
    pub data: Vec<u8>,
}

impl Record {
    pub fn new(host: String, version: String, tag: String, data: Vec<u8>) -> Record {
        let id = crate::utils::uuid_v7().as_simple().to_string();
        let timestamp = chrono::Utc::now();

        Record {
            id,
            host,
            timestamp: timestamp.timestamp_nanos() as u64,
            version,
            tag,
            data,
        }
    }
}
