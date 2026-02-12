use std::collections::HashMap;

use atuin_client::settings::{KeyBindingConfig, Settings};
use tracing::warn;

use super::actions::Action;
use super::conditions::{ConditionAtom, ConditionExpr};
use super::key::KeyInput;
use super::keymap::{KeyBinding, KeyRule, Keymap};

/// Helper to bind a scroll key with optional exit behavior.
///
/// When `scroll_exits` is true AND the key scrolls toward index 0 (the newest
/// entry), we add a conditional rule: at `ListAtStart` → `Exit`, otherwise →
/// the scroll action.
///
/// Whether a key scrolls toward index 0 depends on the `invert` setting:
/// - Non-inverted: "down" / "j" move toward index 0, "up" / "k" move away
/// - Inverted: "up" / "k" move toward index 0, "down" / "j" move away
///
/// If `toward_index_zero` is false, or `scroll_exits` is false, we just bind
/// the key to the plain scroll action (no exit).
fn bind_scroll_key(
    km: &mut Keymap,
    key_str: &str,
    action: Action,
    toward_index_zero: bool,
    scroll_exits: bool,
) {
    let k = key(key_str);
    if scroll_exits && toward_index_zero {
        km.bind_conditional(
            k,
            vec![
                KeyRule::when(ConditionAtom::ListAtStart, Action::Exit),
                KeyRule::always(action),
            ],
        );
    } else {
        km.bind(k, action);
    }
}

/// Helper to parse a key string, panicking on invalid keys (these are all
/// compile-time-known strings).
fn key(s: &str) -> KeyInput {
    KeyInput::parse(s).unwrap_or_else(|e| panic!("invalid default key {s:?}: {e}"))
}

/// All five keymaps bundled together.
#[derive(Debug, Clone)]
pub struct KeymapSet {
    pub emacs: Keymap,
    pub vim_normal: Keymap,
    pub vim_insert: Keymap,
    pub inspector: Keymap,
    pub prefix: Keymap,
}

// ---------------------------------------------------------------------------
// Common bindings shared across search-tab keymaps
// ---------------------------------------------------------------------------

/// Add the bindings that are common to all search-tab keymaps:
/// ctrl-c, ctrl-g, ctrl-o, and tab.
///
/// Note: `esc`/`ctrl-[` are NOT included here because their behavior differs
/// between emacs (exit), vim-normal (exit), and vim-insert (enter normal mode).
fn add_common_bindings(km: &mut Keymap) {
    km.bind(key("ctrl-c"), Action::ReturnOriginal);
    km.bind(key("ctrl-g"), Action::ReturnOriginal);
    km.bind(key("ctrl-o"), Action::ToggleTab);

    // Tab: always returns selection without executing (unlike Enter which respects enter_accept)
    km.bind(key("tab"), Action::ReturnSelection);
}

/// Returns `Accept` or `ReturnSelection` based on the `enter_accept` setting.
fn accept_action(settings: &Settings) -> Action {
    if settings.enter_accept {
        Action::Accept
    } else {
        Action::ReturnSelection
    }
}

// ---------------------------------------------------------------------------
// Emacs keymap (also base for vim-insert)
// ---------------------------------------------------------------------------

