//! String-related utilities and extension traits.
#[cfg(feature = "unicode")]
pub mod align;
#[cfg(feature = "unicode")]
pub mod ellipsis;
mod escape_non_printable_posix_ext;
mod non_nul_str;

#[cfg(feature = "unicode")]
pub use align::{AlignExt, Alignment};
#[cfg(feature = "unicode")]
pub use ellipsis::EllipsizeExt;
pub use escape_non_printable_posix_ext::EscapeNonPrintablePosixExt;
pub use non_nul_str::{ContainsNul, NonNulStr};

#[cfg(feature = "unicode")]
use unicode_width::UnicodeWidthStr;

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
            Measure::Bytes(n) | Measure::Columns(n) => n,
        }
    }

    /// Total cost of `s` in this budget's unit.
    pub(crate) fn cost(self, s: &str) -> usize {
        match self {
            Measure::Bytes(_) => s.len(),
            Measure::Columns(_) => s.width(),
        }
    }
}
