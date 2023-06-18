use async_trait::async_trait;
use atuin_client::{
    database::{Context, Database},
    history::History,
    settings::{FilterMode, SearchMode},
};
use chrono::{DateTime, Utc};
use eyre::Result;

use super::cursor::Cursor;

pub mod db;
pub mod skim;

#[cfg(test)]
pub mod test;

pub fn engine(search_mode: SearchMode) -> Box<dyn SearchEngine> {
    match search_mode {
        SearchMode::Skim => Box::new(skim::Search::new()) as Box<_>,
        mode => Box::new(db::Search(mode)) as Box<_>,
    }
}

pub struct SearchState {
    pub input: Cursor,
    pub filter_mode: FilterMode,
    pub context: Context,
}

#[async_trait]
pub trait SearchEngine: Send + Sync + 'static {
    async fn full_query_since(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
        now: DateTime<Utc>,
    ) -> Result<Vec<History>>;

    async fn full_query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<History>> {
        self.full_query_since(state, db, Utc::now()).await
    }

    async fn query(&mut self, state: &SearchState, db: &mut dyn Database) -> Result<Vec<History>> {
        if state.input.as_str().is_empty() {
            Ok(db
                .list(state.filter_mode, &state.context, Some(200), true)
                .await?
                .into_iter()
                .collect::<Vec<_>>())
        } else {
            self.full_query(state, db).await
        }
    }
}
