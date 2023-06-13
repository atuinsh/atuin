use async_trait::async_trait;
use eyre::Result;

use atuin_common::record::Record;

/// A record store stores records
/// In more detail - we tend to need to process this into _another_ format to actually query it.
/// As is, the record store is intended as the source of truth for arbitratry data, which could
/// be shell history, kvs, etc.
#[async_trait]
pub trait Store {
    // Push a record
    async fn push(&self, record: &Record) -> Result<()> {
        self.push_batch(std::iter::once(record)).await
    }

    // Push a batch of records, all in one transaction
    async fn push_batch(&self, records: impl Iterator<Item = &Record> + Send + Sync) -> Result<()>;

    async fn get(&self, id: &str) -> Result<Record>;
    async fn len(&self, host: &str, tag: &str) -> Result<u64>;

    /// Get the record that follows this record
    async fn next(&self, record: &Record) -> Result<Option<Record>>;

    /// Get the first record for a given host and tag
    async fn first(&self, host: &str, tag: &str) -> Result<Option<Record>>;
    /// Get the last record for a given host and tag
    async fn last(&self, host: &str, tag: &str) -> Result<Option<Record>>;
}
