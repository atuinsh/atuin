//! Helpers for tests of text that holds terminal escape sequences.

/// `text` without its SGR escape sequences, which set colors and attributes.
#[must_use]
pub fn strip_sgr(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            chars.by_ref().find(|&c| c == 'm');
        } else {
            plain.push(c);
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::plain("abc", "abc")]
    #[case::colored("\u{1b}[31mred\u{1b}[0m text", "red text")]
    #[case::bold_and_color("\u{1b}[1;38;5;208mx\u{1b}[39;22m", "x")]
    fn strips(#[case] text: &str, #[case] expected: &str) {
        assert_eq!(strip_sgr(text), expected);
    }
}
