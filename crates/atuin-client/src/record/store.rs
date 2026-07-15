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
        self.push_batch(&mut std::iter::once(record)).await
    }

    // Push a batch of records, all in one transaction.
    //
    // Takes a `&mut dyn Iterator` rather than `impl Iterator` so that `Store`
    // stays object-safe (`Box<dyn Store>` is used for the daemon proxy).
    async fn push_batch(
        &self,
        records: &mut (dyn Iterator<Item = &Record<EncryptedData>> + Send),
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

/// A boxed record store. This is the type the typed stores hold so they can be
/// backed either by the local `SqliteStore` or by the daemon proxy.
pub type BoxStore = Box<dyn Store + Send + Sync>;

/// Blanket forwarding impl so a `BoxStore` is itself a `Store`.
#[async_trait]
impl Store for BoxStore {
    async fn push_batch(
        &self,
        records: &mut (dyn Iterator<Item = &Record<EncryptedData>> + Send),
    ) -> Result<()> {
        (**self).push_batch(records).await
    }
    async fn get(&self, id: RecordId) -> Result<Record<EncryptedData>> {
        (**self).get(id).await
    }
    async fn delete(&self, id: RecordId) -> Result<()> {
        (**self).delete(id).await
    }
    async fn delete_all(&self) -> Result<()> {
        (**self).delete_all().await
    }
    async fn len_all(&self) -> Result<u64> {
        (**self).len_all().await
    }
    async fn len(&self, host: HostId, tag: &str) -> Result<u64> {
        (**self).len(host, tag).await
    }
    async fn len_tag(&self, tag: &str) -> Result<u64> {
        (**self).len_tag(tag).await
    }
    async fn last(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        (**self).last(host, tag).await
    }
    async fn first(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        (**self).first(host, tag).await
    }
    async fn re_encrypt(&self, old_key: &[u8; 32], new_key: &[u8; 32]) -> Result<()> {
        (**self).re_encrypt(old_key, new_key).await
    }
    async fn verify(&self, key: &[u8; 32]) -> Result<()> {
        (**self).verify(key).await
    }
    async fn purge(&self, key: &[u8; 32]) -> Result<()> {
        (**self).purge(key).await
    }
    async fn next(
        &self,
        host: HostId,
        tag: &str,
        idx: RecordIdx,
        limit: u64,
    ) -> Result<Vec<Record<EncryptedData>>> {
        (**self).next(host, tag, idx, limit).await
    }
    async fn idx(
        &self,
        host: HostId,
        tag: &str,
        idx: RecordIdx,
    ) -> Result<Option<Record<EncryptedData>>> {
        (**self).idx(host, tag, idx).await
    }
    async fn status(&self) -> Result<RecordStatus> {
        (**self).status().await
    }
    async fn all_tagged(&self, tag: &str) -> Result<Vec<Record<EncryptedData>>> {
        (**self).all_tagged(tag).await
    }
}
