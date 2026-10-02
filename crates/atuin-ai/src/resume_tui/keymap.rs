//! The picker's actions and default keymaps, over the key names the history search parses
//! ([`atuin_client::tui::key`]).
//!
//! The bindings mirror the history search's defaults wherever an action exists in both, so muscle
//! memory carries over: ctrl-r cycles the filter, ctrl-o toggles Inspect, tab edits, enter follows
//! `enter_accept`, and vim users get normal/insert modes. Where a chosen session resumes (its own
//! harness, or continued in another) is asked by the chooser, which has keys of its own (see
//! [`super::chooser`]).

use std::collections::HashMap;

use atuin_client::settings::{KeymapMode, Settings};
use atuin_client::tui::key::{KeyInput, SingleKey};

/// What a key does: its action, or another one while the query is empty.
#[derive(Debug, Clone, Copy)]
struct Binding {
    action: Action,
    when_input_empty: Option<Action>,
}

/// The keys bound in one mode.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: HashMap<KeyInput, Binding>,
}

impl Keymap {
    fn bind(&mut self, key: KeyInput, action: Action) {
        self.bindings.insert(key, Binding {
            action,
            when_input_empty: None,
        });
    }

    /// Bind `key` to `empty` while the query is empty, and to `action` otherwise.
    fn bind_when_input_empty(&mut self, key: KeyInput, empty: Action, action: Action) {
        self.bindings.insert(key, Binding {
            action,
            when_input_empty: Some(empty),
        });
    }

    /// What `key` does, given whether the query is empty.
    pub fn resolve(&self, key: &KeyInput, input_empty: bool) -> Option<Action> {
        let binding = self.bindings.get(key)?;
        Some(binding.when_input_empty.filter(|_| input_empty).unwrap_or(binding.action))
    }

    /// Whether a bound sequence starts with `prefix` (so it waits for the next key).
    pub fn has_sequence_starting_with(&self, prefix: &SingleKey) -> bool {
        self.bindings.keys().any(|k| match k {
            KeyInput::Sequence(keys) => keys.first() == Some(prefix),
            KeyInput::Single(_) => false,
        })
    }
}

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

    // The preview (the strip under the list, the pane beside it, or Inspect's conversation)
    /// Scroll the preview's text up a line.
    PreviewUp,
    /// Scroll the preview's text down a line.
    PreviewDown,
    /// Scroll the preview's text up a page.
    PreviewPageUp,
    /// Scroll the preview's text down a page.
    PreviewPageDown,

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
    /// Cycle the query's agent token (`agent:`) through the agents.
    CycleAgent,
    ToggleTab,
    /// Inspect: expand the list of grouped sessions and move in it, or collapse it again.
    ToggleChildren,

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
    km.bind(key("alt-a"), Action::CycleAgent);
    km.bind(key("ctrl-l"), Action::Redraw);
    km.bind(key("enter"), enter_action(settings));
    km.bind(key("ctrl-m"), enter_action(settings));
    km.bind(key("up"), Action::SelectPrevious);
    km.bind(key("down"), Action::SelectNext);
    km.bind(key("pageup"), Action::ScrollPageUp);
    km.bind(key("pagedown"), Action::ScrollPageDown);
    // The preview scrolls with shift (or alt) on the keys that move the selection. Neither is
    // bound in the history search, and terminals that don't send shift-up (some macOS ones take
    // it for themselves) mostly send alt-up.
    km.bind(key("shift-up"), Action::PreviewUp);
    km.bind(key("shift-down"), Action::PreviewDown);
    km.bind(key("alt-up"), Action::PreviewUp);
    km.bind(key("alt-down"), Action::PreviewDown);
    km.bind(key("shift-pageup"), Action::PreviewPageUp);
    km.bind(key("shift-pagedown"), Action::PreviewPageDown);
    km.bind(key("alt-pageup"), Action::PreviewPageUp);
    km.bind(key("alt-pagedown"), Action::PreviewPageDown);
}

pub fn emacs(settings: &Settings) -> Keymap {
    let mut km = Keymap::default();
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
    km.bind_when_input_empty(key("ctrl-d"), Action::ReturnOriginal, Action::DeleteCharAfter);
    km.bind(key("ctrl-w"), Action::DeleteToWordBoundary);
    km.bind(key("ctrl-u"), Action::ClearLine);
    km.bind(key("ctrl-k"), Action::ClearToEnd);

    km.bind(key("ctrl-n"), Action::SelectNext);
    km.bind(key("ctrl-j"), Action::SelectNext);
    km.bind(key("ctrl-p"), Action::SelectPrevious);

    km
}

