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

/// Const-time membership test: is `name` one of `candidates`?
///
/// A `const fn` stand-in for `candidates.contains(&name)`, which is unavailable
/// in const context (slice/str equality and iterators are not yet const), so
/// the comparison is spelled out byte by byte.
#[must_use]
pub const fn is_one_of(name: &str, candidates: &[&str]) -> bool {
    let name = name.as_bytes();
    let mut i = 0;
    while i < candidates.len() {
        let c = candidates[i].as_bytes();
        if c.len() == name.len() {
            let mut j = 0;
            let mut eq = true;
            while j < name.len() {
                if name[j] != c[j] {
                    eq = false;
                    break;
                }
                j += 1;
            }
            if eq {
                return true;
            }
        }
        i += 1;
    }
    false
}
