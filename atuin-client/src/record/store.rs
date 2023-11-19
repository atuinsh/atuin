use async_trait::async_trait;
use eyre::Result;

use atuin_common::record::{EncryptedData, HostId, Record, RecordId, RecordIdx, RecordStatus};

/// A record store stores records
/// In more detail - we tend to need to process this into _another_ format to actually query it.
/// As is, the record store is intended as the source of truth for arbitratry data, which could
/// be shell history, kvs, etc.
#[async_trait]
pub trait Store {
    // Push a record
    async fn push(&self, record: &Record<EncryptedData>) -> Result<()> {
        self.push_batch(std::iter::once(record)).await
    }

    // Push a batch of records, all in one transaction
    async fn push_batch(
        &self,
        records: impl Iterator<Item = &Record<EncryptedData>> + Send + Sync,
    ) -> Result<()>;

    async fn get(&self, id: RecordId) -> Result<Record<EncryptedData>>;
    async fn len(&self, host: HostId, tag: &str) -> Result<Option<u64>>;

    async fn last(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>>;
    async fn first(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>>;

    /// Get the record that follows this record
    async fn next(
        &self,
        host: HostId,
        tag: &str,
        idx: RecordIdx,
        limit: u64,
    ) -> Result<Vec<Record<EncryptedData>>>;

    /// Get the first record for a given host and tag
    async fn idx(
        &self,
        host: HostId,
        tag: &str,
        idx: RecordIdx,
    ) -> Result<Option<Record<EncryptedData>>>;

    async fn status(&self) -> Result<RecordStatus>;

    /// Get every start record for a given tag, regardless of host.
    /// Useful when actually operating on synchronized data, and will often have conflict
    /// resolution applied.
    async fn all_tagged(&self, tag: &str) -> Result<Vec<Record<EncryptedData>>>;
}
