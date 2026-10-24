//! Questions the user answers on the terminal.
//!
//! The question goes to stderr, so it reaches the user even when stdout is redirected (`atuin
//! scripts run deploy > out`), and the answer is the next line on stdin.

use std::io::{self, Write as _};

use zeroize::Zeroizing;

/// What an empty answer to [`Prompt::confirm`] means.
#[derive(Clone, Copy, Debug)]
pub enum DefaultAnswer {
    Yes,
    No,
}

/// A question for the user.
pub struct Prompt<'a>(&'a str);

impl<'a> Prompt<'a> {
    /// The question is written as is, so it ends with what the answer follows, like `": "`.
    #[must_use]
    pub const fn new(question: &'a str) -> Self {
        Self(question)
    }

    /// The answer without its line ending, or `None` once stdin has ended.
    ///
    /// The line is read into a buffer that is wiped afterwards, so a secret only lives on in `T`.
    pub fn ask<T: for<'b> From<&'b str>>(self) -> io::Result<Option<T>> {
        let mut stderr = io::stderr().lock();
        stderr.write_all(self.0.as_bytes())?;
        stderr.flush()?;

        let mut answer = Zeroizing::new(String::new());
        if io::stdin().read_line(&mut answer)? == 0 {
            return Ok(None);
        }
        Ok(Some(answer.trim_end_matches(['\r', '\n']).into()))
    }

    /// A yes or no question, followed by `[Y/n]` or `[y/N]`. An empty answer, or none at all,
    /// takes `default`.
    pub fn confirm(self, default: DefaultAnswer) -> io::Result<bool> {
        let hint = match default {
            DefaultAnswer::Yes => "[Y/n]",
            DefaultAnswer::No => "[y/N]",
        };
        let answer: Option<String> = Prompt::new(&format!("{} {hint} ", self.0)).ask()?;
        Ok(match answer.unwrap_or_default().trim().to_lowercase().as_str() {
            "" => matches!(default, DefaultAnswer::Yes),
            "y" | "yes" => true,
            _ => false,
        })
    }
}
