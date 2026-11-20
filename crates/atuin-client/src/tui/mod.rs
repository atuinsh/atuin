//! Terminal UI pieces shared by the interactive views (the history search and `atuin threads resume`).
//!
//! - [`background`]: tell whether the terminal's background is light or dark;
//! - [`key`]: parse key names (`ctrl-r`, `alt-h`, `g g`) and convert crossterm events;
//! - [`cursor`]: the single-line text input with word motions.

pub mod background;
pub mod cursor;
pub mod key;
