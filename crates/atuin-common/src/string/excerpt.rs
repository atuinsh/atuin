//! A short part of a text around the words that a reader looks for.

use super::StrExt as _;

/// About `chars` characters of `text`, centered on the earliest occurrence of any of `words`,
/// ignoring case. Without an occurrence, the start of `text`.
///
/// The excerpt keeps whole characters, and leaves out the whitespace at its ends.
pub fn excerpt_around<'a, 'w>(
    text: &'a str,
    words: impl IntoIterator<Item = &'w str>,
    chars: usize,
) -> &'a str {
    let found =
        words.into_iter().filter_map(|word| text.find_ignoring_case(word)).min().unwrap_or(0);
    let start = text[..found].char_indices().rev().take(chars / 2).last().map_or(found, |(i, _)| i);
    let end = text[start..].char_indices().nth(chars).map_or(text.len(), |(i, _)| start + i);
    text[start..end].trim()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::around_the_word("one two three four five", "FOUR", 8, "ree four")]
    #[case::the_earliest_word("one two three", "three two", 4, "e tw")]
    #[case::the_start_without_a_match("abcdef", "zzz", 3, "abc")]
    #[case::whole_characters("ééé needle", "needle", 4, "é ne")]
    #[case::non_ascii_case("Ça va: ÉCOLE ouverte", "école", 6, "a: ÉCO")]
    #[case::no_words("abcdef", "", 3, "abc")]
    fn excerpts(
        #[case] text: &str,
        #[case] words: &str,
        #[case] chars: usize,
        #[case] expected: &str,
    ) {
        assert_eq!(excerpt_around(text, words.split_whitespace(), chars), expected);
    }
}
