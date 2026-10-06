//! Terminal UI pieces shared by the interactive views (the history search and `atuin ai resume`).
//!
//! - [`key`]: parse key names (`ctrl-r`, `alt-h`, `g g`) and convert crossterm events;
//! - [`cursor`]: the single-line text input with word motions.

pub mod cursor;
pub mod key;
