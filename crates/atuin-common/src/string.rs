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

/// Const-time string equality: are `a` and `b` byte-for-byte equal?
///
/// A `const fn` stand-in for `a == b`, which is unavailable in const context
/// (`str`/slice `PartialEq` is not yet const), so the comparison is spelled out
/// byte by byte.
#[must_use]
pub const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Return `s` without its last `n` bytes.
///
/// A `const fn` helper for building a list with a trailing separator, then
/// dropping it (`String::truncate` and slicing are not available in const
/// context). Panics (a compile error, in the const context this is used from)
/// if `n` exceeds the length of `s` or the cut lands inside a UTF-8 sequence -
/// both signal a mismatch between an emitted separator and the count passed
/// here, which must never ship.
#[must_use]
pub const fn strip_tail(s: &str, n: usize) -> &str {
    let b = s.as_bytes();
    // An empty string is the "no elements" case (a list that contributed no
    // items): there is no trailing separator to drop.
    if b.is_empty() {
        return "";
    }
    // A non-empty string always ends with a full separator, so it can never be
    // shorter than one. If it is, the count here disagrees with the separator
    // that was emitted - a bug that must not ship.
    assert!(b.len() >= n, "strip_tail: n exceeds the length of a non-empty string");
    match core::str::from_utf8(b.split_at(b.len() - n).0) {
        Ok(h) => h,
        Err(_) => panic!("strip_tail: cut lands inside a UTF-8 sequence"),
    }
}