/// Build the default emacs keymap. This encodes the behavior from
/// `handle_key_input` common section + `handle_search_input` shared section.
///
/// The `settings` parameter is used for:
/// - `keys.prefix` — which ctrl-key enters prefix mode
/// - `keys.scroll_exits`, `invert` — scroll-at-boundary exit behavior
/// - `keys.accept_past_line_end` — right arrow at end of line accepts
/// - `keys.exit_past_line_start` — left arrow at start of line exits
/// - `keys.accept_past_line_start` — left arrow at start accepts (overrides exit)
/// - `keys.accept_with_backspace` — backspace at start of line accepts
/// - `ctrl_n_shortcuts` — whether alt or ctrl is used for numeric shortcuts
// Keymap builder that enumerates every default binding; not worth splitting.
#[allow(clippy::too_many_lines)]
pub fn default_emacs_keymap(settings: &Settings) -> Keymap {
    let mut km = Keymap::new();
    add_common_bindings(&mut km);

    let accept = accept_action(settings);

    // esc / ctrl-[ → exit
    km.bind(key("esc"), Action::Exit);
    km.bind(key("ctrl-["), Action::Exit);

    // Prefix key: ctrl-<prefix_char> → enter prefix mode
    let prefix_char = settings.keys.prefix.chars().next().unwrap_or('a');
    km.bind(key(&format!("ctrl-{prefix_char}")), Action::EnterPrefixMode);

    // --- Accept / navigation edge behaviors (from [keys] settings) ---

    // right: behavior at end of line
    if settings.keys.accept_past_line_end {
        km.bind_conditional(
            key("right"),
            vec![
                KeyRule::when(ConditionAtom::CursorAtEnd, Action::ReturnSelection),
                KeyRule::always(Action::CursorRight),
            ],
        );
    } else {
        km.bind(key("right"), Action::CursorRight);
    }

    // left: behavior at start of line
    // accept_past_line_start takes precedence over exit_past_line_start
    if settings.keys.accept_past_line_start {
        km.bind_conditional(
            key("left"),
            vec![
                KeyRule::when(ConditionAtom::CursorAtStart, Action::ReturnSelection),
                KeyRule::always(Action::CursorLeft),
            ],
        );
    } else if settings.keys.exit_past_line_start {
        km.bind_conditional(
            key("left"),
            vec![
                KeyRule::when(ConditionAtom::CursorAtStart, Action::Exit),
                KeyRule::always(Action::CursorLeft),
            ],
        );
    } else {
        km.bind(key("left"), Action::CursorLeft);
    }

    // down/up: scroll with optional exit at boundary.
    // Non-inverted: down moves toward index 0 (can exit); up moves away (no exit).
    // Inverted: up moves toward index 0 (can exit); down moves away (no exit).
    let scroll_exits = settings.keys.scroll_exits;
    let invert = settings.invert;
    bind_scroll_key(&mut km, "down", Action::SelectNext, !invert, scroll_exits);
    bind_scroll_key(&mut km, "up", Action::SelectPrevious, invert, scroll_exits);

    // backspace: behavior at start of line
    if settings.keys.accept_with_backspace {
        km.bind_conditional(
            key("backspace"),
            vec![
                KeyRule::when(ConditionAtom::CursorAtStart, Action::ReturnSelection),
                KeyRule::always(Action::DeleteCharBefore),
            ],
        );
    } else {
        km.bind(key("backspace"), Action::DeleteCharBefore);
    }

    // --- Accept ---
    km.bind(key("enter"), accept.clone());
    km.bind(key("ctrl-m"), accept);

    // --- Copy ---
    km.bind(key("ctrl-y"), Action::Copy);

    // --- Numeric shortcuts (alt-1..9 by default, ctrl-1..9 if ctrl_n_shortcuts) ---
    // These return the selection without executing, regardless of enter_accept.
    let num_mod = if settings.ctrl_n_shortcuts {
        "ctrl"
    } else {
        "alt"
    };
    for n in 1..=9u8 {
        km.bind(
            key(&format!("{num_mod}-{n}")),
            Action::ReturnSelectionNth(n),
        );
    }

    // --- Cursor movement ---
    km.bind(key("ctrl-left"), Action::CursorWordLeft);
    km.bind(key("alt-b"), Action::CursorWordLeft);
    km.bind(key("ctrl-b"), Action::CursorLeft);
    km.bind(key("ctrl-right"), Action::CursorWordRight);
    km.bind(key("alt-f"), Action::CursorWordRight);
    km.bind(key("ctrl-f"), Action::CursorRight);
    km.bind(key("home"), Action::CursorStart);
    // ctrl-a → CursorStart only if prefix char is NOT 'a'
    // (otherwise ctrl-a is already bound to EnterPrefixMode above)
    if prefix_char != 'a' {
        km.bind(key("ctrl-a"), Action::CursorStart);
    }
    km.bind(key("ctrl-e"), Action::CursorEnd);
    km.bind(key("end"), Action::CursorEnd);

    // --- Editing ---
    km.bind(key("ctrl-backspace"), Action::DeleteWordBefore);
    km.bind(key("ctrl-h"), Action::DeleteCharBefore);
    km.bind(key("ctrl-?"), Action::DeleteCharBefore);
    km.bind(key("ctrl-delete"), Action::DeleteWordAfter);
    km.bind(key("delete"), Action::DeleteCharAfter);
    // ctrl-d: if input empty → return original, otherwise delete char
    km.bind_conditional(
        key("ctrl-d"),
        vec![
            KeyRule::when(ConditionAtom::InputEmpty, Action::ReturnOriginal),
            KeyRule::always(Action::DeleteCharAfter),
        ],
    );
    km.bind(key("ctrl-w"), Action::DeleteToWordBoundary);
    km.bind(key("ctrl-u"), Action::ClearLine);

    // --- Search mode ---
    km.bind(key("ctrl-r"), Action::CycleFilterMode);
    km.bind(key("ctrl-s"), Action::CycleSearchMode);

    // --- Scroll (no exit) ---
    km.bind(key("ctrl-n"), Action::SelectNext);
    km.bind(key("ctrl-j"), Action::SelectNext);
    km.bind(key("ctrl-p"), Action::SelectPrevious);
    km.bind(key("ctrl-k"), Action::SelectPrevious);

    // --- Redraw ---
    km.bind(key("ctrl-l"), Action::Redraw);

    // --- Page scroll ---
    km.bind(key("pagedown"), Action::ScrollPageDown);
    km.bind(key("pageup"), Action::ScrollPageUp);

    km
}

// ---------------------------------------------------------------------------
// Vim Normal keymap
// ---------------------------------------------------------------------------

