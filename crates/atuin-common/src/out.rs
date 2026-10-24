//! A command's output on stdout: what the user or a program reads as its result.
//!
//! [`out!`] and [`outln!`] take the place of `print!` and `println!`, which panic once the reader
//! has gone (`atuin stats | head`).

use std::fmt;
use std::io::{self, Write as _};

/// Locked stdout that [`out!`] and [`outln!`] write to.
pub struct Stdout(io::StdoutLock<'static>);

impl Stdout {
    #[must_use]
    pub fn lock() -> Self {
        Self(io::stdout().lock())
    }

    /// A reader that has gone isn't an error: the rest of the output goes nowhere, as it would
    /// have. Any other failure ends the process.
    pub fn write_fmt(&mut self, args: fmt::Arguments<'_>) {
        let Err(err) = self.0.write_fmt(args) else {
            return;
        };
        if err.kind() == io::ErrorKind::BrokenPipe {
            return;
        }
        tracing::error!(target: crate::logs::USER, "failed to write the output: {err}");
        std::process::exit(1);
    }
}

#[macro_export]
macro_rules! __atuin_out {
    ($($arg:tt)*) => {
        ::std::write!($crate::out::Stdout::lock(), $($arg)*)
    };
}

#[macro_export]
macro_rules! __atuin_outln {
    ($($arg:tt)*) => {
        ::std::writeln!($crate::out::Stdout::lock(), $($arg)*)
    };
}

pub use __atuin_out as out;
pub use __atuin_outln as outln;
