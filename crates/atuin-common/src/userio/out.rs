//! A report on stdout for the user to read.
//!
//! [`out!`] and [`outln!`] take the place of `print!` and `println!`, which panic once the reader
//! has gone (`atuin stats | head`).

use std::fmt;
use std::io::{self, Write};

/// Locked stdout, which [`out!`] and [`outln!`] write to.
pub struct Output<W>(W);

impl Output<io::StdoutLock<'static>> {
    #[must_use]
    pub fn stdout() -> Self {
        Self(io::stdout().lock())
    }
}

impl<W: Write> Output<W> {
    /// A reader that has gone isn't an error: the rest of the output goes nowhere, as it would
    /// have. Any other failure ends the process.
    pub fn write_fmt(&mut self, args: fmt::Arguments<'_>) {
        let Err(err) = self.0.write_fmt(args) else {
            return;
        };
        if err.kind() == io::ErrorKind::BrokenPipe {
            return;
        }
        crate::userio::log::error!("failed to write the output: {err}");
        std::process::exit(1);
    }
}

#[macro_export]
macro_rules! __atuin_out {
    ($($arg:tt)*) => {
        ::std::write!($crate::userio::Output::stdout(), $($arg)*)
    };
}

#[macro_export]
macro_rules! __atuin_outln {
    ($($arg:tt)*) => {
        ::std::writeln!($crate::userio::Output::stdout(), $($arg)*)
    };
}

pub use __atuin_out as out;
pub use __atuin_outln as outln;
