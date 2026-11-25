//! String-related utilities and extension traits.

use std::borrow::Cow;
use std::fmt::{self, Write as _};

#[cfg(feature = "unicode")]
use unicode_width::UnicodeWidthStr;
use url::{Position, Url};

#[cfg(feature = "unicode")]
pub mod align;
pub mod bounded_buffer;
#[cfg(feature = "unicode")]
pub mod ellipsis;
pub mod highlighted;
pub mod trim;
#[cfg(feature = "unicode")]
mod wrap;

mod auto_os_string;
mod bounded;
mod escape_non_printable_posix_ext;
mod excerpt;
mod non_blank;
mod non_nul_str;
mod one_line;

#[allow(clippy::manual_range_contains, clippy::must_use_candidate, reason = "vendored file")]
mod normalize;

#[cfg(feature = "unicode")]
pub use align::{AlignExt, Alignment};
pub use auto_os_string::AutoOsString;
pub use bounded::{Bounded, TooLong};
pub use bounded_buffer::BoundedBuffer;
#[cfg(feature = "unicode")]
pub use ellipsis::EllipsizeExt;
pub use escape_non_printable_posix_ext::EscapeNonPrintablePosixExt;
pub use excerpt::excerpt_around;
pub use non_blank::{Blank, NonBlank, NonBlankString};
pub use non_nul_str::{ContainsNul, NonNulStr};
pub use normalize::normalize;
pub use one_line::OneLine;
pub use trim::TrimExt;
#[cfg(feature = "unicode")]
pub use wrap::{Wrap, wrap};

pub trait StrExt {
    /// Like [`str::lines`], but also returns the byte index of each line.
    fn line_indices(&self) -> impl Iterator<Item = (usize, &str)>;

    /// Truncate the string to a specified number of Unicode scalar values.
    fn truncate_chars(&self, max_chars: usize) -> &str;

    /// Normalize Latin diacritics to their ASCII equivalents (`é` -> `e`).
    fn normalize_diacritics(&self) -> Cow<'_, str>;

    /// Check whether the string consists entirely of whitespace or is empty.
    fn is_space_or_empty(&self) -> bool;

    /// Like [`str::find`] with a string pattern, comparing the lowercase forms of the characters.
    fn find_ignoring_case(&self, needle: &str) -> Option<usize>;

    /// Like [`str::contains`] with a string pattern, leaving whitespace out of both strings.
    ///
    /// For example, `"Your code is 123456"` holds `"Your code\nis 123456"`.
    fn contains_ignoring_whitespace(&self, needle: &str) -> bool;

    /// Like `==`, comparing the lowercase forms of the characters.
    fn eq_ignoring_case(&self, other: &str) -> bool;

    /// The lowercase forms of the characters, which [`Self::eq_ignoring_case`] compares.
    fn lowercase_chars(&self) -> impl Iterator<Item = char>;

    /// Like [`str::strip_prefix`], for dotted keys such as `a.b.c`, matching whole segments.
    ///
    /// Returns the segments after `prefix`, which are empty when the key is `prefix` itself. For
    /// example, `"a.b.c"` strips `"a"` to `"b.c"`. `"ab.c"` doesn't strip `"a"`, because `"ab"` is
    /// a different segment. An empty `prefix` is the root, which every key is inside.
    fn strip_dotted_prefix(&self, prefix: &str) -> Option<&str>;

    /// Whether the dotted key is `prefix`, or a key inside `prefix`.
    ///
    /// See [`Self::strip_dotted_prefix`].
    fn has_dotted_prefix(&self, prefix: &str) -> bool {
        self.strip_dotted_prefix(prefix).is_some()
    }
}

#[allow(clippy::use_self, reason = "`str` is more readable")]
impl StrExt for str {
    fn line_indices(&self) -> impl Iterator<Item = (usize, &str)> {
        let mut newlines = self.match_indices('\n').map(|(i, _)| i);
        let mut start = 0;

        std::iter::from_fn(move || {
            let (line, next) = if let Some(i) = newlines.next() {
                let line = &self[start..i];
                (line.strip_suffix('\r').unwrap_or(line), i + 1)
            } else if start < self.len() {
                (&self[start..], self.len())
            } else {
                return None;
            };
            Some((std::mem::replace(&mut start, next), line))
        })
    }

    fn truncate_chars(&self, max_chars: usize) -> &str {
        if self.len() <= max_chars {
            return self;
        }

        match self.char_indices().nth(max_chars) {
            Some((end, _)) => &self[..end],
            None => self,
        }
    }

