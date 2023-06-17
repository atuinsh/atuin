use async_trait::async_trait;
use atuin_client::{
    database::{Context, Database},
    history::History,
    settings::{FilterMode, SearchMode},
};
use eyre::Result;

use super::cursor::Cursor;

pub mod db;
pub mod skim;

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
    async fn full_query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<History>>;

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

#[cfg(test)]
async fn test_entries(since: chrono::DateTime<chrono::Utc>) -> atuin_client::database::Sqlite {
    use atuin_client::database::Sqlite;
    use chrono::Duration;

    let mut db = Sqlite::new("sqlite::memory:").await.unwrap();

    db.save_bulk(&[
        History::import()
            .timestamp(since - Duration::days(2))
            .command("docker run -e POSTGRES_USER=atuin -e POSTGRES_PASSWORD=pass -e POSTGRES_DB=atuin -p 5432:5432 -d --rm postgres:14-alpine")
            .session("1")
            .cwd("/Users/conrad/code/atuin")
            .hostname("host1:conrad")
            .build()
            .into(),
    ]).await.unwrap();

    db
}
