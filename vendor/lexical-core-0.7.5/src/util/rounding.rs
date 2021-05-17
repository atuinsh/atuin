//! Rounding-scheme identifiers.

#![allow(dead_code)]

/// Rounding type for float-parsing.
///
/// Defines the IEEE754 rounding scheme to be used during float parsing.
/// In general, this should be set to `NearestTieEven`, the default
/// recommended rounding scheme by IEEE754 for binary and decimal
/// operations.
///
/// # FFI
///
/// For interfacing with FFI-code, this may be approximated by:
/// ```text
/// const int32_t NEAREST_TIE_EVEN = 0;
/// const int32_t NEAREST_TIE_AWAY_ZERO = 1;
/// const int32_t TOWARD_POSITIVE_INFINITY = 2;
/// const int32_t TOWARD_NEGATIVE_INFINITY = 3;
/// const int32_t TOWARD_ZERO = 4;
/// ```
///
/// # Safety
///
/// Assigning any value outside the range `[1-4]` to value of type
/// RoundingKind may invoke undefined-behavior.
#[repr(i32)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum RoundingKind {
    /// Round to the nearest, tie to even.
    NearestTieEven = 0,
    /// Round to the nearest, tie away from zero.
    NearestTieAwayZero = 1,
    /// Round toward positive infinity.
    TowardPositiveInfinity = 2,
    /// Round toward negative infinity.
    TowardNegativeInfinity = 3,
    /// Round toward zero.
    TowardZero = 4,

    // Hide the internal implementation details, for how we implement
    // TowardPositiveInfinity, TowardNegativeInfinity, and TowardZero.

    /// Round to increase the magnitude of the float.
    /// For example, for a negative number, this rounds to negative infinity,
    /// for a positive number, to positive infinity.
    #[doc(hidden)]
    Upward = -1,

    /// Round to decrease the magnitude of the float.
    /// This always rounds toward zero.
    #[doc(hidden)]
    Downward = -2,
}

/// Determine if we are rounding to the nearest value, then tying away.
#[inline]
pub(crate) fn is_nearest(kind: RoundingKind) -> bool {
    kind == RoundingKind::NearestTieEven || kind == RoundingKind::NearestTieAwayZero
}

/// Determine if we are rounding to the nearest value, then tying away.
#[inline]
pub(crate) fn is_toward(kind: RoundingKind) -> bool {
    !is_nearest(kind)
}
