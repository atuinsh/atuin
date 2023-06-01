use async_trait::async_trait;
use eyre::Result;

use atuin_common::record::Record;

/// A record store stores records
/// In more detail - we tend to need to process this into _another_ format to actually query it.
/// As is, the record store is intended as the source of truth for arbitratry data, which could
/// be shell history, kvs, etc.
#[async_trait]
pub trait Store {
    async fn push(&self, record: Record) -> Result<Record>;
    async fn get(&self, id: &str) -> Result<Record>;
    async fn len(&self, host: &str, tag: &str) -> Result<u64>;
}
