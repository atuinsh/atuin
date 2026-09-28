//! The picker's actions and default keymaps, built on the shared [`atuin_client::tui`] keymap.
//!
//! The bindings mirror the history search's defaults wherever an action exists in both, so muscle
//! memory carries over: ctrl-r cycles the filter, ctrl-o toggles Inspect, tab edits, enter follows
//! `enter_accept`, and vim users get normal/insert modes. Where a chosen session resumes (its own
//! harness, or continued in another) is asked by the chooser, which has keys of its own (see
//! [`super::chooser`]).

use atuin_client::settings::{KeymapMode, Settings};
use atuin_client::tui::{ConditionAtom, KeyInput, KeyRule};

pub type Keymap = atuin_client::tui::Keymap<Action>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    ClearToEnd,

    // List navigation (in visual terms; invert is handled when executing)
    SelectNext,
    SelectPrevious,
    ScrollPageUp,
    ScrollPageDown,
    ScrollHalfPageUp,
    ScrollHalfPageDown,
    ScrollToTop,
    ScrollToBottom,

    /// Resume the selected session now.
    Resume,
    /// Put the resume command on the command line without running it.
    ReturnCommand,
    /// Copy the resume command to the clipboard.
    Copy,
    ReturnOriginal,
    Exit,
    Redraw,
    CycleFilterMode,
    CycleHarness,
    ToggleTab,

    // Mode changes
    VimEnterNormal,
    VimEnterInsert,
    VimEnterInsertAfter,
    VimEnterInsertAtStart,
    VimEnterInsertAtEnd,
    VimSearchInsert,
    VimChangeToEnd,

    Noop,
}

#[derive(Debug, Clone)]
pub struct KeymapSet {
    pub emacs: Keymap,
    pub vim_normal: Keymap,
    pub vim_insert: Keymap,
    pub inspector: Keymap,
}

impl KeymapSet {
    pub fn defaults(settings: &Settings) -> Self {
        Self {
            emacs: emacs(settings),
            vim_normal: vim_normal(settings),
            vim_insert: vim_insert(settings),
            inspector: inspector(settings),
        }
    }

    /// The search-tab keymap for a keymap mode.
    pub fn for_mode(&self, mode: KeymapMode) -> &Keymap {
        match mode {
            KeymapMode::Emacs | KeymapMode::Auto => &self.emacs,
            KeymapMode::VimNormal => &self.vim_normal,
            KeymapMode::VimInsert => &self.vim_insert,
        }
    }
}

fn key(s: &str) -> KeyInput {
    KeyInput::parse(s).unwrap_or_else(|e| panic!("invalid default key {s:?}: {e}"))
}

fn enter_action(settings: &Settings) -> Action {
    if settings.enter_accept {
        Action::Resume
    } else {
        Action::ReturnCommand
    }
}

/// Bindings every tab shares.
fn add_common(km: &mut Keymap, settings: &Settings) {
    km.bind(key("ctrl-c"), Action::ReturnOriginal);
    km.bind(key("ctrl-g"), Action::ReturnOriginal);
    km.bind(key("ctrl-o"), Action::ToggleTab);
    km.bind(key("tab"), Action::ReturnCommand);
    km.bind(key("ctrl-y"), Action::Copy);
    km.bind(key("ctrl-r"), Action::CycleFilterMode);
    km.bind(key("alt-h"), Action::CycleHarness);
    km.bind(key("ctrl-l"), Action::Redraw);
    km.bind(key("enter"), enter_action(settings));
    km.bind(key("ctrl-m"), enter_action(settings));
    km.bind(key("up"), Action::SelectPrevious);
    km.bind(key("down"), Action::SelectNext);
    km.bind(key("pageup"), Action::ScrollPageUp);
    km.bind(key("pagedown"), Action::ScrollPageDown);
}

