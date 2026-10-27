use std::io::{self, IsTerminal as _};

use secrecy::SecretString;
use zeroize::Zeroizing;

use super::Prompt;

/// A question the user answers with a secret, which a terminal does not echo.
pub struct Secret<'a>(&'a str);

impl<'a> Secret<'a> {
    #[must_use]
    pub const fn new(question: &'a str) -> Self {
        Self(question)
    }

    /// The answer, or `None` once stdin has ended.
    ///
    /// Stdin that is not a terminal is answered by a script, so its first line is read without
    /// showing the question.
    pub fn ask(self) -> io::Result<Option<SecretString>> {
        if !io::stdin().is_terminal() {
            return Prompt::read_answer();
        }
        let answer = Zeroizing::new(rpassword::prompt_password(format!("{} ", self.0))?);
        // `From<&str>` copies; `From<String>` would shrink the buffer and free the old one unwiped.
        Ok(Some(answer.as_str().into()))
    }
}
