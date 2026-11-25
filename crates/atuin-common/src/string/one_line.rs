//! A text written on one line.

use std::fmt;

/// A text on one line: whitespace runs shrink to one space, and a text of more than `max_chars`
/// characters ends in `…`.
#[derive(Debug, Clone, Copy)]
pub struct OneLine<'a> {
    pub text: &'a str,
    pub max_chars: usize,
}

impl OneLine<'_> {
    /// Returns whether the text is longer than `max_chars` once its whitespace is joined, so
    /// that the display ends in `…`.
    #[must_use]
    pub fn is_cut(&self) -> bool {
        self.first_word_that_does_not_fit().is_some()
    }

    /// The index of the first word that would take the line beyond `max_chars`.
    fn first_word_that_does_not_fit(&self) -> Option<usize> {
        let mut chars = 0;
        for (index, word) in self.text.split_whitespace().enumerate() {
            chars += usize::from(index > 0) + word.chars().count();
            if chars > self.max_chars {
                return Some(index);
            }
        }
        None
    }
}

impl fmt::Display for OneLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cut = self.first_word_that_does_not_fit();
        let fitting = cut.unwrap_or(usize::MAX);
        for (index, word) in self.text.split_whitespace().take(fitting).enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            f.write_str(word)?;
        }
        if cut.is_some() {
            f.write_str("…")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::joins_whitespace("a  b\n\tc", 10, "a b c")]
    #[case::fits("one two", 8, "one two")]
    #[case::cut_at_a_word("one two three", 8, "one two…")]
    #[case::counts_characters("ééé ééé", 4, "ééé…")]
    #[case::blank(" \n ", 5, "")]
    #[case::exactly_full("one two", 7, "one two")]
    #[case::one_over("one two", 6, "one…")]
    fn one_line(#[case] text: &str, #[case] max_chars: usize, #[case] expected: &str) {
        let line = OneLine { text, max_chars };
        assert_eq!(line.to_string(), expected);
        assert_eq!(line.is_cut(), expected.ends_with('…'));
    }
}