pub fn emacs(settings: &Settings) -> Keymap {
    let mut km = Keymap::new();
    add_common(&mut km, settings);

    km.bind(key("esc"), Action::Exit);
    km.bind(key("ctrl-["), Action::Exit);

    km.bind(key("left"), Action::CursorLeft);
    km.bind(key("right"), Action::CursorRight);
    km.bind(key("ctrl-b"), Action::CursorLeft);
    km.bind(key("ctrl-f"), Action::CursorRight);
    km.bind(key("ctrl-left"), Action::CursorWordLeft);
    km.bind(key("alt-b"), Action::CursorWordLeft);
    km.bind(key("ctrl-right"), Action::CursorWordRight);
    km.bind(key("alt-f"), Action::CursorWordRight);
    km.bind(key("home"), Action::CursorStart);
    km.bind(key("ctrl-a"), Action::CursorStart);
    km.bind(key("end"), Action::CursorEnd);
    km.bind(key("ctrl-e"), Action::CursorEnd);

    km.bind(key("backspace"), Action::DeleteCharBefore);
    km.bind(key("ctrl-h"), Action::DeleteCharBefore);
    km.bind(key("ctrl-?"), Action::DeleteCharBefore);
    km.bind(key("ctrl-backspace"), Action::DeleteWordBefore);
    km.bind(key("alt-backspace"), Action::DeleteWordBefore);
    km.bind(key("delete"), Action::DeleteCharAfter);
    km.bind(key("ctrl-delete"), Action::DeleteWordAfter);
    km.bind(key("alt-d"), Action::DeleteWordAfter);
    km.bind_conditional(key("ctrl-d"), vec![
        KeyRule::when(ConditionAtom::InputEmpty, Action::ReturnOriginal),
        KeyRule::always(Action::DeleteCharAfter),
    ]);
    km.bind(key("ctrl-w"), Action::DeleteToWordBoundary);
    km.bind(key("ctrl-u"), Action::ClearLine);
    km.bind(key("ctrl-k"), Action::ClearToEnd);

    km.bind(key("ctrl-n"), Action::SelectNext);
    km.bind(key("ctrl-j"), Action::SelectNext);
    km.bind(key("ctrl-p"), Action::SelectPrevious);

    km
}

pub fn vim_normal(settings: &Settings) -> Keymap {
    let mut km = Keymap::new();
    add_common(&mut km, settings);

    km.bind(key("esc"), Action::Exit);
    km.bind(key("ctrl-["), Action::Exit);

    km.bind(key("j"), Action::SelectNext);
    km.bind(key("k"), Action::SelectPrevious);
    km.bind(key("h"), Action::CursorLeft);
    km.bind(key("l"), Action::CursorRight);
    km.bind(key("0"), Action::CursorStart);
    km.bind(key("$"), Action::CursorEnd);
    km.bind(key("w"), Action::CursorWordRight);
    km.bind(key("b"), Action::CursorWordLeft);
    km.bind(key("e"), Action::CursorWordEnd);

    km.bind(key("x"), Action::DeleteCharAfter);
    km.bind(key("d d"), Action::ClearLine);
    km.bind(key("D"), Action::ClearToEnd);
    km.bind(key("C"), Action::VimChangeToEnd);

    km.bind(key("?"), Action::VimSearchInsert);
    km.bind(key("/"), Action::VimSearchInsert);
    km.bind(key("a"), Action::VimEnterInsertAfter);
    km.bind(key("A"), Action::VimEnterInsertAtEnd);
    km.bind(key("i"), Action::VimEnterInsert);
    km.bind(key("I"), Action::VimEnterInsertAtStart);

    km.bind(key("ctrl-u"), Action::ScrollHalfPageUp);
    km.bind(key("ctrl-d"), Action::ScrollHalfPageDown);
    km.bind(key("ctrl-b"), Action::ScrollPageUp);
    km.bind(key("ctrl-f"), Action::ScrollPageDown);
    km.bind(key("G"), Action::ScrollToBottom);
    km.bind(key("g g"), Action::ScrollToTop);

    km
}

