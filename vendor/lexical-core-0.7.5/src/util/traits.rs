//! Wrap the low-level API into idiomatic serializers.

use super::format::NumberFormat;
use super::num::Number;
use super::result::Result;

// HELPERS

/// Map partial result to complete result.
macro_rules! to_complete {
    ($cb:expr, $bytes:expr $(,$args:expr)*) => {
        match $cb($bytes $(,$args)*) {
            Err(e)                  => Err(e),
            Ok((value, processed))  => if processed == $bytes.len() {
                Ok(value)
            } else{
                Err((ErrorCode::InvalidDigit, processed).into())
            }
        }
    };
}

// FROM LEXICAL

/// Trait for numerical types that can be parsed from bytes.
pub trait FromLexical: Number {
    /// Checked parser for a string-to-number conversion.
    ///
    /// This method parses the entire string, returning an error if
    /// any invalid digits are found during parsing.
    ///
    /// Returns a `Result` containing either the parsed value,
    /// or an error containing any errors that occurred during parsing.
    ///
    /// Numeric overflow takes precedence over the presence of an invalid
    /// digit, and therefore may mask an invalid digit error.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    fn from_lexical(bytes: &[u8]) -> Result<Self>;

    /// Checked parser for a string-to-number conversion.
    ///
    /// This method parses until an invalid digit is found (or the end
    /// of the string), returning the number of processed digits
    /// and the parsed value until that point.
    ///
    /// Returns a `Result` containing either the parsed value
    /// and the number of processed digits, or an error containing
    /// any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    fn from_lexical_partial(bytes: &[u8]) -> Result<(Self, usize)>;

    /// Checked parser for a string-to-number conversion.
    ///
    /// This method parses the entire string, returning an error if
    /// any invalid digits are found during parsing.
    ///
    /// Returns a `Result` containing either the parsed value,
    /// or an error containing any errors that occurred during parsing.
    ///
    /// Numeric overflow takes precedence over the presence of an invalid
    /// digit, and therefore may mask an invalid digit error.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `radix`   - Radix for the number parsing.
    ///
    /// # Panics
    ///
    /// Panics if the radix is not in the range `[2, 36]`.
    #[cfg(feature = "radix")]
    fn from_lexical_radix(bytes: &[u8], radix: u8) -> Result<Self>;

    /// Checked parser for a string-to-number conversion.
    ///
    /// This method parses until an invalid digit is found (or the end
    /// of the string), returning the number of processed digits
    /// and the parsed value until that point.
    ///
    /// Returns a `Result` containing either the parsed value
    /// and the number of processed digits, or an error containing
    /// any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `radix`   - Radix for the number parsing.
    ///
    /// # Panics
    ///
    /// Panics if the radix is not in the range `[2, 36]`.
    #[cfg(feature = "radix")]
    fn from_lexical_partial_radix(bytes: &[u8], radix: u8) -> Result<(Self, usize)>;
}

// Implement FromLexical for numeric type.
macro_rules! from_lexical {
    ($cb:expr, $t:ty) => (
        impl FromLexical for $t {
            #[inline]
            fn from_lexical(bytes: &[u8]) -> Result<$t>
            {
                to_complete!($cb, bytes, 10)
            }

            #[inline]
            fn from_lexical_partial(bytes: &[u8]) -> Result<($t, usize)>
            {
                $cb(bytes, 10)
            }

            #[cfg(feature = "radix")]
            #[inline]
            fn from_lexical_radix(bytes: &[u8], radix: u8) -> Result<$t>
            {
                to_complete!($cb, bytes, radix.as_u32())
            }

            #[cfg(feature = "radix")]
            #[inline]
            fn from_lexical_partial_radix(bytes: &[u8], radix: u8) -> Result<($t, usize)>
            {
                $cb(bytes, radix.as_u32())
            }
        }
    )
}

// FROM LEXICAL LOSSY

/// Trait for floating-point types that can be parsed using lossy algorithms from bytes.
pub trait FromLexicalLossy: FromLexical {
    /// Lossy, checked parser for a string-to-number conversion.
    ///
    /// This method parses the entire string, returning an error if
    /// any invalid digits are found during parsing. This parser is
    /// lossy, so numerical rounding may occur during parsing.
    ///
    /// Returns a `Result` containing either the parsed value,
    /// or an error containing any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    fn from_lexical_lossy(bytes: &[u8]) -> Result<Self>;

    /// Lossy, checked parser for a string-to-number conversion.
    ///
    /// This method parses until an invalid digit is found (or the end
    /// of the string), returning the number of processed digits
    /// and the parsed value until that point. This parser is
    /// lossy, so numerical rounding may occur during parsing.
    ///
    /// Returns a `Result` containing either the parsed value
    /// and the number of processed digits, or an error containing
    /// any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    fn from_lexical_partial_lossy(bytes: &[u8]) -> Result<(Self, usize)>;

