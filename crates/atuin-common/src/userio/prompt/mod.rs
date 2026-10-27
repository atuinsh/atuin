mod confirm;
mod secret;

use std::io::{self, Write as _};

pub use confirm::{Confirm, DefaultAnswer};
pub use secret::Secret;
use zeroize::Zeroizing;

/// A question the user answers with a line of text, shown with a space after it.
pub struct Prompt<'a>(&'a str);

impl<'a> Prompt<'a> {
    #[must_use]
    pub const fn new(question: &'a str) -> Self {
        Self(question)
    }

    /// The answer without its line ending, or `None` once stdin has ended.
    pub fn ask<T: for<'b> From<&'b str>>(self) -> io::Result<Option<T>> {
        let mut stderr = io::stderr().lock();
        write!(stderr, "{} ", self.0)?;
        stderr.flush()?;
        Self::read_answer()
    }

    /// A line of stdin without its line ending, or `None` once stdin has ended.
    ///
    /// The line is read into a buffer that is wiped afterwards, so a secret only lives on in `T`.
    fn read_answer<T: for<'b> From<&'b str>>() -> io::Result<Option<T>> {
        let mut answer = Zeroizing::new(String::new());
        if io::stdin().read_line(&mut answer)? == 0 {
            return Ok(None);
        }
        Ok(Some(answer.trim_end_matches(['\r', '\n']).into()))
    }
}
