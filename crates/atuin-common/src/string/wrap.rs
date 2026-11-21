//! Word wrapping for plain text.

use unicode_width::UnicodeWidthStr as _;

/// The lines of `text` broken between words, each at most `columns` wide.
///
/// A word that is wider than `columns` has a line to itself. A newline in `text` always ends a
/// line. Each line is a slice of `text`, so the whitespace between two words on one line is kept
/// as `text` has it.
#[must_use]
pub fn wrap(text: &str, columns: usize) -> Wrap<'_> {
    Wrap {
        rest: text,
        columns,
    }
}

/// The iterator that [`wrap`] returns.
#[derive(Debug, Clone)]
pub struct Wrap<'a> {
    rest: &'a str,
    columns: usize,
}

impl<'a> Iterator for Wrap<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let text = self.rest.trim_start();
        if text.is_empty() {
            self.rest = text;
            return None;
        }

        // `text` starts with a word, so the first pass always takes it.
        let mut end = 0;
        let mut width = 0;
        while let Some(start) = text[end..].find(|c: char| !c.is_whitespace()).map(|at| end + at) {
            let stop = text[start..].find(char::is_whitespace).map_or(text.len(), |at| start + at);
            let gap = &text[end..start];
            let wider = width + gap.width() + text[start..stop].width();
            if end > 0 && (gap.contains('\n') || wider > self.columns) {
                break;
            }
            end = stop;
            width = wider;
        }

        self.rest = &text[end..];
        Some(&text[..end])
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;
    use unicode_width::UnicodeWidthStr as _;

    use super::wrap;

    #[rstest]
    #[case::fits("a few words", 20, &["a few words"])]
    #[case::between_words("one two three", 7, &["one two", "three"])]
    #[case::a_long_word_alone("a extraordinarily b", 5, &["a", "extraordinarily", "b"])]
    #[case::nothing("", 5, &[])]
    #[case::only_whitespace(" \n ", 5, &[])]
    #[case::outer_whitespace_dropped("  a b  ", 5, &["a b"])]
    #[case::a_newline_ends_a_line("a\nb c", 10, &["a", "b c"])]
    #[case::wide_glyphs("世界 世界", 4, &["世界", "世界"])]
    fn wrap_breaks_between_words(
        #[case] text: &str,
        #[case] columns: usize,
        #[case] expected: &[&str],
    ) {
        // Bounded so a non-terminating iterator fails rather than exhausting memory.
        let lines: Vec<_> = wrap(text, columns).take(expected.len() + 1).collect();
        assert_eq!(lines, expected);
    }

    proptest! {
        /// The lines hold every word in order, and only a line of one word is too wide.
        #[test]
        fn wrap_keeps_every_word_and_fits_the_lines(text in "[a-z \n]{0,40}", columns in 1..12usize) {
            // A string has at most one line per byte; the bound stops a non-terminating iterator.
            let lines: Vec<_> = wrap(&text, columns).take(text.len() + 1).collect();
            let words: Vec<_> = lines.iter().flat_map(|line| line.split_whitespace()).collect();
            prop_assert_eq!(words, text.split_whitespace().collect::<Vec<_>>());
            for line in lines {
                prop_assert!(line.width() <= columns || !line.contains(char::is_whitespace));
            }
        }
    }
}
