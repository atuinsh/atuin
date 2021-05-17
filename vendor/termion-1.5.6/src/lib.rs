//! Termion is a pure Rust, bindless library for low-level handling, manipulating
//! and reading information about terminals. This provides a full-featured
//! alternative to Termbox.
//!
//! Termion aims to be simple and yet expressive. It is bindless, meaning that it
//! is not a front-end to some other library (e.g., ncurses or termbox), but a
//! standalone library directly talking to the TTY.
//!
//! Supports Redox, Mac OS X, and Linux (or, in general, ANSI terminals).
//!
//! For more information refer to the [README](https://github.com/redox-os/termion).
#![warn(missing_docs)]

extern crate numtoa;

#[cfg(target_os = "redox")]
#[path="sys/redox/mod.rs"]
mod sys;

#[cfg(all(unix, not(target_os = "redox")))]
#[path="sys/unix/mod.rs"]
mod sys;

pub use sys::size::terminal_size;
#[cfg(all(unix, not(target_os = "redox")))]
pub use sys::size::terminal_size_pixels;
pub use sys::tty::{is_tty, get_tty};

mod async;
pub use async::{AsyncReader, async_stdin};

#[macro_use]
mod macros;
pub mod clear;
pub mod color;
pub mod cursor;
pub mod event;
pub mod input;
pub mod raw;
pub mod screen;
pub mod scroll;
pub mod style;

#[cfg(test)]
mod test {
    use super::sys;

    #[test]
    fn test_get_terminal_attr() {
        sys::attr::get_terminal_attr().unwrap();
        sys::attr::get_terminal_attr().unwrap();
        sys::attr::get_terminal_attr().unwrap();
    }

    #[test]
    fn test_set_terminal_attr() {
        let ios = sys::attr::get_terminal_attr().unwrap();
        sys::attr::set_terminal_attr(&ios).unwrap();
    }

    #[test]
    fn test_size() {
        sys::size::terminal_size().unwrap();
    }
}