pub fn vim_normal(settings: &Settings) -> Keymap {
    let mut km = Keymap::default();
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

/// The Inspect tab has no text input: esc goes back to the list, and vim users get j/k. `c`
/// expands the grouped sessions; while it is, up/down (and page up/down, home/end) move in that
/// list instead of between sessions, and esc or `c` collapses it.
pub fn inspector(settings: &Settings) -> Keymap {
    let mut km = Keymap::default();
    add_common(&mut km, settings);
    km.bind(key("esc"), Action::Exit);
    km.bind(key("ctrl-["), Action::Exit);
    km.bind(key("q"), Action::Exit);
    km.bind(key("c"), Action::ToggleChildren);
    km.bind(key("home"), Action::ScrollToTop);
    km.bind(key("end"), Action::ScrollToBottom);
    if matches!(settings.keymap_mode, KeymapMode::VimNormal | KeymapMode::VimInsert) {
        km.bind(key("j"), Action::SelectNext);
        km.bind(key("k"), Action::SelectPrevious);
    }
    km
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    fn resolve(km: &Keymap, k: &str) -> Option<Action> {
        km.resolve(&key(k), false)
    }

    #[rstest]
    fn shared_bindings_match_the_history_search() {
        let settings = Settings::utc();
        for km in [emacs(&settings), vim_normal(&settings), vim_insert(&settings)] {
            assert_eq!(resolve(&km, "ctrl-r"), Some(Action::CycleFilterMode));
            assert_eq!(resolve(&km, "alt-a"), Some(Action::CycleAgent));
            assert_eq!(resolve(&km, "ctrl-y"), Some(Action::Copy));
            assert_eq!(resolve(&km, "ctrl-o"), Some(Action::ToggleTab));
            assert_eq!(resolve(&km, "tab"), Some(Action::ReturnCommand));
            assert_eq!(resolve(&km, "ctrl-c"), Some(Action::ReturnOriginal));
            assert_eq!(resolve(&km, "ctrl-g"), Some(Action::ReturnOriginal));
        }
    }

    /// alt-a cycles the agent in every mode, Inspect and vim's both modes included, and is the
    /// only key that does; alt-h no longer does anything.
    #[rstest]
    #[case(KeymapMode::Emacs)]
    #[case(KeymapMode::VimNormal)]
    #[case(KeymapMode::VimInsert)]
    fn alt_a_cycles_the_agent_everywhere(#[case] mode: KeymapMode) {
        let mut settings = Settings::utc();
        settings.keymap_mode = mode;
        let set = KeymapSet::defaults(&settings);
        for km in [set.for_mode(mode), &set.inspector] {
            assert_eq!(resolve(km, "alt-a"), Some(Action::CycleAgent));
            assert_eq!(km.resolve(&key("alt-a"), true), Some(Action::CycleAgent));
            assert_eq!(resolve(km, "alt-h"), None);
            let cycling: Vec<_> =
                km.bindings.iter().filter(|(_, b)| b.action == Action::CycleAgent).collect();
            assert_eq!(cycling.len(), 1, "{cycling:?}");
        }
    }

    /// The preview's keys are the same everywhere, Inspect included, and take nothing the
    /// selection or the input uses.
    #[rstest]
    fn preview_keys_are_shared_and_free() {
        let settings = Settings::utc();
        for km in
            [emacs(&settings), vim_normal(&settings), vim_insert(&settings), inspector(&settings)]
        {
            for (k, action) in [
                ("shift-up", Action::PreviewUp),
                ("alt-up", Action::PreviewUp),
                ("shift-down", Action::PreviewDown),
                ("alt-down", Action::PreviewDown),
                ("shift-pageup", Action::PreviewPageUp),
                ("shift-pagedown", Action::PreviewPageDown),
                ("alt-pageup", Action::PreviewPageUp),
                ("alt-pagedown", Action::PreviewPageDown),
            ] {
                assert_eq!(resolve(&km, k), Some(action), "{k}");
            }
            assert_eq!(resolve(&km, "up"), Some(Action::SelectPrevious));
            assert_eq!(resolve(&km, "pagedown"), Some(Action::ScrollPageDown));
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
        assert_eq!(km.resolve(&key("ctrl-d"), true), Some(Action::ReturnOriginal));
        assert_eq!(km.resolve(&key("ctrl-d"), false), Some(Action::DeleteCharAfter));
    }

    #[rstest]
    fn vim_normal_has_gg_sequence_and_plain_letters_are_unbound_in_emacs() {
        let settings = Settings::utc();
        let normal = vim_normal(&settings);
        assert_eq!(resolve(&normal, "g g"), Some(Action::ScrollToTop));
        assert!(normal.has_sequence_starting_with(&SingleKey::parse("g").unwrap()));
        assert_eq!(resolve(&emacs(&settings), "j"), None);
    }
}