    /// Lossy, checked parser for a string-to-number conversion.
    ///
    /// This method parses the entire string, returning an error if
    /// any invalid digits are found during parsing. This parser is
    /// lossy, so numerical rounding may occur during parsing.
    ///
    /// Returns a `Result` containing either the parsed value,
    /// or an error containing any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `radix`   - Radix for the number parsing.
    ///
    /// # Panics
    ///
    /// Panics if the radix is not in the range `[2, 36]`.
    #[cfg(feature = "radix")]
    fn from_lexical_lossy_radix(bytes: &[u8], radix: u8) -> Result<Self>;

    /// Lossy, checked parser for a string-to-number conversion.
    ///
    /// This method parses until an invalid digit is found (or the end
    /// of the string), returning the number of processed digits
    /// and the parsed value until that point. This parser is
    /// lossy, so numerical rounding may occur during parsing.
    ///
    /// Returns a `Result` containing either the parsed value
    /// and the number of processed digits, or an error containing
    /// any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `radix`   - Radix for the number parsing.
    ///
    /// # Panics
    ///
    /// Panics if the radix is not in the range `[2, 36]`.
    #[cfg(feature = "radix")]
    fn from_lexical_partial_lossy_radix(bytes: &[u8], radix: u8) -> Result<(Self, usize)>;
}

// Implement FromLexicalLossy for numeric type.
macro_rules! from_lexical_lossy {
    ($cb:expr, $t:ty) => (
        impl FromLexicalLossy for $t {
            #[inline]
            fn from_lexical_lossy(bytes: &[u8]) -> Result<$t>
            {
                to_complete!($cb, bytes, 10)
            }

            #[inline]
            fn from_lexical_partial_lossy(bytes: &[u8]) -> Result<($t, usize)>
            {
                $cb(bytes, 10)
            }

            #[cfg(feature = "radix")]
            #[inline]
            fn from_lexical_lossy_radix(bytes: &[u8], radix: u8) -> Result<$t>
            {
                to_complete!($cb, bytes, radix.as_u32())
            }

            #[cfg(feature = "radix")]
            #[inline]
            fn from_lexical_partial_lossy_radix(bytes: &[u8], radix: u8) -> Result<($t, usize)>
            {
                $cb(bytes, radix.as_u32())
            }
        }
    )
}

// FROM LEXICAL FORMAT

/// Trait for number that can be parsed using a custom format specification.
#[cfg(feature = "format")]
pub trait FromLexicalFormat: FromLexical {
    /// Checked parser for a string-to-number conversion.
    ///
    /// This method parses the entire string, returning an error if
    /// any invalid digits are found during parsing. The numerical format
    /// is specified by the format bitflags, which customize the required
    /// components, digit separators, and other parameters of the number.
    ///
    /// Returns a `Result` containing either the parsed value,
    /// or an error containing any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `format`  - Numerical format.
    fn from_lexical_format(bytes: &[u8], format: NumberFormat) -> Result<Self>;

    /// Checked parser for a string-to-number conversion.
    ///
    /// This method parses until an invalid digit is found (or the end
    /// of the string), returning the number of processed digits
    /// and the parsed value until that point. The numerical format
    /// is specified by the format bitflags, which customize the required
    /// components, digit separators, and other parameters of the number.
    ///
    /// Returns a `Result` containing either the parsed value
    /// and the number of processed digits, or an error containing
    /// any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `format`  - Numerical format.
    fn from_lexical_partial_format(bytes: &[u8], format: NumberFormat) -> Result<(Self, usize)>;

    /// Checked parser for a string-to-number conversion.
    ///
    /// This method parses the entire string, returning an error if
    /// any invalid digits are found during parsing. The numerical format
    /// is specified by the format bitflags, which customize the required
    /// components, digit separators, and other parameters of the number.
    ///
    /// Returns a `Result` containing either the parsed value,
    /// or an error containing any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `radix`   - Radix for the number parsing.
    /// * `format`  - Numerical format.
    ///
    /// # Panics
    ///
    /// Panics if the radix is not in the range `[2, 36]`.
    #[cfg(feature = "radix")]
    fn from_lexical_format_radix(bytes: &[u8], radix: u8, format: NumberFormat) -> Result<Self>;

