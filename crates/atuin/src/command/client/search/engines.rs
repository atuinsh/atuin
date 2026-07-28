use async_trait::async_trait;
use atuin_client::{
    database::{Context, Database, DbSearchMode, OptFilters},
    history::{AUTHOR_FILTER_ALL_USER, History, HistoryId},
    settings::{FilterMode, SearchMode, Settings, Shells},
};
use eyre::Result;

use super::cursor::Cursor;

#[cfg(feature = "daemon")]
pub mod daemon;
pub mod db;
mod layout;
pub mod skim;

#[cfg(test)]
mod tests;

#[allow(unused)] // settings is only used if daemon feature is enabled
pub fn engine(search_mode: SearchMode, settings: &Settings) -> Box<dyn SearchEngine> {
    match search_mode {
        SearchMode::Skim => Box::new(skim::Search::new()),
        #[cfg(feature = "daemon")]
        SearchMode::DaemonFuzzy => Box::new(daemon::Search::new(settings)),
        #[cfg(not(feature = "daemon"))]
        SearchMode::DaemonFuzzy => {
            // Fall back to fuzzy mode if daemon feature is not enabled
            Box::new(db::Search(DbSearchMode::Fuzzy))
        }
        SearchMode::Prefix => Box::new(db::Search(DbSearchMode::Prefix)),
        SearchMode::FullText => Box::new(db::Search(DbSearchMode::FullText)),
        SearchMode::Fuzzy => Box::new(db::Search(DbSearchMode::Fuzzy)),
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
    fn with_input(&self, input: String) -> Self {
        Self {
            input: input.into(),
            filter_mode: self.filter_mode,
            context: self.context.clone(),
            custom_context: self.custom_context.clone(),
            shells: self.shells.clone(),
        }
    }

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
                .search(
                    DbSearchMode::FullText,
                    state.filter_mode,
                    &state.context,
                    "",
                    OptFilters {
                        limit: Some(200),
                        authors: &[AUTHOR_FILTER_ALL_USER.to_owned()],
                        shells: state.shells.to_list().as_slice(),
                        ..Default::default()
                    },
                )
                .await?
                .into_iter()
                .collect::<Vec<_>>())
        } else {
            let results = self.full_query(state, db).await?;
            if !results.is_empty() || !self.corrects_dubeolsik_layout() {
                return Ok(results);
            }

            let Some(corrected_input) = layout::dubeolsik_to_qwerty(state.input.as_str()) else {
                return Ok(results);
            };

            self.full_query(&state.with_input(corrected_input), db)
                .await
        }
    }

    fn corrects_dubeolsik_layout(&self) -> bool {
        false
    }

    fn get_highlight_indices_for_query(&self, command: &str, search_input: &str) -> Vec<usize>;

    fn get_highlight_indices(&self, command: &str, search_input: &str) -> Vec<usize> {
        let indices = self.get_highlight_indices_for_query(command, search_input);
        if !indices.is_empty() || !self.corrects_dubeolsik_layout() {
            return indices;
        }

        layout::dubeolsik_to_qwerty(search_input).map_or(indices, |corrected_input| {
            self.get_highlight_indices_for_query(command, &corrected_input)
        })
    }
}
