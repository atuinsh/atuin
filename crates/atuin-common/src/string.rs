//! String-related utilities and extension traits.

use std::borrow::Cow;
use std::fmt::{self, Write as _};

#[cfg(feature = "unicode")]
use unicode_width::UnicodeWidthStr;
use url::{Position, Url};

#[cfg(feature = "unicode")]
pub mod align;
#[cfg(feature = "unicode")]
pub mod ellipsis;
pub mod trim;

mod buffer;
mod escape_non_printable_posix_ext;
mod non_nul_str;

#[allow(clippy::manual_range_contains, clippy::must_use_candidate, reason = "vendored file")]
mod normalize;

#[cfg(feature = "unicode")]
pub use align::{AlignExt, Alignment};
pub use buffer::BoundedBuffer;
#[cfg(feature = "unicode")]
pub use ellipsis::EllipsizeExt;
pub use escape_non_printable_posix_ext::EscapeNonPrintablePosixExt;
pub use non_nul_str::{ContainsNul, NonNulStr};
pub use normalize::normalize;
pub use trim::TrimExt;

pub trait TruncateCharsExt: AsRef<str> {
    fn truncate_chars(&self, max_chars: usize) -> &str {
        let s = self.as_ref();
        if s.len() <= max_chars {
            return s;
        }

        match s.char_indices().nth(max_chars) {
            Some((end, _)) => &s[..end],
            None => s,
        }
    }
}

impl<T: AsRef<str> + ?Sized> TruncateCharsExt for T {}

/// Extension trait adding diacritic normalization to string slices.
pub trait NormalizeDiacriticsExt: AsRef<str> {
    /// Normalize Latin diacritics to their ASCII equivalents (`é` -> `e`).
    fn normalize_diacritics(&self) -> Cow<'_, str> {
        let s = self.as_ref();
        if s.is_ascii() || !s.chars().any(|c| normalize(c) != c) {
            return Cow::Borrowed(s);
        }
        Cow::Owned(s.chars().map(normalize).collect())
    }
}

impl<T: AsRef<str> + ?Sized> NormalizeDiacriticsExt for T {}

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
    use rstest::rstest;
    use url::Url;

    use super::{FormatSafeUrlExt, NormalizeDiacriticsExt, TruncateCharsExt};

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

    #[test]
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
