use atuin_client::history::HistoryId;

use super::{Index, IndexError, OutputMatch};

/// An [`Index`] that stores nothing and matches nothing.
///
/// Stands in when output capture is disabled or the sqlite index fails to open, so `search` simply
/// comes back empty instead of erroring.
#[derive(Debug, Clone, Copy)]
pub struct NopIndex;

impl Index for NopIndex {
    async fn insert(&self, _id: HistoryId, _text: &str) -> Result<(), IndexError> {
        Ok(())
    }

    async fn remove(&self, _ids: &[HistoryId]) -> Result<(), IndexError> {
        Ok(())
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<OutputMatch>, IndexError> {
        Ok(Vec::new())
    }

    async fn indexed_ids(&self) -> Result<Vec<HistoryId>, IndexError> {
        Ok(Vec::new())
    }
}