/// Build the default vim-normal keymap.
pub fn default_vim_normal_keymap(settings: &Settings) -> Keymap {
    let mut km = Keymap::new();
    add_common_bindings(&mut km);

    // esc / ctrl-[ → exit (vim-normal exits, unlike vim-insert)
    km.bind(key("esc"), Action::Exit);
    km.bind(key("ctrl-["), Action::Exit);

    // Prefix key
    let prefix_char = settings.keys.prefix.chars().next().unwrap_or('a');
    km.bind(key(&format!("ctrl-{prefix_char}")), Action::EnterPrefixMode);

    // --- Vim navigation ---
    // j/k: scroll with optional exit at boundary.
    let scroll_exits = settings.keys.scroll_exits;
    let invert = settings.invert;
    bind_scroll_key(&mut km, "j", Action::SelectNext, !invert, scroll_exits);
    bind_scroll_key(&mut km, "k", Action::SelectPrevious, invert, scroll_exits);
    km.bind(key("h"), Action::CursorLeft);
    km.bind(key("l"), Action::CursorRight);

    // --- Vim cursor movement ---
    km.bind(key("0"), Action::CursorStart);
    km.bind(key("$"), Action::CursorEnd);
    km.bind(key("w"), Action::CursorWordRight);
    km.bind(key("b"), Action::CursorWordLeft);
    km.bind(key("e"), Action::CursorWordEnd);

    // --- Vim editing ---
    km.bind(key("x"), Action::DeleteCharAfter);
    km.bind(key("d d"), Action::ClearLine);
    km.bind(key("D"), Action::ClearToEnd);
    km.bind(key("C"), Action::VimChangeToEnd);

    // --- Mode switching ---
    km.bind(key("?"), Action::VimSearchInsert);
    km.bind(key("/"), Action::VimSearchInsert);
    km.bind(key("a"), Action::VimEnterInsertAfter);
    km.bind(key("A"), Action::VimEnterInsertAtEnd);
    km.bind(key("i"), Action::VimEnterInsert);
    km.bind(key("I"), Action::VimEnterInsertAtStart);

    // --- Numeric shortcuts (return selection without executing) ---
    for n in 1..=9u8 {
        km.bind(key(&n.to_string()), Action::ReturnSelectionNth(n));
    }

    // --- Half/full page scroll ---
    km.bind(key("ctrl-u"), Action::ScrollHalfPageUp);
    km.bind(key("ctrl-d"), Action::ScrollHalfPageDown);
    km.bind(key("ctrl-b"), Action::ScrollPageUp);
    km.bind(key("ctrl-f"), Action::ScrollPageDown);

    // --- Jump ---
    km.bind(key("G"), Action::ScrollToBottom);
    km.bind(key("g g"), Action::ScrollToTop);
    km.bind(key("H"), Action::ScrollToScreenTop);
    km.bind(key("M"), Action::ScrollToScreenMiddle);
    km.bind(key("L"), Action::ScrollToScreenBottom);

    // --- Arrow keys (same as emacs for convenience) ---
    bind_scroll_key(&mut km, "down", Action::SelectNext, !invert, scroll_exits);
    bind_scroll_key(&mut km, "up", Action::SelectPrevious, invert, scroll_exits);

    // --- Page scroll ---
    km.bind(key("pagedown"), Action::ScrollPageDown);
    km.bind(key("pageup"), Action::ScrollPageUp);

    // --- Accept ---
    let accept = accept_action(settings);
    km.bind(key("enter"), accept);

    km
}

// ---------------------------------------------------------------------------
// Vim Insert keymap
// ---------------------------------------------------------------------------

/// Build the default vim-insert keymap. This clones the emacs keymap and
/// overlays vim-insert-specific bindings (esc → enter normal mode).
pub fn default_vim_insert_keymap(settings: &Settings) -> Keymap {
    let mut km = default_emacs_keymap(settings);

    // Override esc and ctrl-[ to enter normal mode instead of exiting
    km.bind(key("esc"), Action::VimEnterNormal);
    km.bind(key("ctrl-["), Action::VimEnterNormal);

    km
}

// ---------------------------------------------------------------------------
// Inspector keymap
// ---------------------------------------------------------------------------

/// Build the default inspector keymap (tab index 1).
///
/// The inspector shows details about the selected history item and has no
/// text input, so we build a minimal keymap with only inspector-relevant
/// bindings. We respect the user's `keymap_mode` to provide vim-style j/k
/// navigation for vim users.
pub fn default_inspector_keymap(settings: &Settings) -> Keymap {
    use atuin_client::settings::KeymapMode;

    let mut km = Keymap::new();

    // Common bindings (same as search tab)
    km.bind(key("ctrl-c"), Action::ReturnOriginal);
    km.bind(key("ctrl-g"), Action::ReturnOriginal);
    km.bind(key("esc"), Action::Exit);
    km.bind(key("ctrl-["), Action::Exit);
    km.bind(key("tab"), Action::ReturnSelection);
    km.bind(key("ctrl-o"), Action::ToggleTab);

    // Accept behavior respects enter_accept setting
    let accept = if settings.enter_accept {
        Action::Accept
    } else {
        Action::ReturnSelection
    };
    km.bind(key("enter"), accept);

    // Inspector-specific: delete history entry
    km.bind(key("ctrl-d"), Action::Delete);

    // Inspector navigation
    km.bind(key("up"), Action::InspectPrevious);
    km.bind(key("down"), Action::InspectNext);
    km.bind(key("pageup"), Action::InspectPrevious);
    km.bind(key("pagedown"), Action::InspectNext);

    // For vim users, add j/k navigation
    if matches!(
        settings.keymap_mode,
        KeymapMode::VimNormal | KeymapMode::VimInsert
    ) {
        km.bind(key("j"), Action::InspectNext);
        km.bind(key("k"), Action::InspectPrevious);
    }

    km
}

// ---------------------------------------------------------------------------
// Prefix keymap
// ---------------------------------------------------------------------------

/// Build the default prefix keymap (active after ctrl-a prefix).
pub fn default_prefix_keymap() -> Keymap {
    let mut km = Keymap::new();

    km.bind(key("d"), Action::Delete);
    km.bind(key("a"), Action::CursorStart);
    km.bind_conditional(
        key("c"),
        vec![
            KeyRule::when(ConditionAtom::HasContext, Action::ClearContext),
            KeyRule::always(Action::SwitchContext),
        ],
    );

    km
}

