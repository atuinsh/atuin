pub mod action_ctx;
pub mod history;
pub mod mode;
pub mod search_input;

pub use action_ctx::{Action, ActionCtx};
pub use history::{HistoryList, HistoryRow, HistorySource};
pub use mode::Mode;
pub use search_input::SearchInput;

use crate::theme::Theme;

/// The state we render. Grows over time; for now it carries the theme, the flags
/// the header reduces into available actions, the (virtualized) history list, the
/// query, and — when the vim-style interface is enabled — the current [`Mode`].
pub struct Model {
    pub theme: Theme,
    /// Whether `<enter>` runs (`true`) or edits (`false`) the selected command.
    pub enter_accept: bool,
    pub history: HistoryList,
    /// The current search query and its editing cursor.
    pub search: SearchInput,
    /// The vim-style mode, or `None` for the plain (non-modal) interface. When
    /// `Some`, the app boots in [`Mode::Search`].
    pub mode: Option<Mode>,
}

impl Model {
    /// A read-only view of which actions are currently available — which keys do
    /// what, given the current state. Borrows the model for its lifetime.
    pub fn ctx(&self) -> ActionCtx<'_> {
        ActionCtx::from_model(self)
    }

    /// The current vim-style mode, or `None` when the interface is non-modal.
    pub fn mode(&self) -> Option<Mode> {
        self.mode
    }

    /// Switch to SEARCH mode. No-op unless the interface is modal.
    pub fn enter_search(&mut self) {
        if self.mode.is_some() {
            self.mode = Some(Mode::Search);
        }
    }

    /// Switch to NORMAL mode (fresh count). No-op unless the interface is modal.
    pub fn enter_normal(&mut self) {
        if self.mode.is_some() {
            self.mode = Some(Mode::Normal { count: None });
        }
    }

    /// Whether a numeric count is currently accumulating (NORMAL mode).
    pub fn count_pending(&self) -> bool {
        matches!(self.mode, Some(Mode::Normal { count: Some(_) }))
    }

    /// Append a decimal `digit` (0–9) to the pending NORMAL-mode count. Saturating,
    /// so a pathologically long prefix can't overflow. No-op outside NORMAL.
    pub fn push_count_digit(&mut self, digit: u8) {
        if let Some(Mode::Normal { count }) = &mut self.mode {
            let next = count
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(digit as usize);
            *count = Some(next);
        }
    }

    /// Consume the pending NORMAL-mode count, returning at least 1 and resetting
    /// it. Returns 1 outside NORMAL or when no count was typed.
    pub fn take_count(&mut self) -> usize {
        if let Some(Mode::Normal { count }) = &mut self.mode {
            count.take().unwrap_or(1).max(1)
        } else {
            1
        }
    }

    /// Discard any pending NORMAL-mode count.
    pub fn clear_count(&mut self) {
        if let Some(Mode::Normal { count }) = &mut self.mode {
            *count = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn modal_model() -> Model {
        Model {
            theme: Theme::default(),
            enter_accept: true,
            history: HistoryList::new(),
            search: SearchInput::new(),
            mode: Some(Mode::Normal { count: None }),
        }
    }

    #[test]
    fn count_accumulates_decimal_digits() {
        let mut m = modal_model();
        m.push_count_digit(1);
        m.push_count_digit(0);
        assert!(m.count_pending());
        assert_eq!(m.take_count(), 10);
        assert!(!m.count_pending(), "take resets the count");
    }

    #[test]
    fn take_count_defaults_to_one() {
        let mut m = modal_model();
        assert_eq!(m.take_count(), 1);
    }

    #[test]
    fn clear_count_discards_pending() {
        let mut m = modal_model();
        m.push_count_digit(9);
        m.clear_count();
        assert!(!m.count_pending());
        assert_eq!(m.take_count(), 1);
    }

    #[test]
    fn mode_transitions_stay_within_modal() {
        let mut m = modal_model();
        m.enter_search();
        assert_eq!(m.mode(), Some(Mode::Search));
        m.enter_normal();
        assert_eq!(m.mode(), Some(Mode::Normal { count: None }));
    }

    #[test]
    fn non_modal_ignores_mode_mutations() {
        let mut m = modal_model();
        m.mode = None;
        m.enter_normal(); // no-op on a non-modal model
        assert_eq!(m.mode(), None);
        assert_eq!(m.take_count(), 1);
    }
}
