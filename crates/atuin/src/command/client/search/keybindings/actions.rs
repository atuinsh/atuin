use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// All possible actions that can be triggered by a keybinding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    // Cursor movement
    CursorLeft,
    CursorRight,
    CursorWordLeft,
    CursorWordRight,
    CursorWordEnd,
    CursorStart,
    CursorEnd,

    // Editing
    DeleteCharBefore,
    DeleteCharAfter,
    DeleteWordBefore,
    DeleteWordAfter,
    DeleteToWordBoundary,
    ClearLine,
    ClearToStart,
    ClearToEnd,

    // List navigation
    SelectNext,
    SelectPrevious,
    ScrollHalfPageUp,
    ScrollHalfPageDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,
    ScrollToScreenTop,
    ScrollToScreenMiddle,
    ScrollToScreenBottom,

    // Commands — accept selection and execute immediately
    Accept,
    AcceptNth(u8),
    // Commands — return selection to command line without executing
    ReturnSelection,
    ReturnSelectionNth(u8),
    // Commands — other
    Copy,
    Delete,
    DeleteAll,
    ReturnOriginal,
    ReturnQuery,
    Exit,
    Redraw,
    CycleFilterMode,
    CycleSearchMode,
    SwitchContext,
    ClearContext,
    ToggleTab,

    // Mode changes
    VimEnterNormal,
    VimEnterInsert,
    VimEnterInsertAfter,
    VimEnterInsertAtStart,
    VimEnterInsertAtEnd,
    VimSearchInsert,
    VimChangeToEnd,
    EnterPrefixMode,

    // Inspector
    InspectPrevious,
    InspectNext,

    // Special
    Noop,
}

impl Action {
    /// Convert from a kebab-case string.
    pub fn from_str(s: &str) -> Result<Self, String> {
        // Handle accept-N and return-selection-N patterns
        if let Some(rest) = s.strip_prefix("accept-")
            && let Ok(n) = rest.parse::<u8>()
            && (1..=9).contains(&n)
        {
            return Ok(Action::AcceptNth(n));
        }
        if let Some(rest) = s.strip_prefix("return-selection-")
            && let Ok(n) = rest.parse::<u8>()
            && (1..=9).contains(&n)
        {
            return Ok(Action::ReturnSelectionNth(n));
        }

        match s {
            "cursor-left" => Ok(Action::CursorLeft),
            "cursor-right" => Ok(Action::CursorRight),
            "cursor-word-left" => Ok(Action::CursorWordLeft),
            "cursor-word-right" => Ok(Action::CursorWordRight),
            "cursor-word-end" => Ok(Action::CursorWordEnd),
            "cursor-start" => Ok(Action::CursorStart),
            "cursor-end" => Ok(Action::CursorEnd),

            "delete-char-before" => Ok(Action::DeleteCharBefore),
            "delete-char-after" => Ok(Action::DeleteCharAfter),
            "delete-word-before" => Ok(Action::DeleteWordBefore),
            "delete-word-after" => Ok(Action::DeleteWordAfter),
            "delete-to-word-boundary" => Ok(Action::DeleteToWordBoundary),
            "clear-line" => Ok(Action::ClearLine),
            "clear-to-start" => Ok(Action::ClearToStart),
            "clear-to-end" => Ok(Action::ClearToEnd),

            "select-next" => Ok(Action::SelectNext),
            "select-previous" => Ok(Action::SelectPrevious),
            "scroll-half-page-up" => Ok(Action::ScrollHalfPageUp),
            "scroll-half-page-down" => Ok(Action::ScrollHalfPageDown),
            "scroll-page-up" => Ok(Action::ScrollPageUp),
            "scroll-page-down" => Ok(Action::ScrollPageDown),
            "scroll-to-top" => Ok(Action::ScrollToTop),
            "scroll-to-bottom" => Ok(Action::ScrollToBottom),
            "scroll-to-screen-top" => Ok(Action::ScrollToScreenTop),
            "scroll-to-screen-middle" => Ok(Action::ScrollToScreenMiddle),
            "scroll-to-screen-bottom" => Ok(Action::ScrollToScreenBottom),

            "accept" => Ok(Action::Accept),
            "return-selection" => Ok(Action::ReturnSelection),
            "copy" => Ok(Action::Copy),
            "delete" => Ok(Action::Delete),
            "delete-all" => Ok(Action::DeleteAll),
            "return-original" => Ok(Action::ReturnOriginal),
            "return-query" => Ok(Action::ReturnQuery),
            "exit" => Ok(Action::Exit),
            "redraw" => Ok(Action::Redraw),
            "cycle-filter-mode" => Ok(Action::CycleFilterMode),
            "cycle-search-mode" => Ok(Action::CycleSearchMode),
            "switch-context" => Ok(Action::SwitchContext),
            "clear-context" => Ok(Action::ClearContext),
            "toggle-tab" => Ok(Action::ToggleTab),

            "vim-enter-normal" => Ok(Action::VimEnterNormal),
            "vim-enter-insert" => Ok(Action::VimEnterInsert),
            "vim-enter-insert-after" => Ok(Action::VimEnterInsertAfter),
            "vim-enter-insert-at-start" => Ok(Action::VimEnterInsertAtStart),
            "vim-enter-insert-at-end" => Ok(Action::VimEnterInsertAtEnd),
            "vim-search-insert" => Ok(Action::VimSearchInsert),
            "vim-change-to-end" => Ok(Action::VimChangeToEnd),
            "enter-prefix-mode" => Ok(Action::EnterPrefixMode),

            "inspect-previous" => Ok(Action::InspectPrevious),
            "inspect-next" => Ok(Action::InspectNext),

            "noop" => Ok(Action::Noop),

            _ => Err(format!("unknown action: {s}")),
        }
    }