// ---------------------------------------------------------------------------
// KeymapSet construction
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Config → Keymap conversion
// ---------------------------------------------------------------------------

/// Convert a `KeyBindingConfig` (from TOML) into a `KeyBinding`.
/// Returns `Err` if an action name or condition expression is invalid.
fn parse_binding_config(config: &KeyBindingConfig) -> Result<KeyBinding, String> {
    match config {
        KeyBindingConfig::Simple(action_str) => {
            let action = Action::from_str(action_str)?;
            Ok(KeyBinding::simple(action))
        }
        KeyBindingConfig::Rules(rules) => {
            let mut parsed_rules = Vec::with_capacity(rules.len());
            for rule_cfg in rules {
                let action = Action::from_str(&rule_cfg.action)?;
                let rule = match &rule_cfg.when {
                    None => KeyRule::always(action),
                    Some(cond_str) => {
                        let cond = ConditionExpr::parse(cond_str)?;
                        KeyRule::when(cond, action)
                    }
                };
                parsed_rules.push(rule);
            }
            Ok(KeyBinding::conditional(parsed_rules))
        }
    }
}

/// Apply a map of key-string → binding-config overrides to a keymap.
/// Per-key override replaces the entire rule list for that key.
/// Invalid keys or action names are logged and skipped.
fn apply_config_to_keymap(keymap: &mut Keymap, overrides: &HashMap<String, KeyBindingConfig>) {
    for (key_str, binding_cfg) in overrides {
        let key = match KeyInput::parse(key_str) {
            Ok(k) => k,
            Err(e) => {
                warn!("invalid key in keymap config: {key_str:?}: {e}");
                continue;
            }
        };
        match parse_binding_config(binding_cfg) {
            Ok(binding) => {
                keymap.bindings.insert(key, binding);
            }
            Err(e) => {
                warn!("invalid binding for {key_str:?} in keymap config: {e}");
            }
        }
    }
}

impl KeymapSet {
    /// Build the complete set of default keymaps from settings.
    pub fn defaults(settings: &Settings) -> Self {
        KeymapSet {
            emacs: default_emacs_keymap(settings),
            vim_normal: default_vim_normal_keymap(settings),
            vim_insert: default_vim_insert_keymap(settings),
            inspector: default_inspector_keymap(settings),
            prefix: default_prefix_keymap(),
        }
    }

    /// Build keymaps from settings, applying any user `[keymap]` overrides.
    ///
    /// Precedence rules:
    /// - If `[keymap]` has any entries, `[keys]` is **ignored entirely**.
    ///   Defaults are built with standard `[keys]` values, then `[keymap]`
    ///   overrides are applied per-key.
    /// - If `[keymap]` is empty/absent, `[keys]` customizes the defaults
    ///   (current behavior for backward compatibility).
    pub fn from_settings(settings: &Settings) -> Self {
        use atuin_client::settings::Keys;

        if settings.keymap.is_empty() {
            // No [keymap] section → use [keys] to customize defaults
            Self::defaults(settings)
        } else {
            // [keymap] present → ignore [keys], use standard defaults as base
            let mut base_settings = settings.clone();
            base_settings.keys = Keys::standard_defaults();
            let mut set = Self::defaults(&base_settings);
            set.apply_config(settings);
            set
        }
    }

