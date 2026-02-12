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
    use super::*;

    #[test]
    fn parse_basic_actions() {
        assert_eq!(Action::from_str("cursor-left").unwrap(), Action::CursorLeft);
        assert_eq!(Action::from_str("accept").unwrap(), Action::Accept);
        assert_eq!(Action::from_str("exit").unwrap(), Action::Exit);
        assert_eq!(Action::from_str("noop").unwrap(), Action::Noop);
        assert_eq!(
            Action::from_str("vim-enter-normal").unwrap(),
            Action::VimEnterNormal
        );
    }

    #[test]
    fn parse_accept_nth() {
        assert_eq!(Action::from_str("accept-1").unwrap(), Action::AcceptNth(1));
        assert_eq!(Action::from_str("accept-9").unwrap(), Action::AcceptNth(9));
    }

    #[test]
    fn parse_return_selection() {
        assert_eq!(
            Action::from_str("return-selection").unwrap(),
            Action::ReturnSelection
        );
        assert_eq!(
            Action::from_str("return-selection-1").unwrap(),
            Action::ReturnSelectionNth(1)
        );
        assert_eq!(
            Action::from_str("return-selection-9").unwrap(),
            Action::ReturnSelectionNth(9)
        );
    }

    #[test]
    fn parse_unknown_action() {
        assert!(Action::from_str("unknown-action").is_err());
        assert!(Action::from_str("accept-0").is_err());
        assert!(Action::from_str("accept-10").is_err());
        assert!(Action::from_str("return-selection-0").is_err());
        assert!(Action::from_str("return-selection-10").is_err());
    }

    #[test]
    fn round_trip() {
        let actions = vec![
            Action::CursorLeft,
            Action::Accept,
            Action::AcceptNth(5),
            Action::ReturnSelection,
            Action::ReturnSelectionNth(3),
            Action::VimSearchInsert,
            Action::ScrollToScreenMiddle,
        ];
        for action in actions {
            let s = action.as_str();
            let parsed = Action::from_str(&s).unwrap();
            assert_eq!(action, parsed);
        }
    }

    #[test]
    fn serde_round_trip() {
        let action = Action::CursorLeft;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"cursor-left\"");
        let parsed: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Action::CursorLeft);

        let action = Action::AcceptNth(3);
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"accept-3\"");
        let parsed: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Action::AcceptNth(3));
    }
}
