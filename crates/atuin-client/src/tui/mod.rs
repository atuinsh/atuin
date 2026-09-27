//! Terminal UI plumbing shared by atuin's interactive views (history search, `atuin ai resume`).
//!
//! Only depends on crossterm and serde, so any crate that renders a TUI can use it:
//! - [`key`]: parse and display key presses (`ctrl-r`, `alt-h`, `g g`) and convert crossterm events;
//! - [`conditions`]: boolean conditions over input/list state (`cursor-at-start && !no-results`);
//! - [`keymap`]: keymaps generic over the view's own action type;
//! - [`cursor`]: the single-line text input with word motions.

pub mod conditions;
pub mod cursor;
pub mod key;
pub mod keymap;

pub use conditions::{ConditionAtom, ConditionExpr, EvalContext};
pub use cursor::Cursor;
pub use key::{KeyCodeValue, KeyInput, SingleKey};
pub use keymap::{KeyBinding, KeyRule, Keymap};
