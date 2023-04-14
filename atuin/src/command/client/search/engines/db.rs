use async_trait::async_trait;
use atuin_client::{
    database::Database, database::OptFilters, history::History, settings::SearchMode,
};
use eyre::Result;

use super::{SearchEngine, SearchState};

pub struct Search(pub SearchMode);

#[async_trait]
impl SearchEngine for Search {
    async fn full_query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<History>> {
        Ok(db
            .search(
                self.0,
                state.filter_mode,
                &state.context,
                state.input.as_str(),
                OptFilters {
                    limit: Some(200),
                    ..Default::default()
                },
            )
            .await?
            .into_iter()
            .collect::<Vec<_>>())
    }
}
