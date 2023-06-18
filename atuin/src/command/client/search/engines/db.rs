use async_trait::async_trait;
use atuin_client::{
    database::Database, database::OptFilters, history::History, settings::SearchMode,
};
use chrono::{DateTime, Utc};
use eyre::Result;

use super::{SearchEngine, SearchState};

pub struct Search(pub SearchMode);

#[async_trait]
impl SearchEngine for Search {
    async fn full_query_since(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
        _now: DateTime<Utc>,
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

#[cfg(test)]
mod tests {
    mod fuzzy {
        use atuin_client::settings::SearchMode;
        use insta::assert_debug_snapshot;

        use crate::command::client::search::engines::{self, db::Search};

        #[tokio::test]
        async fn docker_postgres() {
            assert_debug_snapshot!(engines::test::docker_postgres(Search(SearchMode::Fuzzy)).await);
        }

        #[tokio::test]
        async fn postgres() {
            assert_debug_snapshot!(engines::test::postgres(Search(SearchMode::Fuzzy)).await);
        }
    }

    mod full_text {
        use atuin_client::settings::SearchMode;
        use insta::assert_debug_snapshot;

        use crate::command::client::search::engines::{self, db::Search};

        #[tokio::test]
        async fn docker_postgres() {
            assert_debug_snapshot!(engines::test::docker_postgres(Search(SearchMode::FullText)).await);
        }

        #[tokio::test]
        async fn postgres() {
            assert_debug_snapshot!(engines::test::postgres(Search(SearchMode::FullText)).await);
        }
    }

    mod prefix {
        use atuin_client::settings::SearchMode;
        use insta::assert_debug_snapshot;

        use crate::command::client::search::engines::{self, db::Search};

        #[tokio::test]
        async fn docker_postgres() {
            assert_debug_snapshot!(engines::test::docker_postgres(Search(SearchMode::Prefix)).await);
        }

        #[tokio::test]
        async fn postgres() {
            assert_debug_snapshot!(engines::test::postgres(Search(SearchMode::Prefix)).await);
        }
    }
}