    fn normalize_diacritics(&self) -> Cow<'_, str> {
        if self.is_ascii() || !self.chars().any(|c| normalize(c) != c) {
            return Cow::Borrowed(self);
        }
        Cow::Owned(self.chars().map(normalize).collect())
    }

    fn is_space_or_empty(&self) -> bool {
        self.chars().all(char::is_whitespace)
    }

    fn find_ignoring_case(&self, needle: &str) -> Option<usize> {
        self.char_indices().map(|(i, _)| i).chain([self.len()]).find(|&i| {
            let mut rest = self[i..].chars().flat_map(char::to_lowercase);
            needle.chars().flat_map(char::to_lowercase).all(|wanted| rest.next() == Some(wanted))
        })
    }

    fn contains_ignoring_whitespace(&self, needle: &str) -> bool {
        fn visible(text: &str) -> impl Iterator<Item = char> {
            text.chars().filter(|c| !c.is_whitespace())
        }
        self.char_indices().map(|(i, _)| i).chain([self.len()]).any(|i| {
            let mut rest = visible(&self[i..]);
            visible(needle).all(|wanted| rest.next() == Some(wanted))
        })
    }

    fn eq_ignoring_case(&self, other: &str) -> bool {
        self.lowercase_chars().eq(other.lowercase_chars())
    }

    fn lowercase_chars(&self) -> impl Iterator<Item = char> {
        self.chars().flat_map(char::to_lowercase)
    }

    fn strip_dotted_prefix(&self, prefix: &str) -> Option<&str> {
        if prefix.is_empty() {
            return Some(self);
        }
        let rest = self.strip_prefix(prefix)?;
        if rest.is_empty() {
            Some(rest)
        } else {
            rest.strip_prefix('.')
        }
    }
}

/// Puts the lines of `text` on one line, in `text`'s own buffer.
///
/// Line breaks become spaces, each run of spaces becomes one space, and the result has no space
/// at either end.
#[must_use]
pub fn join_lines(text: String) -> String {
    let mut bytes = text.into_bytes();
    for byte in &mut bytes {
        if matches!(*byte, b'\n' | b'\r') {
            *byte = b' ';
        }
    }
    bytes.dedup_by(|byte, previous| *byte == b' ' && *previous == b' ');
    if bytes.last() == Some(&b' ') {
        bytes.pop();
    }
    if bytes.first() == Some(&b' ') {
        bytes.remove(0);
    }
    // Only ASCII bytes changed or went away, so the bytes are still UTF-8.
    String::from_utf8(bytes).expect("joining lines keeps UTF-8")
}

/// Returns `text` with `inserted` before each character at one of `offsets`.
///
/// The offsets count characters and come in ascending order. An offset at or past the end of
/// `text` inserts nothing.
#[must_use]
pub fn insert_before_chars(
    text: &str,
    inserted: char,
    offsets: impl IntoIterator<Item = usize>,
) -> String {
    let mut offsets = offsets.into_iter().peekable();
    let mut out = String::with_capacity(text.len() + offsets.size_hint().0 * inserted.len_utf8());
    for (offset, c) in text.chars().enumerate() {
        if offsets.next_if_eq(&offset).is_some() {
            out.push(inserted);
        }
        out.push(c);
    }
    out
}

/// Extension methods for [`String`].
// TODO(taylordotfish): fold `TrimExt` into this trait.
pub trait StringExt {
    /// Replace `old` with `new` within the first occurrence of `context` in `self`, in place.
    ///
    /// # Errors
    ///
    /// If `self` doesn't contain `context`, or `context` doesn't contain `old`. `self` is then
    /// left as it was.
    fn replace_within(
        &mut self,
        context: &str,
        old: &str,
        new: &str,
    ) -> Result<(), ReplaceWithinError>;
}

impl StringExt for String {
    fn replace_within(
        &mut self,
        context: &str,
        old: &str,
        new: &str,
    ) -> Result<(), ReplaceWithinError> {
        let context_start = self.find(context).ok_or(ReplaceWithinError::MissingContext)?;
        let start = context_start + context.find(old).ok_or(ReplaceWithinError::MissingOld)?;
        self.replace_range(start..start + old.len(), new);
        Ok(())
    }
}

/// Error returned by [`StringExt::replace_within`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReplaceWithinError {
    #[error("the string doesn't contain the context")]
    MissingContext,
    #[error("the context doesn't contain the text to replace")]
    MissingOld,
}

/// Extension trait for owned strings providing an empty-string fallback.
pub trait NonEmptyOrExt: Sized {
    /// Return the string if it is non-empty, otherwise `value`.
    #[must_use]
    fn nonempty_or(self, value: Self) -> Self {
        self.nonempty_or_else(|| value)
    }

