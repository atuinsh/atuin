//! String-related utilities and extension traits.
#[cfg(feature = "unicode")]
pub mod align;
#[cfg(feature = "unicode")]
pub mod ellipsis;
mod escape_non_printable_posix_ext;

#[cfg(feature = "unicode")]
pub use align::{AlignExt, Alignment};
#[cfg(feature = "unicode")]
pub use ellipsis::EllipsizeExt;
pub use escape_non_printable_posix_ext::EscapeNonPrintablePosixExt;

#[cfg(feature = "unicode")]
use unicode_width::UnicodeWidthStr;

/// How much room to truncate or pad into, and the unit it is measured in.
#[cfg(feature = "unicode")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphWidth {
    /// A UTF-8 byte budget.
    Bytes(usize),
    /// A display-column budget via `unicode-width` - a double-width glyph such
    /// as `世` or `🦀` counts as two. Use for presentation.
    Columns(usize),
}

#[cfg(feature = "unicode")]
impl GlyphWidth {
    /// The numeric limit, in this budget's own unit.
    pub(crate) fn amount(self) -> usize {
        match self {
            GlyphWidth::Bytes(n) | GlyphWidth::Columns(n) => n,
        }
    }

    /// Total cost of `s` in this budget's unit.
    pub(crate) fn cost(self, s: &str) -> usize {
        match self {
            GlyphWidth::Bytes(_) => s.len(),
            GlyphWidth::Columns(_) => s.width(),
        }
    }
}