pub fn vim_insert(settings: &Settings) -> Keymap {
    let mut km = emacs(settings);
    km.bind(key("esc"), Action::VimEnterNormal);
    km.bind(key("ctrl-["), Action::VimEnterNormal);
    km
}

/// The Inspect tab has no text input: esc goes back to the list, and vim users get j/k.
pub fn inspector(settings: &Settings) -> Keymap {
    let mut km = Keymap::new();
    add_common(&mut km, settings);
    km.bind(key("esc"), Action::Exit);
    km.bind(key("ctrl-["), Action::Exit);
    km.bind(key("q"), Action::Exit);
    if matches!(settings.keymap_mode, KeymapMode::VimNormal | KeymapMode::VimInsert) {
        km.bind(key("j"), Action::SelectNext);
        km.bind(key("k"), Action::SelectPrevious);
    }
    km
}

#[cfg(test)]
mod tests {
    use atuin_client::tui::EvalContext;
    use rstest::rstest;

    use super::*;

    fn ctx(input_len: usize) -> EvalContext {
        EvalContext {
            cursor_position: input_len,
            input_width: input_len,
            input_byte_len: input_len,
            selected_index: 0,
            results_len: 3,
            original_input_empty: true,
            has_context: false,
        }
    }

    fn resolve(km: &Keymap, k: &str) -> Option<Action> {
        km.resolve(&key(k), &ctx(3))
    }

    #[rstest]
    fn shared_bindings_match_the_history_search() {
        let settings = Settings::utc();
        for km in [emacs(&settings), vim_normal(&settings), vim_insert(&settings)] {
            assert_eq!(resolve(&km, "ctrl-r"), Some(Action::CycleFilterMode));
            assert_eq!(resolve(&km, "alt-h"), Some(Action::CycleHarness));
            assert_eq!(resolve(&km, "ctrl-y"), Some(Action::Copy));
            assert_eq!(resolve(&km, "ctrl-o"), Some(Action::ToggleTab));
            assert_eq!(resolve(&km, "tab"), Some(Action::ReturnCommand));
            assert_eq!(resolve(&km, "ctrl-c"), Some(Action::ReturnOriginal));
            assert_eq!(resolve(&km, "ctrl-g"), Some(Action::ReturnOriginal));
        }
    }

    #[rstest]
    fn enter_follows_enter_accept() {
        let mut settings = Settings::utc();
        settings.enter_accept = true;
        assert_eq!(resolve(&emacs(&settings), "enter"), Some(Action::Resume));
        settings.enter_accept = false;
        assert_eq!(resolve(&emacs(&settings), "enter"), Some(Action::ReturnCommand));
        assert_eq!(resolve(&inspector(&settings), "enter"), Some(Action::ReturnCommand));
    }

    #[rstest]
    fn esc_exits_emacs_but_enters_normal_mode_from_insert() {
        let settings = Settings::utc();
        assert_eq!(resolve(&emacs(&settings), "esc"), Some(Action::Exit));
        assert_eq!(resolve(&vim_insert(&settings), "esc"), Some(Action::VimEnterNormal));
        assert_eq!(resolve(&vim_normal(&settings), "esc"), Some(Action::Exit));
    }

    #[rstest]
    fn ctrl_d_returns_original_only_on_empty_input() {
        let km = emacs(&Settings::utc());
        assert_eq!(km.resolve(&key("ctrl-d"), &ctx(0)), Some(Action::ReturnOriginal));
        assert_eq!(km.resolve(&key("ctrl-d"), &ctx(2)), Some(Action::DeleteCharAfter));
    }

    #[rstest]
    fn vim_normal_has_gg_sequence_and_plain_letters_are_unbound_in_emacs() {
        let settings = Settings::utc();
        let normal = vim_normal(&settings);
        assert_eq!(resolve(&normal, "g g"), Some(Action::ScrollToTop));
        assert!(
            normal.has_sequence_starting_with(&atuin_client::tui::SingleKey::parse("g").unwrap())
        );
        assert_eq!(resolve(&emacs(&settings), "j"), None);
    }
}
