//! History search keymaps: the shared generic keymap bound to the search [`Action`].

use super::actions::Action;

pub type KeyRule = atuin_client::tui::keymap::KeyRule<Action>;
pub type KeyBinding = atuin_client::tui::keymap::KeyBinding<Action>;
pub type Keymap = atuin_client::tui::keymap::Keymap<Action>;

#[cfg(test)]
mod tests {
    use super::super::conditions::{ConditionAtom, EvalContext};
    use super::super::key::{KeyInput, SingleKey};
    use super::*;

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

    #[test]
    fn simple_binding_resolves() {
        let mut keymap = Keymap::new();
        let key = KeyInput::parse("ctrl-c").unwrap();
        keymap.bind(key.clone(), Action::ReturnOriginal);

        let ctx = make_ctx(0, 0, 0, 10);
        assert_eq!(keymap.resolve(&key, &ctx), Some(Action::ReturnOriginal));
    }

    #[test]
    fn conditional_first_match_wins() {
        let mut keymap = Keymap::new();
        let key = KeyInput::parse("left").unwrap();
        keymap.bind_conditional(key.clone(), vec![
            KeyRule::when(ConditionAtom::CursorAtStart, Action::Exit),
            KeyRule::always(Action::CursorLeft),
        ]);

        // Cursor at start → Exit
        let ctx = make_ctx(0, 5, 0, 10);
        assert_eq!(keymap.resolve(&key, &ctx), Some(Action::Exit));

        // Cursor not at start → CursorLeft
        let ctx = make_ctx(3, 5, 0, 10);
        assert_eq!(keymap.resolve(&key, &ctx), Some(Action::CursorLeft));
    }

    #[test]
    fn no_match_returns_none() {
        let keymap = Keymap::new();
        let key = KeyInput::parse("ctrl-c").unwrap();
        let ctx = make_ctx(0, 0, 0, 0);
        assert_eq!(keymap.resolve(&key, &ctx), None);
    }

    #[test]
    fn conditional_no_condition_matches_returns_none() {
        let mut keymap = Keymap::new();
        let key = KeyInput::parse("left").unwrap();
        // Only one rule with a condition that won't match
        keymap.bind_conditional(key.clone(), vec![KeyRule::when(
            ConditionAtom::CursorAtStart,
            Action::Exit,
        )]);

        // Cursor not at start → no match
        let ctx = make_ctx(3, 5, 0, 10);
        assert_eq!(keymap.resolve(&key, &ctx), None);
    }

    #[test]
    fn has_sequence_starting_with() {
        let mut keymap = Keymap::new();
        let seq = KeyInput::parse("g g").unwrap();
        keymap.bind(seq, Action::ScrollToTop);

        let g = SingleKey::parse("g").unwrap();
        assert!(keymap.has_sequence_starting_with(&g));

        let h = SingleKey::parse("h").unwrap();
        assert!(!keymap.has_sequence_starting_with(&h));
    }

    #[test]
    fn merge_overrides() {
        let mut base = Keymap::new();
        let key = KeyInput::parse("ctrl-c").unwrap();
        base.bind(key.clone(), Action::ReturnOriginal);

        let mut overlay = Keymap::new();
        overlay.bind(key.clone(), Action::Exit);

        base.merge(&overlay);

        let ctx = make_ctx(0, 0, 0, 0);
        assert_eq!(base.resolve(&key, &ctx), Some(Action::Exit));
    }

    #[test]
    fn merge_preserves_unoverridden() {
        let mut base = Keymap::new();
        let key1 = KeyInput::parse("ctrl-c").unwrap();
        let key2 = KeyInput::parse("ctrl-d").unwrap();
        base.bind(key1.clone(), Action::ReturnOriginal);
        base.bind(key2.clone(), Action::DeleteCharAfter);

        let mut overlay = Keymap::new();
        overlay.bind(key1.clone(), Action::Exit);

        base.merge(&overlay);

        let ctx = make_ctx(0, 0, 0, 0);
        assert_eq!(base.resolve(&key1, &ctx), Some(Action::Exit));
        assert_eq!(base.resolve(&key2, &ctx), Some(Action::DeleteCharAfter));
    }
}