    /// Apply user keymap config overrides to all modes.
    fn apply_config(&mut self, settings: &Settings) {
        let config = &settings.keymap;
        apply_config_to_keymap(&mut self.emacs, &config.emacs);
        apply_config_to_keymap(&mut self.vim_normal, &config.vim_normal);
        apply_config_to_keymap(&mut self.vim_insert, &config.vim_insert);
        apply_config_to_keymap(&mut self.inspector, &config.inspector);
        apply_config_to_keymap(&mut self.prefix, &config.prefix);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::client::search::keybindings::conditions::EvalContext;

    fn make_ctx(cursor: usize, width: usize, selected: usize, len: usize) -> EvalContext {
        EvalContext {
            cursor_position: cursor,
            input_width: width,
            input_byte_len: width,
            selected_index: selected,
            results_len: len,
            original_input_empty: false,
            has_context: false,
        }
    }

    fn default_settings() -> Settings {
        Settings::utc()
    }

    // -- Emacs keymap tests --

    #[test]
    fn emacs_ctrl_c_returns_original() {
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(
            km.resolve(&key("ctrl-c"), &ctx),
            Some(Action::ReturnOriginal)
        );
    }

    #[test]
    fn emacs_esc_exits() {
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("esc"), &ctx), Some(Action::Exit));
    }

    #[test]
    fn emacs_tab_returns_selection() {
        // enter_accept=false in test defaults → ReturnSelection
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("tab"), &ctx), Some(Action::ReturnSelection));
    }

    #[test]
    fn emacs_enter_returns_selection() {
        // enter_accept=false in test defaults → ReturnSelection
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(
            km.resolve(&key("enter"), &ctx),
            Some(Action::ReturnSelection)
        );
    }

    #[test]
    fn emacs_enter_accept_true_uses_accept() {
        let mut settings = default_settings();
        settings.enter_accept = true;
        let km = default_emacs_keymap(&settings);
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("enter"), &ctx), Some(Action::Accept));
        assert_eq!(km.resolve(&key("tab"), &ctx), Some(Action::ReturnSelection));
    }

    #[test]
    fn emacs_right_at_end_returns_selection() {
        let km = default_emacs_keymap(&default_settings());
        // cursor at end of "hello" (width 5)
        let ctx = make_ctx(5, 5, 0, 10);
        assert_eq!(
            km.resolve(&key("right"), &ctx),
            Some(Action::ReturnSelection)
        );
    }

    #[test]
    fn emacs_right_not_at_end_moves() {
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(2, 5, 0, 10);
        assert_eq!(km.resolve(&key("right"), &ctx), Some(Action::CursorRight));
    }

    #[test]
    fn emacs_left_at_start_exits() {
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(0, 5, 0, 10);
        assert_eq!(km.resolve(&key("left"), &ctx), Some(Action::Exit));
    }

    #[test]
    fn emacs_left_not_at_start_moves() {
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(3, 5, 0, 10);
        assert_eq!(km.resolve(&key("left"), &ctx), Some(Action::CursorLeft));
    }

    #[test]
    fn emacs_down_at_start_exits() {
        let km = default_emacs_keymap(&default_settings());
        // selected=0 → ListAtStart → Exit
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("down"), &ctx), Some(Action::Exit));
    }

    #[test]
    fn emacs_down_not_at_start_selects_next() {
        let km = default_emacs_keymap(&default_settings());
        // selected=5 → not at start → SelectNext
        let ctx = make_ctx(0, 0, 5, 10);
        assert_eq!(km.resolve(&key("down"), &ctx), Some(Action::SelectNext));
    }

    #[test]
    fn emacs_up_selects_previous() {
        let km = default_emacs_keymap(&default_settings());
        // Non-inverted: up never exits (moves away from index 0)
        let ctx = make_ctx(0, 0, 5, 10);
        assert_eq!(km.resolve(&key("up"), &ctx), Some(Action::SelectPrevious));
    }

    #[test]
    fn emacs_ctrl_d_empty_returns_original() {
        let km = default_emacs_keymap(&default_settings());
        // input empty (byte_len = 0)
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(
            km.resolve(&key("ctrl-d"), &ctx),
            Some(Action::ReturnOriginal)
        );
    }

    #[test]
    fn emacs_ctrl_d_nonempty_deletes() {
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(2, 5, 0, 10);
        assert_eq!(
            km.resolve(&key("ctrl-d"), &ctx),
            Some(Action::DeleteCharAfter)
        );
    }

    #[test]
    fn emacs_ctrl_n_selects_next_no_exit_condition() {
        let km = default_emacs_keymap(&default_settings());
        // at start, but ctrl-n should NOT exit (no exit condition bound)
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("ctrl-n"), &ctx), Some(Action::SelectNext));
    }

    #[test]
    fn emacs_prefix_key_enters_prefix() {
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(
            km.resolve(&key("ctrl-a"), &ctx),
            Some(Action::EnterPrefixMode)
        );
    }

    #[test]
    fn emacs_home_cursor_start() {
        let km = default_emacs_keymap(&default_settings());
        let ctx = make_ctx(5, 10, 0, 10);
        assert_eq!(km.resolve(&key("home"), &ctx), Some(Action::CursorStart));
    }

    // -- Vim Normal keymap tests --

    #[test]
    fn vim_normal_j_at_start_exits() {
        let km = default_vim_normal_keymap(&default_settings());
        // selected=0 → ListAtStart → Exit (non-inverted: j moves toward index 0)
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("j"), &ctx), Some(Action::Exit));
    }

    #[test]
    fn vim_normal_j_not_at_start_selects_next() {
        let km = default_vim_normal_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 5, 10);
        assert_eq!(km.resolve(&key("j"), &ctx), Some(Action::SelectNext));
    }

    #[test]
    fn vim_normal_k_selects_previous() {
        let km = default_vim_normal_keymap(&default_settings());
        // Non-inverted: k never exits (moves away from index 0)
        let ctx = make_ctx(0, 0, 5, 10);
        assert_eq!(km.resolve(&key("k"), &ctx), Some(Action::SelectPrevious));
    }

    #[test]
    fn vim_normal_i_enters_insert() {
        let km = default_vim_normal_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("i"), &ctx), Some(Action::VimEnterInsert));
    }

    #[test]
    fn vim_normal_slash_search_insert() {
        let km = default_vim_normal_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("/"), &ctx), Some(Action::VimSearchInsert));
    }

    #[test]
    fn vim_normal_gg_scroll_to_top() {
        let km = default_vim_normal_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 50, 100);
        assert_eq!(km.resolve(&key("g g"), &ctx), Some(Action::ScrollToTop));
    }

    #[test]
    fn vim_normal_big_g_scroll_to_bottom() {
        let km = default_vim_normal_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 50, 100);
        assert_eq!(km.resolve(&key("G"), &ctx), Some(Action::ScrollToBottom));
    }

    #[test]
    fn vim_normal_numeric_returns_selection() {
        let km = default_vim_normal_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(
            km.resolve(&key("3"), &ctx),
            Some(Action::ReturnSelectionNth(3))
        );
    }

    #[test]
    fn vim_normal_ctrl_u_half_page_up() {
        let km = default_vim_normal_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 50, 100);
        assert_eq!(
            km.resolve(&key("ctrl-u"), &ctx),
            Some(Action::ScrollHalfPageUp)
        );
    }

    #[test]
    fn vim_normal_screen_jumps() {
        let km = default_vim_normal_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 50, 100);
        assert_eq!(km.resolve(&key("H"), &ctx), Some(Action::ScrollToScreenTop));
        assert_eq!(
            km.resolve(&key("M"), &ctx),
            Some(Action::ScrollToScreenMiddle)
        );
        assert_eq!(
            km.resolve(&key("L"), &ctx),
            Some(Action::ScrollToScreenBottom)
        );
    }

    #[test]
    fn vim_normal_enter_returns_selection() {
        // enter_accept=false in test defaults → ReturnSelection
        let km = default_vim_normal_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(
            km.resolve(&key("enter"), &ctx),
            Some(Action::ReturnSelection)
        );
    }

    #[test]
    fn vim_normal_enter_accept_true_uses_accept() {
        let mut settings = default_settings();
        settings.enter_accept = true;
        let km = default_vim_normal_keymap(&settings);
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("enter"), &ctx), Some(Action::Accept));
    }

    // -- Vim Insert keymap tests --

    #[test]
    fn vim_insert_inherits_emacs_enter() {
        let km = default_vim_insert_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        // enter_accept=false → ReturnSelection
        assert_eq!(
            km.resolve(&key("enter"), &ctx),
            Some(Action::ReturnSelection)
        );
    }

    #[test]
    fn vim_insert_esc_enters_normal() {
        let km = default_vim_insert_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("esc"), &ctx), Some(Action::VimEnterNormal));
    }

    #[test]
    fn vim_insert_ctrl_bracket_enters_normal() {
        let km = default_vim_insert_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(
            km.resolve(&key("ctrl-["), &ctx),
            Some(Action::VimEnterNormal)
        );
    }

    #[test]
    fn vim_insert_inherits_emacs_ctrl_d() {
        let km = default_vim_insert_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        // input empty → return original
        assert_eq!(
            km.resolve(&key("ctrl-d"), &ctx),
            Some(Action::ReturnOriginal)
        );
    }

    // -- Inspector keymap tests --

    #[test]
    fn inspector_ctrl_d_deletes() {
        let km = default_inspector_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("ctrl-d"), &ctx), Some(Action::Delete));
    }

    #[test]
    fn inspector_up_inspects_previous() {
        let km = default_inspector_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("up"), &ctx), Some(Action::InspectPrevious));
    }

    #[test]
    fn inspector_down_inspects_next() {
        let km = default_inspector_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("down"), &ctx), Some(Action::InspectNext));
    }

    #[test]
    fn inspector_esc_exits() {
        let km = default_inspector_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("esc"), &ctx), Some(Action::Exit));
    }

    #[test]
    fn inspector_tab_returns_selection() {
        // enter_accept=false → ReturnSelection
        let km = default_inspector_keymap(&default_settings());
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("tab"), &ctx), Some(Action::ReturnSelection));
    }

    // -- Prefix keymap tests --

    #[test]
    fn prefix_d_deletes() {
        let km = default_prefix_keymap();
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("d"), &ctx), Some(Action::Delete));
    }

    #[test]
    fn prefix_a_cursor_start() {
        let km = default_prefix_keymap();
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("a"), &ctx), Some(Action::CursorStart));
    }

    #[test]
    fn prefix_unknown_key_returns_none() {
        let km = default_prefix_keymap();
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(km.resolve(&key("x"), &ctx), None);
    }

    // -- KeymapSet tests --

    #[test]
    fn keymap_set_defaults_builds() {
        let settings = default_settings();
        let set = KeymapSet::defaults(&settings);
        let ctx = make_ctx(0, 0, 0, 10);

        // Sanity check each keymap has bindings
        assert!(set.emacs.resolve(&key("ctrl-c"), &ctx).is_some());
        assert!(set.vim_normal.resolve(&key("ctrl-c"), &ctx).is_some());
        assert!(set.vim_insert.resolve(&key("ctrl-c"), &ctx).is_some());
        assert!(set.inspector.resolve(&key("ctrl-c"), &ctx).is_some());
        assert!(set.prefix.resolve(&key("d"), &ctx).is_some());
    }

    // -- Settings-dependent behavior --

    #[test]
    fn custom_prefix_char() {
        let mut settings = default_settings();
        settings.keys.prefix = "x".to_string();
        let km = default_emacs_keymap(&settings);
        let ctx = make_ctx(0, 0, 0, 10);

        // ctrl-x should be prefix mode
        assert_eq!(
            km.resolve(&key("ctrl-x"), &ctx),
            Some(Action::EnterPrefixMode)
        );
        // ctrl-a should now be CursorStart (not prefix)
        assert_eq!(km.resolve(&key("ctrl-a"), &ctx), Some(Action::CursorStart));
    }

    #[test]
    fn ctrl_n_shortcuts_changes_numeric_modifier() {
        let mut settings = default_settings();
        settings.ctrl_n_shortcuts = true;
        let km = default_emacs_keymap(&settings);
        let ctx = make_ctx(0, 0, 0, 10);

        // ctrl-1 should work
        assert_eq!(
            km.resolve(&key("ctrl-1"), &ctx),
            Some(Action::ReturnSelectionNth(1))
        );
        // alt-1 should NOT be bound
        assert_eq!(km.resolve(&key("alt-1"), &ctx), None);
    }

    #[test]
    fn default_alt_numeric_shortcuts() {
        let settings = default_settings();
        let km = default_emacs_keymap(&settings);
        let ctx = make_ctx(0, 0, 0, 10);

        // alt-1 should work by default
        assert_eq!(
            km.resolve(&key("alt-1"), &ctx),
            Some(Action::ReturnSelectionNth(1))
        );
    }

    // -----------------------------------------------------------------------
    // Config parsing and merging tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_simple_binding_config() {
        use atuin_client::settings::KeyBindingConfig;
        let cfg = KeyBindingConfig::Simple("accept".to_string());
        let binding = super::parse_binding_config(&cfg).unwrap();
        assert_eq!(binding.rules.len(), 1);
        assert!(binding.rules[0].condition.is_none());
        assert_eq!(binding.rules[0].action, Action::Accept);
    }

    #[test]
    fn parse_conditional_binding_config() {
        use atuin_client::settings::{KeyBindingConfig, KeyRuleConfig};
        let cfg = KeyBindingConfig::Rules(vec![
            KeyRuleConfig {
                when: Some("cursor-at-start".to_string()),
                action: "exit".to_string(),
            },
            KeyRuleConfig {
                when: None,
                action: "cursor-left".to_string(),
            },
        ]);
        let binding = super::parse_binding_config(&cfg).unwrap();
        assert_eq!(binding.rules.len(), 2);
        assert!(binding.rules[0].condition.is_some());
        assert_eq!(binding.rules[0].action, Action::Exit);
        assert!(binding.rules[1].condition.is_none());
        assert_eq!(binding.rules[1].action, Action::CursorLeft);
    }

    #[test]
    fn parse_binding_config_invalid_action() {
        use atuin_client::settings::KeyBindingConfig;
        let cfg = KeyBindingConfig::Simple("not-a-real-action".to_string());
        assert!(super::parse_binding_config(&cfg).is_err());
    }

    #[test]
    fn parse_binding_config_invalid_condition() {
        use atuin_client::settings::{KeyBindingConfig, KeyRuleConfig};
        let cfg = KeyBindingConfig::Rules(vec![KeyRuleConfig {
            when: Some("not-a-real-condition".to_string()),
            action: "exit".to_string(),
        }]);
        assert!(super::parse_binding_config(&cfg).is_err());
    }

    #[test]
    fn config_override_replaces_key() {
        use atuin_client::settings::KeyBindingConfig;
        use std::collections::HashMap;

        let mut settings = default_settings();
        let set = KeymapSet::defaults(&settings);

        // Default: ctrl-c → ReturnOriginal
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(
            set.emacs.resolve(&key("ctrl-c"), &ctx),
            Some(Action::ReturnOriginal)
        );

        // Override ctrl-c → Exit via config
        settings.keymap.emacs = HashMap::from([(
            "ctrl-c".to_string(),
            KeyBindingConfig::Simple("exit".to_string()),
        )]);

        let set = KeymapSet::from_settings(&settings);
        assert_eq!(set.emacs.resolve(&key("ctrl-c"), &ctx), Some(Action::Exit));
    }

    #[test]
    fn config_override_preserves_unoverridden_keys() {
        use atuin_client::settings::KeyBindingConfig;
        use std::collections::HashMap;

        let mut settings = default_settings();
        // Override only ctrl-c; enter should keep its default
        settings.keymap.emacs = HashMap::from([(
            "ctrl-c".to_string(),
            KeyBindingConfig::Simple("exit".to_string()),
        )]);

        let set = KeymapSet::from_settings(&settings);
        let ctx = make_ctx(0, 0, 0, 10);

        // ctrl-c overridden
        assert_eq!(set.emacs.resolve(&key("ctrl-c"), &ctx), Some(Action::Exit));
        // enter still has default (enter_accept=false → ReturnSelection)
        assert_eq!(
            set.emacs.resolve(&key("enter"), &ctx),
            Some(Action::ReturnSelection)
        );
    }

    #[test]
    fn config_conditional_override() {
        use atuin_client::settings::{KeyBindingConfig, KeyRuleConfig};
        use std::collections::HashMap;

        let mut settings = default_settings();
        // Override "up" with a custom conditional
        settings.keymap.emacs = HashMap::from([(
            "up".to_string(),
            KeyBindingConfig::Rules(vec![
                KeyRuleConfig {
                    when: Some("no-results".to_string()),
                    action: "exit".to_string(),
                },
                KeyRuleConfig {
                    when: None,
                    action: "select-previous".to_string(),
                },
            ]),
        )]);

        let set = KeymapSet::from_settings(&settings);

        // With no results → exit
        let ctx = make_ctx(0, 0, 0, 0);
        assert_eq!(set.emacs.resolve(&key("up"), &ctx), Some(Action::Exit));

        // With results → select-previous
        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(
            set.emacs.resolve(&key("up"), &ctx),
            Some(Action::SelectPrevious)
        );
    }

    #[test]
    fn from_settings_with_empty_config_equals_defaults() {
        let settings = default_settings();
        let defaults = KeymapSet::defaults(&settings);
        let from_settings = KeymapSet::from_settings(&settings);

        // Verify a sample of keys produce the same results
        let ctx = make_ctx(0, 0, 0, 10);
        let test_keys = [
            "ctrl-c", "enter", "esc", "tab", "up", "down", "left", "right",
        ];
        for k in &test_keys {
            assert_eq!(
                defaults.emacs.resolve(&key(k), &ctx),
                from_settings.emacs.resolve(&key(k), &ctx),
                "mismatch for emacs key {k}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Phase 5: [keys] vs [keymap] backward compatibility
    // -----------------------------------------------------------------------

    #[test]
    fn keymap_overrides_ignore_keys_section() {
        use atuin_client::settings::KeyBindingConfig;

        // Set up: [keys] disables scroll_exits, but [keymap] is present
        let mut settings = default_settings();
        settings.keys.scroll_exits = false;

        // Without [keymap], scroll_exits=false means no exit condition on down
        let set_legacy = KeymapSet::defaults(&settings);
        // At list-at-start (selected=0), down should still be SelectNext (no exit)
        let ctx_at_boundary = make_ctx(0, 0, 0, 10);
        assert_eq!(
            set_legacy.emacs.resolve(&key("down"), &ctx_at_boundary),
            Some(Action::SelectNext),
            "legacy: down at boundary should be SelectNext with scroll_exits=false"
        );

        // With [keymap] present (even just one override), [keys] is ignored
        // so the standard defaults (scroll_exits=true) apply
        settings.keymap.emacs = HashMap::from([(
            "ctrl-c".to_string(),
            KeyBindingConfig::Simple("exit".to_string()),
        )]);
        let set_keymap = KeymapSet::from_settings(&settings);

        // Not at boundary (selected=5): should SelectNext normally
        let ctx_not_at_boundary = make_ctx(0, 0, 5, 10);
        assert_eq!(
            set_keymap.emacs.resolve(&key("down"), &ctx_not_at_boundary),
            Some(Action::SelectNext),
            "keymap: down not at boundary should SelectNext"
        );
        // At list-at-start (selected=0): should Exit (standard scroll_exits=true)
        assert_eq!(
            set_keymap.emacs.resolve(&key("down"), &ctx_at_boundary),
            Some(Action::Exit),
            "keymap: down at boundary should Exit (standard defaults restored)"
        );
    }

    #[test]
    fn keymap_present_resets_to_standard_keys_defaults() {
        use atuin_client::settings::KeyBindingConfig;

        let mut settings = default_settings();
        // Disable all [keys] behaviors
        settings.keys.exit_past_line_start = false;
        settings.keys.accept_past_line_end = false;

        // Without [keymap], left should be plain CursorLeft
        let set_legacy = KeymapSet::defaults(&settings);
        let ctx_at_start = make_ctx(0, 5, 0, 10);
        assert_eq!(
            set_legacy.emacs.resolve(&key("left"), &ctx_at_start),
            Some(Action::CursorLeft),
            "legacy: left should be plain CursorLeft without exit_past_line_start"
        );

        // Add a [keymap] entry (for a different key)
        settings.keymap.emacs = HashMap::from([(
            "ctrl-c".to_string(),
            KeyBindingConfig::Simple("exit".to_string()),
        )]);
        let set_keymap = KeymapSet::from_settings(&settings);

        // Now left should use standard defaults (exit_past_line_start=true)
        // At cursor start → Exit
        assert_eq!(
            set_keymap.emacs.resolve(&key("left"), &ctx_at_start),
            Some(Action::Exit),
            "keymap: left at cursor start should exit (standard defaults)"
        );

        // Right at cursor end should return selection (standard defaults: accept_past_line_end=true, enter_accept=false)
        let ctx_at_end = make_ctx(5, 5, 0, 10);
        assert_eq!(
            set_keymap.emacs.resolve(&key("right"), &ctx_at_end),
            Some(Action::ReturnSelection),
            "keymap: right at cursor end should return selection (standard defaults)"
        );
    }

    #[test]
    fn keys_has_non_default_values_detection() {
        use atuin_client::settings::Keys;

        let standard = Keys::standard_defaults();
        assert!(!standard.has_non_default_values());

        let mut modified = Keys::standard_defaults();
        modified.scroll_exits = false;
        assert!(modified.has_non_default_values());

        let mut modified = Keys::standard_defaults();
        modified.prefix = "x".to_string();
        assert!(modified.has_non_default_values());
    }

    #[test]
    fn original_input_empty_condition_in_config() {
        use atuin_client::settings::{KeyBindingConfig, KeyRuleConfig};
        use std::collections::HashMap;

        let mut settings = default_settings();
        // Configure esc to: if original-input-empty -> return-query, else return-original
        settings.keymap.emacs = HashMap::from([(
            "esc".to_string(),
            KeyBindingConfig::Rules(vec![
                KeyRuleConfig {
                    when: Some("original-input-empty".to_string()),
                    action: "return-query".to_string(),
                },
                KeyRuleConfig {
                    when: None,
                    action: "return-original".to_string(),
                },
            ]),
        )]);

        let set = KeymapSet::from_settings(&settings);

        // When original input was empty, should return-query
        let ctx_original_empty = EvalContext {
            cursor_position: 0,
            input_width: 5,
            input_byte_len: 5,
            selected_index: 0,
            results_len: 10,
            original_input_empty: true,
            has_context: false,
        };
        assert_eq!(
            set.emacs.resolve(&key("esc"), &ctx_original_empty),
            Some(Action::ReturnQuery),
            "esc with original_input_empty=true should return-query"
        );

        // When original input was not empty, should return-original
        let ctx_original_not_empty = EvalContext {
            cursor_position: 0,
            input_width: 5,
            input_byte_len: 5,
            selected_index: 0,
            results_len: 10,
            original_input_empty: false,
            has_context: false,
        };
        assert_eq!(
            set.emacs.resolve(&key("esc"), &ctx_original_not_empty),
            Some(Action::ReturnOriginal),
            "esc with original_input_empty=false should return-original"
        );
    }
}