    /// Checked parser for a string-to-number conversion.
    ///
    /// This method parses until an invalid digit is found (or the end
    /// of the string), returning the number of processed digits
    /// and the parsed value until that point. The numerical format
    /// is specified by the format bitflags, which customize the required
    /// components, digit separators, and other parameters of the number.
    ///
    /// Returns a `Result` containing either the parsed value
    /// and the number of processed digits, or an error containing
    /// any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `radix`   - Radix for the number parsing.
    /// * `format`  - Numerical format.
    ///
    /// # Panics
    ///
    /// Panics if the radix is not in the range `[2, 36]`.
    #[cfg(feature = "radix")]
    fn from_lexical_partial_format_radix(bytes: &[u8], radix: u8, format: NumberFormat) -> Result<(Self, usize)>;
}

// Implement FromLexicalFormat for numeric type.
#[cfg(feature = "format")]
macro_rules! from_lexical_format {
    ($cb:expr, $t:ty) => (
        impl FromLexicalFormat for $t {
            #[inline]
            fn from_lexical_format(bytes: &[u8], format: NumberFormat) -> Result<$t>
            {
                to_complete!($cb, bytes, 10, format)
            }

            #[inline]
            fn from_lexical_partial_format(bytes: &[u8], format: NumberFormat) -> Result<($t, usize)>
            {
                $cb(bytes, 10, format)
            }

            #[cfg(feature = "radix")]
            #[inline]
            fn from_lexical_format_radix(bytes: &[u8], radix: u8, format: NumberFormat) -> Result<$t>
            {
                to_complete!($cb, bytes, radix.as_u32(), format)
            }

            #[cfg(feature = "radix")]
            #[inline]
            fn from_lexical_partial_format_radix(bytes: &[u8], radix: u8, format: NumberFormat) -> Result<($t, usize)>
            {
                $cb(bytes, radix.as_u32(), format)
            }
        }
    )
}

// FROM LEXICAL LOSSY

/// Trait for floating-point types that can be parsed using lossy algorithms with a custom format specification.
#[cfg(feature = "format")]
pub trait FromLexicalLossyFormat: FromLexical {
    /// Lossy, checked parser for a string-to-number conversion.
    ///
    /// This method parses the entire string, returning an error if
    /// any invalid digits are found during parsing. This parser is
    /// lossy, so numerical rounding may occur during parsing. The
    /// numerical format is specified by the format bitflags, which
    /// customize the required components, digit separators, and other
    /// parameters of the number.
    ///
    /// Returns a `Result` containing either the parsed value,
    /// or an error containing any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `format`  - Numerical format.
    fn from_lexical_lossy_format(bytes: &[u8], format: NumberFormat) -> Result<Self>;

    /// Lossy, checked parser for a string-to-number conversion.
    ///
    /// This method parses until an invalid digit is found (or the end
    /// of the string), returning the number of processed digits
    /// and the parsed value until that point. This parser is
    /// lossy, so numerical rounding may occur during parsing. The
    /// numerical format is specified by the format bitflags, which
    /// customize the required components, digit separators, and other
    /// parameters of the number.
    ///
    /// Returns a `Result` containing either the parsed value
    /// and the number of processed digits, or an error containing
    /// any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `format`  - Numerical format.
    fn from_lexical_partial_lossy_format(bytes: &[u8], format: NumberFormat) -> Result<(Self, usize)>;

    /// Lossy, checked parser for a string-to-number conversion.
    ///
    /// This method parses the entire string, returning an error if
    /// any invalid digits are found during parsing. This parser is
    /// lossy, so numerical rounding may occur during parsing. The
    /// numerical format is specified by the format bitflags, which
    /// customize the required components, digit separators, and other
    /// parameters of the number.
    ///
    /// Returns a `Result` containing either the parsed value,
    /// or an error containing any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `radix`   - Radix for the number parsing.
    /// * `format`  - Numerical format.
    ///
    /// # Panics
    ///
    /// Panics if the radix is not in the range `[2, 36]`.
    #[cfg(feature = "radix")]
    fn from_lexical_lossy_format_radix(bytes: &[u8], radix: u8, format: NumberFormat) -> Result<Self>;

    /// Lossy, checked parser for a string-to-number conversion.
    ///
    /// This method parses until an invalid digit is found (or the end
    /// of the string), returning the number of processed digits
    /// and the parsed value until that point. This parser is
    /// lossy, so numerical rounding may occur during parsing. The
    /// numerical format is specified by the format bitflags, which
    /// customize the required components, digit separators, and other
    /// parameters of the number.
    ///
    /// Returns a `Result` containing either the parsed value
    /// and the number of processed digits, or an error containing
    /// any errors that occurred during parsing.
    ///
    /// * `bytes`   - Slice containing a numeric string.
    /// * `radix`   - Radix for the number parsing.
    /// * `format`  - Numerical format.
    ///
    /// # Panics
    ///
    /// Panics if the radix is not in the range `[2, 36]`.
    #[cfg(feature = "radix")]
    fn from_lexical_partial_lossy_format_radix(bytes: &[u8], radix: u8, format: NumberFormat) -> Result<(Self, usize)>;
}

