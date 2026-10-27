use std::io;

use super::Prompt;

/// What an empty answer to a [`Confirm`] means.
#[derive(Clone, Copy, Debug)]
pub enum DefaultAnswer {
    Yes,
    No,
}

/// A yes or no question, followed by `[Y/n]` or `[y/N]`.
pub struct Confirm<'a> {
    question: &'a str,
    default: DefaultAnswer,
}

impl<'a> Confirm<'a> {
    #[must_use]
    pub const fn new(question: &'a str, default: DefaultAnswer) -> Self {
        Self { question, default }
    }

    /// Prompt the user and check whether they answered yes or no.
    pub fn ask(self) -> io::Result<bool> {
        let hint = match self.default {
            DefaultAnswer::Yes => "[Y/n]",
            DefaultAnswer::No => "[y/N]",
        };
        let answer: Option<String> = Prompt::new(&format!("{} {hint}", self.question)).ask()?;
        Ok(match answer.unwrap_or_default().trim().to_lowercase().as_str() {
            "" => matches!(self.default, DefaultAnswer::Yes),
            "y" | "yes" => true,
            _ => false,
        })
    }
}
