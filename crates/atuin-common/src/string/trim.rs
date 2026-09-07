mod sealed {
    pub trait Sealed {}
}

/// A pattern that, unlike [`std::str::pattern::Pattern`], does not consume the pattern when used.
// Because `std::str::pattern::Pattern` is unstable, we cannot implement `PatternRef` in terms of
// it. Ideally this trait would be very simple -- an associated type `Self::Pattern<'a>` that
// implements `std::str::pattern::Pattern`, and a method to go from `&'a mut Self` to
// `Self::Pattern<'a>`. But because the standard library trait is unstable, we need to implement
// every `Pattern`-accepting `str` method we want to use here.
pub trait PatternRef: sealed::Sealed {
    fn trim_start_matches<'a>(&mut self, s: &'a str) -> &'a str;
    fn trim_end_matches<'a>(&mut self, s: &'a str) -> &'a str;
}

macro_rules! impl_pattern_ref {
    ([$($gen:tt)*], $ty:ty, $to_pattern:expr) => {
        impl<$($gen)*> sealed::Sealed for $ty {}

        impl<$($gen)*> PatternRef for $ty {
            fn trim_start_matches<'a>(&mut self, s: &'a str) -> &'a str {
                s.trim_start_matches(($to_pattern)(self))
            }

            fn trim_end_matches<'a>(&mut self, s: &'a str) -> &'a str {
                s.trim_end_matches(($to_pattern)(self))
            }
        }
    };
}

// Implement `PatternRef` for all of the types that implement `Pattern`.
impl_pattern_ref!([], char, Clone::clone);
impl_pattern_ref!([const N: usize], [char; N], Clone::clone);
impl_pattern_ref!([const N: usize], &[char; N], Clone::clone);
impl_pattern_ref!([], &[char], Clone::clone);
impl_pattern_ref!([], &str, Clone::clone);
impl_pattern_ref!([], &&str, Clone::clone);
impl_pattern_ref!([F: FnMut(char) -> bool], F, std::convert::identity);

pub trait TrimExt {
    /// Like [`str::trim_matches`], but modifies the [`String`] in-place instead of returning a
    /// substring.
    fn trim_matches_in_place<P: PatternRef>(&mut self, pattern: P);

    /// Like [`str::trim`], but modifies the [`String`] in-place instead of returning a substring.
    fn trim_in_place(&mut self) {
        self.trim_matches_in_place(char::is_whitespace);
    }
}

impl TrimExt for String {
    fn trim_matches_in_place<P>(&mut self, mut pattern: P)
    where
        P: PatternRef,
    {
        self.truncate(pattern.trim_end_matches(self).len());
        self.drain(..self.len() - pattern.trim_start_matches(self).len());
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::TrimExt;

    /// Run `trim_matches_in_place` over an owned copy of `input`.
    fn trimmed(input: &str, pattern: impl super::PatternRef) -> String {
        let mut string = input.to_string();
        string.trim_matches_in_place(pattern);
        string
    }

    #[rstest]
    #[case::both_ends("xxhixx", "hi")]
    #[case::leading_only("xxhi", "hi")]
    #[case::trailing_only("hixx", "hi")]
    #[case::interior_kept("xhixhix", "hixhi")]
    #[case::no_match("hi", "hi")]
    #[case::all_pattern("xxxx", "")]
    #[case::empty("", "")]
    #[case::single_char("x", "")]
    fn trims_a_char_pattern(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(trimmed(input, 'x'), expected);
    }

    #[rstest]
    #[case::blank_lines_and_spaces("\n\n  hi there  \n\n", "hi there")]
    #[case::mixed_run(" \n \n hi", "hi")]
    #[case::interior_newline_kept("\none\ntwo\n", "one\ntwo")]
    fn trims_a_char_array_pattern(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(trimmed(input, ['\n', ' ']), expected);
        assert_eq!(trimmed(input, &['\n', ' ']), expected);
        // A slice, too: `str::trim_matches` accepts one, so `PatternRef` has to as well.
        assert_eq!(trimmed(input, &['\n', ' '][..]), expected);
    }

    #[rstest]
    #[case::str_pattern("abcXabc", "abc", "X")]
    #[case::repeated_str_pattern("abcabcXabcabc", "abc", "X")]
    #[case::partial_match_kept("abXab", "abc", "abXab")]
    fn trims_a_str_pattern(#[case] input: &str, #[case] pattern: &str, #[case] expected: &str) {
        assert_eq!(trimmed(input, pattern), expected);
        assert_eq!(trimmed(input, &pattern), expected);
    }

    #[rstest]
    #[case::digits("123hi456", "hi")]
    #[case::only_digits("123", "")]
    #[case::interior_digits_kept("1h2i3", "h2i")]
    #[case::nothing_to_trim("hi", "hi")]
    fn trims_a_closure_pattern(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(trimmed(input, |c: char| c.is_ascii_digit()), expected);
    }

    #[rstest]
    #[case::multibyte_pattern("——hi——", '—', "hi")]
    #[case::multibyte_content_preserved("xx🦀 世界xx", 'x', "🦀 世界")]
    #[case::multibyte_content_all_trimmed("🦀🦀", '🦀', "")]
    fn handles_multibyte_characters(
        #[case] input: &str,
        #[case] pattern: char,
        #[case] expected: &str,
    ) {
        assert_eq!(trimmed(input, pattern), expected);
    }

    #[rstest]
    #[case::ascii_whitespace(" \t\r\nhi \t\r\n", "hi")]
    #[case::unicode_whitespace("\u{3000}hi\u{3000}", "hi")]
    #[case::interior_kept("  a b  ", "a b")]
    #[case::nothing_to_trim("hi", "hi")]
    fn trim_in_place_matches_str_trim(#[case] input: &str, #[case] expected: &str) {
        let mut string = input.to_string();
        string.trim_in_place();
        assert_eq!(string, expected);
        assert_eq!(string, input.trim());
    }

    #[rstest]
    fn a_stateful_pattern_is_reused_rather_than_consumed() {
        // The point of `PatternRef`: a single `FnMut` drives both ends.
        let mut string = "abhixy".to_string();
        let mut chars: std::collections::HashSet<char> = string.chars().collect();
        string.trim_matches_in_place(|c| {
            assert!(chars.remove(&c), "matcher called on nonexistent char, or same char twice");
            !c.is_ascii_alphabetic() || "abxy".contains(c)
        });
        assert_eq!(string, "hi");
        assert!(chars.is_empty(), "matcher not called on every char");
    }

    proptest! {
        /// However the pattern and haystack are chosen, trimming in place must agree with
        /// `str::trim_matches` and never leave the string on a non-char boundary.
        #[test]
        fn agrees_with_str_trim_matches(input in ".{0,64}", pattern in prop::char::range('a', 'e')) {
            let mut string = input.clone();
            string.trim_matches_in_place(pattern);
            prop_assert_eq!(string, input.trim_matches(pattern));
        }

        #[test]
        fn agrees_with_str_trim(input in ".{0,64}") {
            let mut string = input.clone();
            string.trim_in_place();
            prop_assert_eq!(string, input.trim());
        }
    }
}