    /// Same as [`Self::nonempty_or`] but takes a factory function.
    #[must_use]
    fn nonempty_or_else(self, default: impl FnOnce() -> Self) -> Self;
}

impl NonEmptyOrExt for String {
    fn nonempty_or_else(self, default: impl FnOnce() -> Self) -> Self {
        if self.is_empty() {
            default()
        } else {
            self
        }
    }
}

impl NonEmptyOrExt for &str {
    fn nonempty_or_else(self, default: impl FnOnce() -> Self) -> Self {
        if self.is_empty() {
            default()
        } else {
            self
        }
    }
}

impl<T> NonEmptyOrExt for &[T] {
    fn nonempty_or_else(self, default: impl FnOnce() -> Self) -> Self {
        if self.is_empty() {
            default()
        } else {
            self
        }
    }
}

impl<T> NonEmptyOrExt for Vec<T> {
    fn nonempty_or_else(self, default: impl FnOnce() -> Self) -> Self {
        if self.is_empty() {
            default()
        } else {
            self
        }
    }
}

/// Extension trait for [`Url`] to render a `Debug` representation with any
/// password redacted.
pub trait FormatSafeUrlExt {
    /// Debug-format the URL with its password replaced by `****`.
    fn format_safe(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

impl FormatSafeUrlExt for Url {
    fn format_safe(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.password().is_none() {
            return fmt::Debug::fmt(self.as_str(), f);
        }

        f.write_char('"')?;
        for c in self[..Position::BeforePassword].escape_debug() {
            f.write_char(c)?;
        }
        f.write_str("****")?;
        for c in self[Position::AfterPassword..].escape_debug() {
            f.write_char(c)?;
        }
        f.write_char('"')
    }
}

/// How much room to truncate or pad into, and the unit it is measured in.
#[cfg(feature = "unicode")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    /// A UTF-8 byte budget.
    Bytes(usize),
    /// A display-column budget via `unicode-width` - a double-width glyph such
    /// as `世` or `🦀` counts as two. Use for presentation.
    Columns(usize),
}

#[cfg(feature = "unicode")]
impl Measure {
    /// The numeric limit, in this budget's own unit.
    pub(crate) fn amount(self) -> usize {
        match self {
            Self::Bytes(n) | Self::Columns(n) => n,
        }
    }

