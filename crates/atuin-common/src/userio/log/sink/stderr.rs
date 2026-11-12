//! A sink that writes every message to stderr.

use std::io::{self, IsTerminal, Write};

use colored::{Color, Colorize};

use super::{Message, Severity, Sink};

/// Writes every message to stderr, leaving stdout to the command's output.
///
/// A warning starts with `warning: ` and an error with `error: `.
pub struct Stderr {
    /// Whether stderr is a terminal. [`colored`] adds its own checks on top: `NO_COLOR`, and
    /// that *stdout* is a terminal.
    styled: bool,
}

impl Default for Stderr {
    fn default() -> Self {
        Self {
            styled: io::stderr().is_terminal(),
        }
    }
}

impl Sink for Stderr {
    fn show(&self, message: Message) {
        let mut stderr = io::stderr();

        let label = match message.severity {
            Severity::Info => None,
            Severity::Warn => Some(("warning:", Color::Yellow)),
            Severity::Error => Some(("error:", Color::Red)),
        };

        let written = match label {
            None => writeln!(stderr, "{}", message.text),
            Some((label, color)) if self.styled => {
                writeln!(stderr, "{} {}", label.color(color).bold(), message.text)
            }
            Some((label, _)) => writeln!(stderr, "{label} {}", message.text),
        };

        let Err(err) = written else {
            return;
        };

        if err.kind() == io::ErrorKind::BrokenPipe {
            return;
        }

        tracing::error!(err = ?err, "failed to write a message for the user to stderr.");
    }
}
