use async_trait::async_trait;
use eyre::Result;

use atuin_common::record::Record;

/// A record store stores records
/// In more detail - we tend to need to process this into _another_ format to actually query it.
/// As is, the record store is intended as the source of truth for arbitratry data, which could
/// be shell history, kvs, etc.
#[async_trait]
pub trait Store {
    // Push a record and return it
    async fn push(&self, record: Record) -> Result<Record>;

    // Push a batch of records, all in one transaction
    // Returns a record if you push at least one. If the iterator is empty, then
    // there is no return record.
    async fn push_batch(
        &self,
        records: impl Iterator<Item = &Record> + Send + Sync,
    ) -> Result<Option<Record>>;
    async fn get(&self, id: &str) -> Result<Record>;
    async fn len(&self, host: &str, tag: &str) -> Result<u64>;

    async fn next(&self, record: &Record) -> Result<Option<Record>>;

    // Get the first record for a given host and tag
    async fn first(&self, host: &str, tag: &str) -> Result<Record>;
    async fn last(&self, host: &str, tag: &str) -> Result<Record>;
}
