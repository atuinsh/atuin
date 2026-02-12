use std::collections::HashMap;

use super::actions::Action;
use super::conditions::{ConditionExpr, EvalContext};
use super::key::{KeyInput, SingleKey};

/// A single rule within a keybinding: an optional condition and an action.
/// If the condition is `None`, the rule always matches.
#[derive(Debug, Clone)]
pub struct KeyRule {
    pub condition: Option<ConditionExpr>,
    pub action: Action,
}

/// A keybinding is an ordered list of rules. The first rule whose condition
/// matches (or has no condition) wins.
#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub rules: Vec<KeyRule>,
}

/// A keymap is a collection of keybindings indexed by key input.
#[derive(Debug, Clone)]
pub struct Keymap {
    pub bindings: HashMap<KeyInput, KeyBinding>,
}

impl KeyRule {
    /// Create an unconditional rule.
    pub fn always(action: Action) -> Self {
        KeyRule {
            condition: None,
            action,
        }
    }

    /// Create a conditional rule. Accepts any type convertible to `ConditionExpr`,
    /// including bare `ConditionAtom` values.
    pub fn when(condition: impl Into<ConditionExpr>, action: Action) -> Self {
        KeyRule {
            condition: Some(condition.into()),
            action,
        }
    }
}

impl KeyBinding {
    /// Create a simple (unconditional) binding.
    pub fn simple(action: Action) -> Self {
        KeyBinding {
            rules: vec![KeyRule::always(action)],
        }
    }

    /// Create a conditional binding from a list of rules.
    pub fn conditional(rules: Vec<KeyRule>) -> Self {
        KeyBinding { rules }
    }
}

impl Keymap {
    /// Create an empty keymap.
    pub fn new() -> Self {
        Keymap {
            bindings: HashMap::new(),
        }
    }

    /// Bind a key input to a simple (unconditional) action.
    pub fn bind(&mut self, key: KeyInput, action: Action) {
        self.bindings.insert(key, KeyBinding::simple(action));
    }

    /// Bind a key input to a conditional set of rules.
    pub fn bind_conditional(&mut self, key: KeyInput, rules: Vec<KeyRule>) {
        self.bindings.insert(key, KeyBinding::conditional(rules));
    }

    /// Resolve a key input to an action given the current evaluation context.
    /// Returns `None` if the key has no binding or no rule's condition matches.
    pub fn resolve(&self, key: &KeyInput, ctx: &EvalContext) -> Option<Action> {
        let binding = self.bindings.get(key)?;
        for rule in &binding.rules {
            match &rule.condition {
                None => return Some(rule.action.clone()),
                Some(cond) if cond.evaluate(ctx) => return Some(rule.action.clone()),
                Some(_) => {}
            }
        }
        None
    }

    /// Check if any binding starts with the given single key as the first key
    /// of a multi-key sequence. Used to detect pending multi-key sequences.
    pub fn has_sequence_starting_with(&self, prefix: &SingleKey) -> bool {
        self.bindings.keys().any(|ki| match ki {
            KeyInput::Sequence(keys) => keys.first() == Some(prefix),
            KeyInput::Single(_) => false,
        })
    }

    /// Merge another keymap into this one. Keys from `other` override keys in `self`.
    #[allow(dead_code)]
    pub fn merge(&mut self, other: &Keymap) {
        for (key, binding) in &other.bindings {
            self.bindings.insert(key.clone(), binding.clone());
        }
    }
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::conditions::ConditionAtom;
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
        keymap.bind_conditional(
            key.clone(),
            vec![
                KeyRule::when(ConditionAtom::CursorAtStart, Action::Exit),
                KeyRule::always(Action::CursorLeft),
            ],
        );

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
        keymap.bind_conditional(
            key.clone(),
            vec![KeyRule::when(ConditionAtom::CursorAtStart, Action::Exit)],
        );

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