// Implement FromLexicalLossyFormat for numeric type.
#[cfg(feature = "format")]
macro_rules! from_lexical_lossy_format {
    ($cb:expr, $t:ty) => (
        impl FromLexicalLossyFormat for $t {
            #[inline]
            fn from_lexical_lossy_format(bytes: &[u8], format: NumberFormat)
                -> Result<$t>
            {
                to_complete!($cb, bytes, 10, format)
            }

            #[inline]
            fn from_lexical_partial_lossy_format(bytes: &[u8], format: NumberFormat)
                -> Result<($t, usize)>
            {
                $cb(bytes, 10, format)
            }

            #[cfg(feature = "radix")]
            #[inline]
            fn from_lexical_lossy_format_radix(bytes: &[u8], radix: u8, format: NumberFormat)
                -> Result<$t>
            {
                to_complete!($cb, bytes, radix.as_u32(), format)
            }

            #[cfg(feature = "radix")]
            #[inline]
            fn from_lexical_partial_lossy_format_radix(bytes: &[u8], radix: u8, format: NumberFormat)
                -> Result<($t, usize)>
            {
                $cb(bytes, radix.as_u32(), format)
            }
        }
    )
}

// TO LEXICAL

/// Trait for numerical types that can be serialized to bytes.
///
/// To determine the number of bytes required to serialize a value to
/// string, check the associated constants from a required trait:
/// - [`FORMATTED_SIZE`]
/// - [`FORMATTED_SIZE_DECIMAL`]
///
/// [`FORMATTED_SIZE`]: trait.Number.html#associatedconstant.FORMATTED_SIZE
/// [`FORMATTED_SIZE_DECIMAL`]: trait.Number.html#associatedconstant.FORMATTED_SIZE_DECIMAL
pub trait ToLexical: Number {
    /// Serializer for a number-to-string conversion.
    ///
    /// Returns a subslice of the input buffer containing the written bytes,
    /// starting from the same address in memory as the input slice.
    ///
    /// * `value`   - Number to serialize.
    /// * `bytes`   - Slice containing a numeric string.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not of sufficient size. The caller
    /// must provide a slice of sufficient size. In order to ensure
    /// the function will not panic, ensure the buffer has at least
    /// [`FORMATTED_SIZE_DECIMAL`] elements.
    ///
    /// [`FORMATTED_SIZE_DECIMAL`]: trait.Number.html#associatedconstant.FORMATTED_SIZE_DECIMAL
    fn to_lexical<'a>(self, bytes: &'a mut [u8]) -> &'a mut [u8];

     /// Serializer for a number-to-string conversion.
    ///
    /// Returns a subslice of the input buffer containing the written bytes,
    /// starting from the same address in memory as the input slice.
    ///
    /// * `value`   - Number to serialize.
    /// * `radix`   - Radix for number encoding.
    /// * `bytes`   - Slice containing a numeric string.
    ///
    /// # Panics
    ///
    /// Panics if the radix is not in the range `[2, 36]`.
    ///
    /// Also panics if the buffer is not of sufficient size. The caller
    /// must provide a slice of sufficient size. In order to ensure
    /// the function will not panic, ensure the buffer has at least
    /// [`FORMATTED_SIZE`] elements.
    ///
    /// [`FORMATTED_SIZE`]: trait.Number.html#associatedconstant.FORMATTED_SIZE
    #[cfg(feature = "radix")]
    fn to_lexical_radix<'a>(self, radix: u8, bytes: &'a mut [u8]) -> &'a mut [u8];
}

// Implement ToLexical for numeric type.
macro_rules! to_lexical {
    ($cb:expr, $t:ty) => (
        impl ToLexical for $t {
            #[inline]
            fn to_lexical<'a>(self, bytes: &'a mut [u8])
                -> &'a mut [u8]
            {
                assert_buffer!(10, bytes, $t);
                let len = $cb(self, 10, bytes);
                &mut index_mut!(bytes[..len])
            }

            #[cfg(feature = "radix")]
            #[inline]
            fn to_lexical_radix<'a>(self, radix: u8, bytes: &'a mut [u8])
                -> &'a mut [u8]
            {
                assert_radix!(radix);
                assert_buffer!(radix, bytes, $t);
                let len = $cb(self, radix.as_u32(), bytes);
                &mut index_mut!(bytes[..len])
            }
        }
    )
}