    /// Convert to a kebab-case string.
    pub fn as_str(&self) -> String {
        match self {
            Action::CursorLeft => "cursor-left".to_string(),
            Action::CursorRight => "cursor-right".to_string(),
            Action::CursorWordLeft => "cursor-word-left".to_string(),
            Action::CursorWordRight => "cursor-word-right".to_string(),
            Action::CursorWordEnd => "cursor-word-end".to_string(),
            Action::CursorStart => "cursor-start".to_string(),
            Action::CursorEnd => "cursor-end".to_string(),

            Action::DeleteCharBefore => "delete-char-before".to_string(),
            Action::DeleteCharAfter => "delete-char-after".to_string(),
            Action::DeleteWordBefore => "delete-word-before".to_string(),
            Action::DeleteWordAfter => "delete-word-after".to_string(),
            Action::DeleteToWordBoundary => "delete-to-word-boundary".to_string(),
            Action::ClearLine => "clear-line".to_string(),
            Action::ClearToStart => "clear-to-start".to_string(),
            Action::ClearToEnd => "clear-to-end".to_string(),

            Action::SelectNext => "select-next".to_string(),
            Action::SelectPrevious => "select-previous".to_string(),
            Action::ScrollHalfPageUp => "scroll-half-page-up".to_string(),
            Action::ScrollHalfPageDown => "scroll-half-page-down".to_string(),
            Action::ScrollPageUp => "scroll-page-up".to_string(),
            Action::ScrollPageDown => "scroll-page-down".to_string(),
            Action::ScrollToTop => "scroll-to-top".to_string(),
            Action::ScrollToBottom => "scroll-to-bottom".to_string(),
            Action::ScrollToScreenTop => "scroll-to-screen-top".to_string(),
            Action::ScrollToScreenMiddle => "scroll-to-screen-middle".to_string(),
            Action::ScrollToScreenBottom => "scroll-to-screen-bottom".to_string(),

            Action::Accept => "accept".to_string(),
            Action::AcceptNth(n) => format!("accept-{n}"),
            Action::ReturnSelection => "return-selection".to_string(),
            Action::ReturnSelectionNth(n) => format!("return-selection-{n}"),
            Action::Copy => "copy".to_string(),
            Action::Delete => "delete".to_string(),
            Action::DeleteAll => "delete-all".to_string(),
            Action::ReturnOriginal => "return-original".to_string(),
            Action::ReturnQuery => "return-query".to_string(),
            Action::Exit => "exit".to_string(),
            Action::Redraw => "redraw".to_string(),
            Action::CycleFilterMode => "cycle-filter-mode".to_string(),
            Action::CycleSearchMode => "cycle-search-mode".to_string(),
            Action::SwitchContext => "switch-context".to_string(),
            Action::ClearContext => "clear-context".to_string(),
            Action::ToggleTab => "toggle-tab".to_string(),

            Action::VimEnterNormal => "vim-enter-normal".to_string(),
            Action::VimEnterInsert => "vim-enter-insert".to_string(),
            Action::VimEnterInsertAfter => "vim-enter-insert-after".to_string(),
            Action::VimEnterInsertAtStart => "vim-enter-insert-at-start".to_string(),
            Action::VimEnterInsertAtEnd => "vim-enter-insert-at-end".to_string(),
            Action::VimSearchInsert => "vim-search-insert".to_string(),
            Action::VimChangeToEnd => "vim-change-to-end".to_string(),
            Action::EnterPrefixMode => "enter-prefix-mode".to_string(),

            Action::InspectPrevious => "inspect-previous".to_string(),
            Action::InspectNext => "inspect-next".to_string(),

            Action::Noop => "noop".to_string(),
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Action::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::cursor_left("cursor-left", Action::CursorLeft)]
    #[case::accept("accept", Action::Accept)]
    #[case::exit("exit", Action::Exit)]
    #[case::noop("noop", Action::Noop)]
    #[case::vim_enter_normal("vim-enter-normal", Action::VimEnterNormal)]
    #[case::accept_nth_1("accept-1", Action::AcceptNth(1))]
    #[case::accept_nth_9("accept-9", Action::AcceptNth(9))]
    #[case::return_selection("return-selection", Action::ReturnSelection)]
    #[case::return_selection_1("return-selection-1", Action::ReturnSelectionNth(1))]
    #[case::return_selection_9("return-selection-9", Action::ReturnSelectionNth(9))]
    fn parse_action(#[case] input: &str, #[case] expected: Action) {
        assert_eq!(Action::from_str(input).unwrap(), expected);
    }

    #[rstest]
    #[case::unknown_action("unknown-action")]
    #[case::accept_0("accept-0")]
    #[case::accept_10("accept-10")]
    #[case::return_selection_0("return-selection-0")]
    #[case::return_selection_10("return-selection-10")]
    fn parse_unknown_action(#[case] input: &str) {
        assert!(Action::from_str(input).is_err());
    }

    #[rstest]
    #[case(Action::CursorLeft)]
    #[case(Action::Accept)]
    #[case(Action::AcceptNth(5))]
    #[case(Action::ReturnSelection)]
    #[case(Action::ReturnSelectionNth(3))]
    #[case(Action::VimSearchInsert)]
    #[case(Action::ScrollToScreenMiddle)]
    fn round_trip(#[case] action: Action) {
        assert_eq!(Action::from_str(&action.as_str()).unwrap(), action);
    }

    #[rstest]
    #[case(Action::CursorLeft, "\"cursor-left\"")]
    #[case(Action::AcceptNth(3), "\"accept-3\"")]
    fn serde_round_trip(#[case] action: Action, #[case] json: &str) {
        assert_eq!(serde_json::to_string(&action).unwrap(), json);
        assert_eq!(serde_json::from_str::<Action>(json).unwrap(), action);
    }
}