    /// Total cost of `s` in this budget's unit.
    pub(crate) fn cost(self, s: &str) -> usize {
        match self {
            Self::Bytes(_) => s.len(),
            Self::Columns(_) => s.width(),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;
    use url::Url;

    use super::{
        FormatSafeUrlExt, NonEmptyOrExt, ReplaceWithinError, StrExt, StringExt as _,
        insert_before_chars, join_lines,
    };

    #[rstest]
    #[case::between_words("Yesisgood", &[3, 5], "Yes is good")]
    #[case::none("ab", &[], "ab")]
    #[case::after_a_wide_character("日本", &[1], "日 本")]
    #[case::past_the_end("ab", &[2], "ab")]
    fn insertions_go_before_their_offsets(
        #[case] text: &str,
        #[case] offsets: &[usize],
        #[case] expected: &str,
    ) {
        assert_eq!(insert_before_chars(text, ' ', offsets.iter().copied()), expected);
    }

    #[rstest]
    #[case::one_line("Error while copying.", "Error while copying.")]
    #[case::two_lines("Error while copying.\nThe folder", "Error while copying. The folder")]
    #[case::blank_lines_and_crlf("\r\na\r\n\r\n  b \n", "a b")]
    #[case::multibyte("“atuin-1003”\nkann nicht", "“atuin-1003” kann nicht")]
    #[case::only_breaks("\n\n", "")]
    fn join_lines_puts_lines_on_one_line(#[case] text: &str, #[case] expected: &str) {
        assert_eq!(join_lines(text.to_owned()), expected);
    }

    proptest! {
        #[test]
        fn join_lines_matches_joining_the_words(text in "[a-zé \r\n]{0,40}") {
            let words: Vec<_> = text.split([' ', '\r', '\n']).filter(|w| !w.is_empty()).collect();
            prop_assert_eq!(join_lines(text.clone()), words.join(" "));
        }
    }

    /// Only the text within the context changes, even when the same text comes earlier in the
    /// string.
    #[rstest]
    #[case::alone("const A = \"pi\";\n", "const A = \"omp\";\n")]
    #[case::earlier_in_the_string("pipe();\nconst A = \"pi\";\n", "pipe();\nconst A = \"omp\";\n")]
    #[case::first_of_two_contexts(
        "const A = \"pi\";\nconst A = \"pi\";\n",
        "const A = \"omp\";\nconst A = \"pi\";\n"
    )]
    fn replace_within_changes_only_the_context(#[case] s: &str, #[case] want: &str) {
        let mut s = s.to_owned();
        s.replace_within(r#"const A = "pi";"#, "pi", "omp").unwrap();
        assert_eq!(s, want);
    }

    /// Without the context, or without the old text in it, the string stays as it was.
    #[rstest]
    #[case::no_context(r#"const B = "pi";"#, "pi", ReplaceWithinError::MissingContext)]
    #[case::no_old_text(r#"const A = "pi";"#, "omp", ReplaceWithinError::MissingOld)]
    fn replace_within_without_the_text_changes_nothing(
        #[case] context: &str,
        #[case] old: &str,
        #[case] want: ReplaceWithinError,
    ) {
        let mut s = r#"const A = "pi";"#.to_owned();
        assert_eq!(s.replace_within(context, old, "new"), Err(want));
        assert_eq!(s, r#"const A = "pi";"#);
    }

    #[rstest]
    #[case::the_key_itself("a.b", "a.b", Some(""))]
    #[case::a_parent_table("a.b.c", "a", Some("b.c"))]
    #[case::a_nested_parent_table("a.b.c", "a.b", Some("c"))]
    #[case::the_root("a.b", "", Some("a.b"))]
    #[case::a_longer_segment("ab.c", "a", None)]
    #[case::a_shorter_segment("a.c", "ab", None)]
    #[case::a_child("a", "a.b", None)]
    #[case::a_sibling("a.c", "a.b", None)]
    fn strip_dotted_prefix(
        #[case] key: &str,
        #[case] prefix: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(key.strip_dotted_prefix(prefix), expected);
        assert_eq!(key.has_dotted_prefix(prefix), expected.is_some());
    }

    proptest! {
        #[test]
        fn strip_dotted_prefix_returns_the_segments_after_the_prefix(
            prefix in "[a-z_]{1,8}(\\.[a-z_]{1,8}){0,3}",
            rest in "[a-z_]{1,8}(\\.[a-z_]{1,8}){0,3}",
        ) {
            let key = format!("{prefix}.{rest}");
            prop_assert_eq!(key.strip_dotted_prefix(&prefix), Some(rest.as_str()));
            prop_assert_eq!(prefix.strip_dotted_prefix(&prefix), Some(""));
        }
    }

    #[rstest]
    #[case::same_case("a needle", "needle", Some(2))]
    #[case::other_case("a NeEdLe", "nEeDlE", Some(2))]
    #[case::the_first_match("xx xx", "XX", Some(0))]
    #[case::after_multibyte("é x", "X", Some(3))]
    #[case::non_ascii_case("Ça va: ÉCOLE", "école", Some(8))]
    #[case::missing("abc", "abcd", None)]
    #[case::empty_needle("abc", "", Some(0))]
    #[case::empty_haystack("", "", Some(0))]
    fn find_ignoring_case(
        #[case] haystack: &str,
        #[case] needle: &str,
        #[case] expected: Option<usize>,
    ) {
        assert_eq!(haystack.find_ignoring_case(needle), expected);
    }

    #[rstest]
    #[case::same("a needle here", "needle", true)]
    #[case::a_line_break("Your code is 123456", "Your code\nis 123456", true)]
    #[case::spaces_in_the_haystack("Your  code is\t123456", "code is 123456", true)]
    #[case::no_space_at_all("Yourcode", "Your code", true)]
    #[case::after_multibyte("é x", "x", true)]
    #[case::missing("Your code", "Your codes", false)]
    #[case::other_case("Needle", "needle", false)]
    #[case::empty_needle("abc", "", true)]
    #[case::blank_needle("abc", " \n", true)]
    #[case::empty_haystack("", "a", false)]
    fn contains_ignoring_whitespace(
        #[case] haystack: &str,
        #[case] needle: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(haystack.contains_ignoring_whitespace(needle), expected);
    }

    #[rstest]
    #[case::same("Firefox", "Firefox", true)]
    #[case::ascii_case("FIREFOX", "firefox", true)]
    #[case::non_ascii_case("Écrire", "éCRIRE", true)]
    #[case::greek("ΣΟΦΙΑ", "σοφια", true)]
    #[case::other_text("kate", "kwrite", false)]
    #[case::prefix("kate", "kat", false)]
    #[case::empty("", "", true)]
    fn eq_ignoring_case(#[case] a: &str, #[case] b: &str, #[case] expected: bool) {
        assert_eq!(a.eq_ignoring_case(b), expected);
        assert_eq!(b.eq_ignoring_case(a), expected);
    }

    #[rstest]
    #[case::empty("", &[])]
    #[case::one_line("a", &[(0, "a")])]
    #[case::trailing_newline("a\n", &[(0, "a")])]
    #[case::two_lines("a\nbc", &[(0, "a"), (2, "bc")])]
    #[case::blank_lines("\n\na\n", &[(0, ""), (1, ""), (2, "a")])]
    #[case::crlf("a\r\nb\r\n", &[(0, "a"), (3, "b")])]
    #[case::bare_carriage_return("a\rb\r", &[(0, "a\rb\r")])]
    #[case::multibyte("é\nü", &[(0, "é"), (3, "ü")])]
    fn line_indices(#[case] input: &str, #[case] expected: &[(usize, &str)]) {
        // Bounded so a non-terminating iterator fails rather than exhausting memory.
        let indexed: Vec<_> = input.line_indices().take(expected.len() + 1).collect();
        assert_eq!(indexed, expected);
    }

    proptest! {
        /// The lines are those of [`str::lines`], each at the index it starts at.
        #[test]
        fn line_indices_are_the_lines_and_where_they_start(input in "[a\r\n é]{0,20}") {
            // A string has at most one line per byte; the bound stops a
            // non-terminating iterator from exhausting memory.
            let indexed: Vec<_> = input.line_indices().take(input.len() + 1).collect();
            let lines: Vec<_> = indexed.iter().map(|&(_, line)| line).collect();
            prop_assert_eq!(lines, input.lines().collect::<Vec<_>>());

            for (i, line) in indexed {
                prop_assert!(input[i..].starts_with(line));
                prop_assert!(i == 0 || input[..i].ends_with('\n'));
            }
        }
    }

    #[rstest]
    #[case::empty("", "")]
    #[case::ascii_unchanged("hello world", "hello world")]
    #[case::accented("café", "cafe")]
    #[case::keeps_unmappable("naïve Æ", "naive Æ")] // ï -> i, but Æ has no single-ASCII mapping
    #[case::position_preserved("élève", "eleve")]
    fn normalizes_diacritics(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(input.normalize_diacritics(), expected);
    }

    #[rstest]
    #[case::empty("")]
    #[case::plain_ascii("just ascii text")]
    #[case::unmappable("日本語")]
    fn normalize_diacritics_borrows_when_unchanged(#[case] input: &str) {
        assert!(matches!(input.normalize_diacritics(), std::borrow::Cow::Borrowed(_)));
    }

    #[rstest]
    #[case::under_budget("hello", 10, "hello")]
    #[case::exact_budget("hello", 5, "hello")]
    #[case::over_budget("hello", 3, "hel")] // codespell:ignore hel
    #[case::zero("hello", 0, "")]
    #[case::empty("", 5, "")]
    #[case::multibyte_cut("café", 3, "caf")] // never splits a multibyte char; codespell:ignore caf
    #[case::multibyte_kept("café", 4, "café")]
    fn truncates_by_char_count(#[case] input: &str, #[case] max: usize, #[case] expected: &str) {
        let out = input.truncate_chars(max);
        assert_eq!(out, expected);
        assert!(out.chars().count() <= max);
    }

    #[rstest]
    #[case::empty("", "fallback")]
    #[case::non_empty("value", "value")]
    fn nonempty_or_falls_back_only_when_empty(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(input.to_string().nonempty_or_else(|| "fallback".into()), expected);
    }

    #[rstest]
    fn truncate_chars_returns_the_original_slice_when_it_fits() {
        let s = "borrow me";
        assert!(std::ptr::eq(s.truncate_chars(100), s));
    }

    struct Safe<'a>(&'a Url);

    impl std::fmt::Debug for Safe<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.0.format_safe(f)
        }
    }

    fn safe(url: &str) -> String {
        format!("{:?}", Safe(&Url::parse(url).unwrap()))
    }

    #[rstest]
    #[case::redacts_password(
        "postgres://user:hunter2@localhost/db",
        r#""postgres://user:****@localhost/db""#
    )]
    #[case::passwordless_unchanged(
        "postgres://user@localhost/db",
        r#""postgres://user@localhost/db""#
    )]
    #[case::empty_password_dropped("mysql://user:@localhost/db", r#""mysql://user@localhost/db""#)]
    fn format_safe_redacts(#[case] url: &str, #[case] expected: &str) {
        assert_eq!(safe(url), expected);
    }
}
