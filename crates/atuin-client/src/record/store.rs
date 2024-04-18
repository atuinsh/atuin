use async_trait::async_trait;
use eyre::Result;

use atuin_common::record::{EncryptedData, HostId, Record, RecordId, RecordIdx, RecordStatus};

/// A record store stores records
/// In more detail - we tend to need to process this into _another_ format to actually query it.
/// As is, the record store is intended as the source of truth for arbitrary data, which could
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

    async fn delete(&self, id: RecordId) -> Result<()>;
    async fn delete_all(&self) -> Result<()>;

    async fn len_all(&self) -> Result<u64>;
    async fn len(&self, host: HostId, tag: &str) -> Result<u64>;
    async fn len_tag(&self, tag: &str) -> Result<u64>;

    async fn last(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>>;
    async fn first(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>>;

    async fn re_encrypt(&self, old_key: &[u8; 32], new_key: &[u8; 32]) -> Result<()>;
    async fn verify(&self, key: &[u8; 32]) -> Result<()>;
    async fn purge(&self, key: &[u8; 32]) -> Result<()>;

    /// Get the next `limit` records, after and including the given index
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

    /// Get all records for a given tag
    async fn all_tagged(&self, tag: &str) -> Result<Vec<Record<EncryptedData>>>;
}
