//! C-compatible error type.

/// Error code, indicating failure type.
///
/// Error messages are designating by an error code of less than 0.
/// This is to be compatible with C conventions. This enumeration is
/// FFI-compatible for interfacing with C code.
///
/// # FFI
///
/// For interfacing with FFI-code, this may be approximated by:
/// ```text
/// const int32_t OVERFLOW = -1;
/// const int32_t UNDERFLOW = -2;
/// const int32_t INVALID_DIGIT = -3;
/// const int32_t EMPTY = -4;
/// const int32_t EMPTY_FRACTION = -5;
/// const int32_t EMPTY_EXPONENT = -6;
/// ```
///
/// # Safety
///
/// Assigning any value outside the range `[-6, -1]` to value of type
/// ErrorCode may invoke undefined-behavior.
#[repr(i32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ErrorCode {
    /// Integral overflow occurred during numeric parsing.
    ///
    /// Numeric overflow takes precedence over the presence of an invalid
    /// digit.
    Overflow = -1,
    /// Integral underflow occurred during numeric parsing.
    ///
    /// Numeric overflow takes precedence over the presence of an invalid
    /// digit.
    Underflow = -2,
    /// Invalid digit found before string termination.
    InvalidDigit = -3,
    /// Empty byte array found.
    Empty = -4,
    /// Empty mantissa found.
    EmptyMantissa = -5,
    /// Empty exponent found.
    EmptyExponent = -6,
    /// Empty integer found.
    EmptyInteger = -7,
    /// Empty fraction found.
    EmptyFraction = -8,
    /// Invalid positive mantissa sign was found.
    InvalidPositiveMantissaSign = -9,
    /// Mantissa sign was required, but not found.
    MissingMantissaSign = -10,
    /// Exponent was present but not allowed.
    InvalidExponent = -11,
    /// Invalid positive exponent sign was found.
    InvalidPositiveExponentSign = -12,
    /// Exponent sign was required, but not found.
    MissingExponentSign = -13,
    /// Exponent was present without fraction component.
    ExponentWithoutFraction = -14,
    /// Integer had invalid leading zeros.
    InvalidLeadingZeros = -15,

    // We may add additional variants later, so ensure that client matching
    // does not depend on exhaustive matching.
    #[doc(hidden)]
    __Nonexhaustive = -200,
}

/// Error type for lexical parsing.
///
/// This error is FFI-compatible for interfacing with C code.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Error {
    /// Error code designating the type of error occurred.
    pub code: ErrorCode,
    /// Optional position within the buffer for the error.
    pub index: usize,
}

impl From<ErrorCode> for Error {
    #[inline]
    fn from(code: ErrorCode) -> Self {
        Error { code: code, index: 0 }
    }
}

impl From<(ErrorCode, usize)> for Error {
    #[inline]
    fn from(error: (ErrorCode, usize)) -> Self {
        Error { code: error.0, index: error.1 }
    }
}
