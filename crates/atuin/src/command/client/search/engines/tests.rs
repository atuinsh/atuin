use async_trait::async_trait;
use atuin_client::{
    database::{Context, Database, DbSearchMode, Sqlite},
    history::History,
    settings::{FilterMode, Shells},
};
use eyre::Result;
use time::OffsetDateTime;

use super::{SearchEngine, SearchState, db};

fn search_state(input: &str) -> SearchState {
    SearchState {
        input: input.to_owned().into(),
        filter_mode: FilterMode::Global,
        context: Context {
            session: "test-session".to_owned(),
            cwd: "/tmp".to_owned(),
            hostname: "test-host".to_owned(),
            host_id: "test-host-id".to_owned(),
            git_root: None,
        },
        custom_context: None,
        shells: Shells::All,
    }
}

async fn database_with(commands: &[&str]) -> Sqlite {
    let db = Sqlite::new("sqlite::memory:", 2.0).await.unwrap();
    for command in commands {
        let history: History = History::capture()
            .timestamp(OffsetDateTime::now_utc())
            .command(*command)
            .cwd("/tmp")
            .build()
            .into();
        db.save(&history).await.unwrap();
    }
    db
}

async fn fuzzy_commands(query: &str, commands: &[&str]) -> Vec<String> {
    let mut database = database_with(commands).await;
    let mut engine = db::Search(DbSearchMode::Fuzzy);

    engine
        .query(&search_state(query), &mut database)
        .await
        .unwrap()
        .into_iter()
        .map(|history| history.command)
        .collect()
}

#[tokio::test]
async fn native_korean_match_wins_before_layout_fallback() {
    let mut database = database_with(&["echo 안녕", "dkssud"]).await;
    let mut engine = db::Search(DbSearchMode::Fuzzy);

    let results = engine
        .query(&search_state("안녕"), &mut database)
        .await
        .unwrap();

    assert_eq!(
        results
            .iter()
            .map(|history| history.command.as_str())
            .collect::<Vec<_>>(),
        ["echo 안녕"]
    );
}

#[derive(Default)]
struct CountingSearch {
    queries: Vec<String>,
}

#[async_trait]
impl SearchEngine for CountingSearch {
    async fn full_query(
        &mut self,
        state: &SearchState,
        _database: &mut dyn Database,
    ) -> Result<Vec<History>> {
        self.queries.push(state.input.as_str().to_owned());
        Ok(Vec::new())
    }

    fn corrects_dubeolsik_layout(&self) -> bool {
        true
    }

    fn get_highlight_indices_for_query(&self, _command: &str, _search_input: &str) -> Vec<usize> {
        Vec::new()
    }
}

#[tokio::test]
async fn ascii_query_does_not_retry() {
    let mut database = database_with(&[]).await;
    let mut engine = CountingSearch::default();

    let results = engine
        .query(&search_state("git worktree"), &mut database)
        .await
        .unwrap();

    assert!(results.is_empty());
    assert_eq!(engine.queries, ["git worktree"]);
}

#[tokio::test]
async fn prefix_search_does_not_use_layout_fallback() {
    let mut database = database_with(&["et work"]).await;
    let mut engine = db::Search(DbSearchMode::Prefix);

    let results = engine
        .query(&search_state("ㄷㅅ 재가"), &mut database)
        .await
        .unwrap();

    assert!(results.is_empty());
}

#[tokio::test]
async fn search_falls_back_from_reported_dubeolsik_query() {
    let commands = fuzzy_commands("ㄷㅅ 재가", &["et work"]).await;

    assert_eq!(commands, ["et work"]);
}

#[tokio::test]
async fn search_falls_back_for_mixed_ascii_and_dubeolsik() {
    let commands = fuzzy_commands("git 재가", &["git work"]).await;

    assert_eq!(commands, ["git work"]);
}

#[tokio::test]
async fn search_falls_back_for_compound_dubeolsik_jamo() {
    let commands = fuzzy_commands("ㄳㅘ", &["rthk"]).await;

    assert_eq!(commands, ["rthk"]);
}

#[test]
fn fuzzy_highlight_uses_dubeolsik_fallback() {
    let engine = db::Search(DbSearchMode::Fuzzy);

    let indices = engine.get_highlight_indices("et work", "ㄷㅅ 재가");

    assert!(!indices.is_empty());
}
