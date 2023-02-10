//! Fork of `tui-rs`
#![allow(
    clippy::module_name_repetitions,
    clippy::bool_to_int_with_if,
    clippy::similar_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    dead_code
)]

pub mod backend;
pub mod buffer;
pub mod layout;
pub mod style;
pub mod symbols;
pub mod terminal;
pub mod text;
pub mod widgets;

pub use self::terminal::{Frame, Terminal, TerminalOptions, Viewport};
