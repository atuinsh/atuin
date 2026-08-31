//! String-related utilities and extension traits.

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

#[cfg(feature = "unicode")]
pub use align::{AlignExt, Alignment};
pub use buffer::BoundedBuffer;
#[cfg(feature = "unicode")]
pub use ellipsis::EllipsizeExt;
pub use escape_non_printable_posix_ext::EscapeNonPrintablePosixExt;
pub use non_nul_str::{ContainsNul, NonNulStr};
pub use trim::TrimExt;

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

    use super::FormatSafeUrlExt;

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
