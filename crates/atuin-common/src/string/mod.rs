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

mod auto_os_string;
mod escape_non_printable_posix_ext;
mod non_blank;
mod non_nul_str;

#[allow(clippy::manual_range_contains, clippy::must_use_candidate, reason = "vendored file")]
mod normalize;

#[cfg(feature = "unicode")]
pub use align::{AlignExt, Alignment};
pub use auto_os_string::AutoOsString;
pub use bounded_buffer::BoundedBuffer;
#[cfg(feature = "unicode")]
pub use ellipsis::EllipsizeExt;
pub use escape_non_printable_posix_ext::EscapeNonPrintablePosixExt;
pub use non_blank::{Blank, NonBlank, NonBlankString};
pub use non_nul_str::{ContainsNul, NonNulStr};
pub use normalize::normalize;
pub use trim::TrimExt;

pub trait StrExt {
    /// Like [`str::lines`], but also returns the byte index of each line.
    fn line_indices(&self) -> impl Iterator<Item = (usize, &str)>;

    /// Truncate the string to a specified number of Unicode scalar values.
    fn truncate_chars(&self, max_chars: usize) -> &str;

    /// Normalize Latin diacritics to their ASCII equivalents (`é` -> `e`).
    fn normalize_diacritics(&self) -> Cow<'_, str>;

    /// Check whether the string consists entirely of whitespace or is empty.
    fn is_space_or_empty(&self) -> bool;
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

    use super::{FormatSafeUrlExt, NonEmptyOrExt, StrExt};

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
