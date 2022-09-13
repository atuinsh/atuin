use std::env;

use super::{
    history::History,
    settings::{FilterMode, SearchMode},
};

mod sqlite;
use chrono::Utc;
pub use sqlite::Sqlite;

pub struct Context {
    session: String,
    cwd: String,
    hostname: String,
}

impl Default for Context {
    fn default() -> Self {
        let session = env::var("ATUIN_SESSION")
            .expect("failed to find ATUIN_SESSION - check your shell setup");
        let hostname = format!("{}:{}", whoami::hostname(), whoami::username());
        let cwd = match env::current_dir() {
            Ok(dir) => dir.display().to_string(),
            Err(_) => String::from(""),
        };
        Self {
            session,
            hostname,
            cwd,
        }
    }
}

pub trait Database {
    type Error: std::error::Error + Send + Sync + 'static;

    fn save(&mut self, h: &History) -> Result<(), Self::Error>;
    fn save_bulk(&mut self, h: &[History]) -> Result<(), Self::Error>;

    fn load(&self, id: &str) -> Result<History, Self::Error>;
    fn list(
        &self,
        filter: FilterMode,
        context: &Context,
        max: Option<usize>,
        unique: bool,
    ) -> Result<Vec<History>, Self::Error>;
    fn range(
        &self,
        from: chrono::DateTime<Utc>,
        to: chrono::DateTime<Utc>,
    ) -> Result<Vec<History>, Self::Error>;

    fn update(&self, h: &History) -> Result<(), Self::Error>;
    fn history_count(&self) -> Result<i64, Self::Error>;

    fn first(&self) -> Result<History, Self::Error>;
    fn last(&self) -> Result<History, Self::Error>;
    fn before(
        &self,
        timestamp: chrono::DateTime<Utc>,
        count: i64,
    ) -> Result<Vec<History>, Self::Error>;

    fn search(
        &self,
        limit: Option<i64>,
        search_mode: SearchMode,
        filter: FilterMode,
        context: &Context,
        query: &str,
    ) -> Result<Vec<History>, Self::Error>;

    fn query_history(&self, query: &str) -> Result<Vec<History>, Self::Error>;
}
