use atuin_client::database::{Context, DbSearchMode, OptFilters, Sqlite};
use atuin_client::history::{History, HistoryId, all_user_author_filter};
use atuin_client::settings::{FilterMode, SearchMode, Settings, Shells};
use enum_dispatch::enum_dispatch;
use eyre::Result;

use super::cursor::Cursor;

#[cfg(feature = "daemon")]
pub mod daemon;
pub mod db;

#[allow(unused)] // settings is only used if daemon feature is enabled
pub fn engine(search_mode: SearchMode, settings: &Settings) -> AnySearchEngine {
    match search_mode {
        #[cfg(feature = "daemon")]
        SearchMode::DaemonFuzzy => Box::new(daemon::Search::new(settings)).into(),
        #[cfg(not(feature = "daemon"))]
        SearchMode::DaemonFuzzy => {
            // Fall back to fuzzy mode if daemon feature is not enabled
            db::Search(DbSearchMode::Fuzzy).into()
        }
        SearchMode::Prefix => db::Search(DbSearchMode::Prefix).into(),
        SearchMode::FullText => db::Search(DbSearchMode::FullText).into(),
        SearchMode::Fuzzy => db::Search(DbSearchMode::Fuzzy).into(),
    }
}

pub struct SearchState {
    pub input: Cursor,
    pub filter_mode: FilterMode,
    pub context: Context,
    pub custom_context: Option<HistoryId>,
    pub shells: Shells,
}

impl SearchState {
    pub(crate) fn rotate_filter_mode(&mut self, settings: &Settings, offset: isize) {
        let mut i =
            settings.search.filters.iter().position(|&m| m == self.filter_mode).unwrap_or_default();
        for _ in 0..settings.search.filters.len() {
            i = (i.wrapping_add_signed(offset)) % settings.search.filters.len();
            let mode = settings.search.filters[i];
            if self.filter_mode_available(mode, settings) {
                self.filter_mode = mode;
                break;
            }
        }
    }

    fn filter_mode_available(&self, mode: FilterMode, settings: &Settings) -> bool {
        match mode {
            FilterMode::Global | FilterMode::SessionPreload => self.custom_context.is_none(),
            FilterMode::Workspace => settings.workspaces && self.context.git_root.is_some(),
            _ => true,
        }
    }
}

#[enum_dispatch]
pub trait SearchEngine: Send + Sync + 'static {
    async fn full_query(&mut self, state: &SearchState, db: &mut Sqlite) -> Result<Vec<History>>;

    async fn query(&mut self, state: &SearchState, db: &mut Sqlite) -> Result<Vec<History>> {
        if state.input.as_str().is_empty() {
            let shells = state.shells.to_filter();
            Ok(db
                .search(DbSearchMode::FullText, state.filter_mode, &state.context, "", OptFilters {
                    limit: Some(200),
                    authors: all_user_author_filter(),
                    shells: shells.as_filter(),
                    ..Default::default()
                })
                .await?
                .into_iter()
                .collect::<Vec<_>>())
        } else {
            self.full_query(state, db).await
        }
    }

    fn get_highlight_indices(&self, command: &str, search_input: &str) -> Vec<usize>;
}

impl<T: SearchEngine> SearchEngine for Box<T> {
    async fn full_query(&mut self, state: &SearchState, db: &mut Sqlite) -> Result<Vec<History>> {
        T::full_query(self, state, db).await
    }

    async fn query(&mut self, state: &SearchState, db: &mut Sqlite) -> Result<Vec<History>> {
        T::query(self, state, db).await
    }

    fn get_highlight_indices(&self, command: &str, search_input: &str) -> Vec<usize> {
        T::get_highlight_indices(self, command, search_input)
    }
}

/// Static-dispatch enum over the search-engine backends.
#[enum_dispatch(SearchEngine)]
pub enum AnySearchEngine {
    Db(db::Search),
    #[cfg(feature = "daemon")]
    Daemon(Box<daemon::Search>),
}
