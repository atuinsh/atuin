use std::sync::Arc;

use async_trait::async_trait;
use atuin_client::{database::Database, settings::SearchMode};
use eyre::Result;

use super::interactive::{HistoryWrapper, SearchEngine, SearchState};

pub struct Search(pub SearchMode);

#[async_trait]
impl SearchEngine for Search {
    async fn query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<Arc<HistoryWrapper>>> {
        Ok(db
            .search(
                self.0,
                state.filter_mode,
                &state.context,
                state.input.as_str(),
                Some(200),
                None,
                None,
            )
            .await?
            .into_iter()
            .map(|history| HistoryWrapper { history, count: 1 })
            .map(Arc::new)
            .collect::<Vec<_>>())
    }
}
