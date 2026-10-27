//! Talking to the user: a command's report, and the questions the user answers.
//!
//! [`out!`] and [`outln!`] write a report the user reads to stdout. What a program reads, such as
//! an init script, is written to stdout directly instead. [`widgetln!`] sits beside them, though
//! the shell widget reads its line rather than the user.
//!
//! [`Prompt`] and [`Confirm`] ask on stderr, so the question reaches the user even when stdout is
//! redirected (`atuin scripts run deploy > out`), and the answer is the next line on stdin.

mod confirm;
mod out;
mod prompt;

pub use confirm::{Confirm, DefaultAnswer};
pub use out::{Output, out, outln, widgetln};
pub use prompt::Prompt;
