use async_trait::async_trait;
use atuin_client::{
    database::{Context, Database},
    history::{History, HistoryId},
    settings::{FilterMode, SearchMode, Settings},
};
use eyre::Result;

use super::cursor::Cursor;

#[cfg(feature = "daemon")]
pub mod daemon;
pub mod db;
pub mod skim;

#[allow(unused)] // settings is only used if daemon feature is enabled
pub fn engine(search_mode: SearchMode, settings: &Settings) -> Box<dyn SearchEngine> {
    match search_mode {
        SearchMode::Skim => Box::new(skim::Search::new()) as Box<_>,
        #[cfg(feature = "daemon")]
        SearchMode::DaemonFuzzy => Box::new(daemon::Search::new(settings)) as Box<_>,
        #[cfg(not(feature = "daemon"))]
        SearchMode::DaemonFuzzy => {
            // Fall back to fuzzy mode if daemon feature is not enabled
            Box::new(db::Search(SearchMode::Fuzzy)) as Box<_>
        }
        mode => Box::new(db::Search(mode)) as Box<_>,
    }
}

pub struct SearchState {
    pub input: Cursor,
    pub filter_mode: FilterMode,
    pub context: Context,
    pub custom_context: Option<HistoryId>,
}

impl SearchState {
    pub(crate) fn rotate_filter_mode(&mut self, settings: &Settings, offset: isize) {
        let mut i = settings
            .search
            .filters
            .iter()
            .position(|&m| m == self.filter_mode)
            .unwrap_or_default();
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
                .list(&[state.filter_mode], &state.context, Some(200), true, false)
                .await?
                .into_iter()
                .collect::<Vec<_>>())
        } else {
            self.full_query(state, db).await
        }
    }
    fn get_highlight_indices(&self, command: &str, search_input: &str) -> Vec<usize>;
}
