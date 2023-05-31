/// A single record stored inside of our local database
pub struct Record {
    pub id: i64,

    pub host: String,

    pub timestamp: u64,

    /// The type of data we are storing here. It is probably useful to also
    /// include some sort of version. For example, history.v2
    pub tag: String,

    /// Some data. This can be anything you wish to store. Use the tag field to know how to handle it.
    pub data: Vec<u8>,
}
