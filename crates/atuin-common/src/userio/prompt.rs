use std::io::{self, Write as _};

use zeroize::Zeroizing;

/// A question the user answers with a line of text.
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
}
