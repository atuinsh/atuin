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
    use rstest::{fixture, rstest};

    /// A modal model starting in NORMAL with no pending count.
    #[fixture]
    fn modal() -> Model {
        Model {
            theme: Theme::default(),
            enter_accept: true,
            history: HistoryList::new(),
            search: SearchInput::new(),
            mode: Some(Mode::Normal { count: None }),
        }
    }

    #[rstest]
    #[case::single(&[5], 5)]
    #[case::two_digits(&[1, 0], 10)]
    #[case::three_digits(&[2, 5, 0], 250)]
    fn take_count_reads_the_accumulated_digits(
        mut modal: Model,
        #[case] digits: &[u8],
        #[case] expected: usize,
    ) {
        for &d in digits {
            modal.push_count_digit(d);
        }
        assert!(modal.count_pending());
        assert_eq!(modal.take_count(), expected);
        assert!(!modal.count_pending(), "take resets the count");
    }

    #[rstest]
    fn take_count_defaults_to_one(mut modal: Model) {
        assert_eq!(modal.take_count(), 1);
    }

    #[rstest]
    fn clear_count_discards_pending(mut modal: Model) {
        modal.push_count_digit(9);
        modal.clear_count();
        assert!(!modal.count_pending());
        assert_eq!(modal.take_count(), 1);
    }

    #[rstest]
    fn mode_transitions_stay_within_modal(mut modal: Model) {
        modal.enter_search();
        assert_eq!(modal.mode(), Some(Mode::Search));
        modal.enter_normal();
        assert_eq!(modal.mode(), Some(Mode::Normal { count: None }));
    }

    #[rstest]
    fn non_modal_ignores_mode_mutations(mut modal: Model) {
        modal.mode = None;
        modal.enter_normal(); // no-op on a non-modal model
        assert_eq!(modal.mode(), None);
        assert_eq!(modal.take_count(), 1);
    }
}
