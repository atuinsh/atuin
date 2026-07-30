pub mod action_ctx;
pub mod history;
pub mod search_input;

pub use action_ctx::{Action, ActionCtx};
pub use history::{HistoryList, HistoryRow, HistorySource};
pub use search_input::SearchInput;

use crate::theme::Theme;

/// The state we render. Grows over time; for now it carries the theme, the flags
/// the header reduces into available actions, and the (virtualized) history list.
pub struct Model {
    pub theme: Theme,
    /// Whether `<enter>` runs (`true`) or edits (`false`) the selected command.
    pub enter_accept: bool,
    pub history: HistoryList,
}

impl Model {
    /// A read-only view of which actions are currently available — which keys do
    /// what, given the current state. Borrows the model for its lifetime.
    pub fn ctx(&self) -> ActionCtx<'_> {
        ActionCtx::from_model(self)
    }
}
