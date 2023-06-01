/// A single record stored inside of our local database
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
