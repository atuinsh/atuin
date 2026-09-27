use std::collections::HashMap;

use super::conditions::{ConditionExpr, EvalContext};
use super::key::{KeyInput, SingleKey};

/// A single rule within a keybinding: an optional condition and an action.
/// If the condition is `None`, the rule always matches.
///
/// Generic over the action type so each TUI (history search, `atuin ai resume`, ...) can bind
/// keys to its own action enum while sharing key parsing and condition evaluation.
#[derive(Debug, Clone)]
pub struct KeyRule<A> {
    pub condition: Option<ConditionExpr>,
    pub action: A,
}

/// A keybinding is an ordered list of rules. The first rule whose condition
/// matches (or has no condition) wins.
#[derive(Debug, Clone)]
pub struct KeyBinding<A> {
    pub rules: Vec<KeyRule<A>>,
}

/// A keymap is a collection of keybindings indexed by key input.
#[derive(Debug, Clone)]
pub struct Keymap<A> {
    pub bindings: HashMap<KeyInput, KeyBinding<A>>,
}

impl<A> KeyRule<A> {
    /// Create an unconditional rule.
    pub fn always(action: A) -> Self {
        Self {
            condition: None,
            action,
        }
    }

    /// Create a conditional rule. Accepts any type convertible to `ConditionExpr`,
    /// including bare `ConditionAtom` values.
    pub fn when(condition: impl Into<ConditionExpr>, action: A) -> Self {
        Self {
            condition: Some(condition.into()),
            action,
        }
    }
}

impl<A> KeyBinding<A> {
    /// Create a simple (unconditional) binding.
    pub fn simple(action: A) -> Self {
        Self {
            rules: vec![KeyRule::always(action)],
        }
    }

    /// Create a conditional binding from a list of rules.
    #[must_use]
    pub fn conditional(rules: Vec<KeyRule<A>>) -> Self {
        Self { rules }
    }
}

impl<A: Clone> Keymap<A> {
    /// Create an empty keymap.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Bind a key input to a simple (unconditional) action.
    pub fn bind(&mut self, key: KeyInput, action: A) {
        self.bindings.insert(key, KeyBinding::simple(action));
    }

    /// Bind a key input to a conditional set of rules.
    pub fn bind_conditional(&mut self, key: KeyInput, rules: Vec<KeyRule<A>>) {
        self.bindings.insert(key, KeyBinding::conditional(rules));
    }

    /// Resolve a key input to an action given the current evaluation context.
    /// Returns `None` if the key has no binding or no rule's condition matches.
    #[must_use]
    pub fn resolve(&self, key: &KeyInput, ctx: &EvalContext) -> Option<A> {
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
    #[must_use]
    pub fn has_sequence_starting_with(&self, prefix: &SingleKey) -> bool {
        self.bindings.keys().any(|ki| match ki {
            KeyInput::Sequence(keys) => keys.first() == Some(prefix),
            KeyInput::Single(_) => false,
        })
    }

    /// Merge another keymap into this one. Keys from `other` override keys in `self`.
    pub fn merge(&mut self, other: &Self) {
        for (key, binding) in &other.bindings {
            self.bindings.insert(key.clone(), binding.clone());
        }
    }
}

impl<A: Clone> Default for Keymap<A> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::super::conditions::ConditionAtom;
    use super::*;

    /// A stand-in action type: the keymap only needs `Clone`, so any enum works.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestAction {
        Quit,
        Left,
    }

    fn ctx(cursor: usize, width: usize) -> EvalContext {
        EvalContext {
            cursor_position: cursor,
            input_width: width,
            input_byte_len: width,
            selected_index: 0,
            results_len: 0,
            original_input_empty: false,
            has_context: false,
        }
    }

    #[rstest]
    fn generic_action_type_resolves() {
        let mut keymap = Keymap::new();
        let key = KeyInput::parse("left").unwrap();
        keymap.bind_conditional(key.clone(), vec![
            KeyRule::when(ConditionAtom::CursorAtStart, TestAction::Quit),
            KeyRule::always(TestAction::Left),
        ]);

        assert_eq!(keymap.resolve(&key, &ctx(0, 3)), Some(TestAction::Quit));
        assert_eq!(keymap.resolve(&key, &ctx(2, 3)), Some(TestAction::Left));
        assert_eq!(keymap.resolve(&KeyInput::parse("right").unwrap(), &ctx(0, 0)), None);
    }

    #[rstest]
    fn generic_merge_overrides() {
        let key = KeyInput::parse("esc").unwrap();
        let mut base = Keymap::new();
        base.bind(key.clone(), TestAction::Left);
        let mut overlay = Keymap::new();
        overlay.bind(key.clone(), TestAction::Quit);
        base.merge(&overlay);
        assert_eq!(base.resolve(&key, &ctx(0, 0)), Some(TestAction::Quit));
    }
}
