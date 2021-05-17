//! Traits that provide format-dependent data for floating parsing algorithms.

use crate::util::*;

#[cfg(feature = "correct")]
use super::exponent::*;

/// Private data interface for local utilities.
pub(crate) trait FastDataInterfaceImpl<'a>: Sized {
    /// Get integer component of float.
    fn integer(&self) -> &'a [u8];

    /// Set integer component of float.
    fn set_integer(&mut self, integer: &'a [u8]);

    /// Get fraction component of float.
    fn fraction(&self) -> Option<&'a [u8]>;

    /// Set fraction component of float.
    fn set_fraction(&mut self, fraction: Option<&'a [u8]>);

    /// Get exponent component of float.
    fn exponent(&self) -> Option<&'a [u8]>;

    /// Set exponent component of float.
    fn set_exponent(&mut self, exponent: Option<&'a [u8]>);

    /// Get raw exponent component of float.
    fn raw_exponent(&self) -> i32;

    /// Set raw exponent component of float.
    fn set_raw_exponent(&mut self, raw_exponent: i32);
}

/// Private data interface for local utilities.
#[cfg(feature = "correct")]
pub(crate) trait SlowDataInterfaceImpl<'a>: Sized {
    /// Get integer component of float.
    fn integer(&self) -> &'a [u8];

    /// Set integer component of float.
    fn set_integer(&mut self, integer: &'a [u8]);

    /// Get fraction component of float.
    fn fraction(&self) -> &'a [u8];

    /// Set fraction component of float.
    fn set_fraction(&mut self, fraction: &'a [u8]);

    /// Get raw exponent component of float.
    fn raw_exponent(&self) -> i32;

    /// Set raw exponent component of float.
    fn set_raw_exponent(&mut self, raw_exponent: i32);
}

// Implement FastDataInterfaceImpl for a default structure.
macro_rules! fast_data_interface_impl {
    ($name:ident) => (
        impl<'a> FastDataInterfaceImpl<'a> for $name<'a> {
            perftools_inline!{
            fn integer(&self) -> &'a [u8] {
                self.integer
            }}

            perftools_inline!{
            fn set_integer(&mut self, integer: &'a [u8]) {
                self.integer = integer
            }}

            perftools_inline!{
            fn fraction(&self) -> Option<&'a [u8]> {
                self.fraction
            }}

            perftools_inline!{
            fn set_fraction(&mut self, fraction: Option<&'a [u8]>) {
                self.fraction = fraction
            }}

            perftools_inline!{
            fn exponent(&self) -> Option<&'a [u8]> {
                self.exponent
            }}

            perftools_inline!{
            fn set_exponent(&mut self, exponent: Option<&'a [u8]>) {
                self.exponent = exponent
            }}

            perftools_inline!{
            fn raw_exponent(&self) -> i32 {
                self.raw_exponent
            }}

            perftools_inline!{
            fn set_raw_exponent(&mut self, raw_exponent: i32) {
                self.raw_exponent = raw_exponent
            }}
        }
    );
}

// Implement SlowDataInterfaceImpl for a default structure.
#[cfg(feature = "correct")]
macro_rules! slow_data_interface_impl {
    ($name:ident) => (
        impl<'a> SlowDataInterfaceImpl<'a> for $name<'a> {
            perftools_inline!{
            fn integer(&self) -> &'a [u8] {
                self.integer
            }}

            perftools_inline!{
            fn set_integer(&mut self, integer: &'a [u8]) {
                self.integer = integer
            }}

            perftools_inline!{
            fn fraction(&self) -> &'a [u8] {
                self.fraction
            }}

            perftools_inline!{
            fn set_fraction(&mut self, fraction: &'a [u8]) {
                self.fraction = fraction
            }}

            perftools_inline!{
            fn raw_exponent(&self) -> i32 {
                self.raw_exponent
            }}

            perftools_inline!{
            fn set_raw_exponent(&mut self, raw_exponent: i32) {
                self.raw_exponent = raw_exponent
            }}
        }
    );
}

// PUBLIC

/// Data interface for fast float parsers.
pub(crate) trait FastDataInterface<'a>: FastDataInterfaceImpl<'a> {
    /// Integer digits iterator type.
    type IntegerIter: ConsumedIterator<Item=&'a u8> + AsPtrIterator<'a, u8>;

    /// Float digits iterator type.
    type FractionIter: ConsumedIterator<Item=&'a u8> + AsPtrIterator<'a, u8>;

    /// Exponent digits iterator type.
    type ExponentIter: ConsumedIterator<Item=&'a u8> + AsPtrIterator<'a, u8>;

    /// Associated slow data type.
    #[cfg(feature = "correct")]
    type SlowInterface: SlowDataInterface<'a>;

    /// Create new float data from format specification.
    fn new(format: NumberFormat) -> Self;

    // DATA

    /// Iterate over all integer digits.
    fn integer_iter(&self) -> Self::IntegerIter;

    /// Iterate over all fraction digits
    fn fraction_iter(&self) -> Self::FractionIter;

    /// Iterate over all exponent digits
    fn exponent_iter(&self) -> Self::ExponentIter;

    /// Get the number format.
    fn format(&self) -> NumberFormat;

    perftools_inline!{
    /// Get the mantissa exponent from the raw exponent.
    #[cfg(feature = "correct")]
    fn mantissa_exponent(&self, truncated_digits: usize) -> i32 {
        mantissa_exponent(self.raw_exponent(), self.fraction_iter().count(), truncated_digits)
    }}

    // EXTRACT

    // Consume integer digits until a non-digit character is found.
    fn consume_integer_digits(&self, bytes: &'a [u8], radix: u32) -> (&'a [u8], &'a [u8]);

    // Consume fraction digits until a non-digit character is found.
    fn consume_fraction_digits(&self, bytes: &'a [u8], radix: u32) -> (&'a [u8], &'a [u8]);

    // Extract the integer substring from the float.
    perftools_inline!{
    fn extract_integer(&mut self, bytes: &'a [u8], radix: u32)
        -> &'a [u8]
    {
        let result = self.consume_integer_digits(bytes, radix);
        self.set_integer(result.0);
        result.1
    }}

    // Extract the fraction substring from the float.
    //
    //  Preconditions:
    //      `bytes.len()` >= 1 and `bytes[0] == b'.'`.
    perftools_inline!{
    fn extract_fraction(&mut self, bytes: &'a [u8], radix: u32)
        -> &'a [u8]
    {
        let digits = &index!(bytes[1..]);
        let result = self.consume_fraction_digits(digits, radix);
        self.set_fraction(Some(result.0));
        result.1
    }}

    // Extract and parse the exponent substring from the float.
    fn extract_exponent(&mut self, bytes: &'a [u8], radix: u32) -> &'a [u8];

    // Validate the extracted mantissa components.
    fn validate_mantissa(&self) -> ParseResult<()>;

    // Validate the extracted exponent component.
    fn validate_exponent(&self) -> ParseResult<()>;

    // Validate the extracted exponent depending on the fraction component.
    fn validate_exponent_fraction(&self) -> ParseResult<()>;

    // Validate the extracted exponent sign.
    fn validate_exponent_sign(&self) -> ParseResult<()>;

    // Trim leading 0s and digit separators.
    fn ltrim_zero(&self, bytes: &'a [u8]) -> (&'a [u8], usize);

    // Trim leading digit separators.
    fn ltrim_separator(&self, bytes: &'a [u8]) -> (&'a [u8], usize);

    // Trim trailing 0s and digit separators.
    fn rtrim_zero(&self, bytes: &'a [u8]) -> (&'a [u8], usize);

    // Trim trailing digit separators.
    fn rtrim_separator(&self, bytes: &'a [u8]) -> (&'a [u8], usize);

    // Post-process float to trim leading and trailing 0s and digit separators.
    // This is required for accurate results in the slow-path algorithm,
    // otherwise, we may incorrect guess the mantissa or scientific exponent.
    perftools_inline!{
    fn trim(&mut self) {
        self.set_integer(self.ltrim_zero(self.integer()).0);
        self.set_fraction(self.fraction().map(|x| self.rtrim_zero(x).0));
    }}

    perftools_inline!{
    /// Extract float subcomponents from input bytes.
    fn extract(&mut self, bytes: &'a [u8], radix: u32) -> ParseResult<*const u8> {
        // Parse the integer, aka, the digits preceding any control characters.
        let mut digits = bytes;
        digits = self.extract_integer(digits, radix);

        // Parse and validate a fraction, if present.
        let exp_char = exponent_notation_char(radix).to_ascii_lowercase();
        if let Some(&b'.') = digits.first() {
            digits = self.extract_fraction(digits, radix);
        }
        self.validate_mantissa()?;

        // Parse and validate an exponent, if present.
        if let Some(&c) = digits.first() {
            if c.to_ascii_lowercase() == exp_char {
                digits = self.extract_exponent(digits, radix);
            }
        }
        self.validate_exponent()?;
        self.validate_exponent_fraction()?;
        self.validate_exponent_sign()?;

        // Trim the remaining digits.
        self.trim();

        Ok(digits.as_ptr())
    }}

    // TO SLOW DATA

    // Calculate the digit start from the integer and fraction slices.
    perftools_inline!{
    #[cfg(feature = "correct")]
    fn digits_start(&self) -> usize {
        // If there are no returned values in the integer iterator
        // since we've trimmed leading 0s, then we have to trim
        // leading zeros to get to the start of the significant
        // digits in the fraction.
        match self.integer().is_empty() {
            true  => self.ltrim_zero(self.fraction().unwrap_or(&[])).1,
            false => 0,
        }
    }}

    /// Process float data for moderate/slow float parsers.
    #[cfg(feature = "correct")]
    fn to_slow(self, truncated_digits: usize) -> Self::SlowInterface;

    // TESTS

    #[cfg(test)]
    fn clear(&mut self) {
        self.set_integer(&[]);
        self.set_fraction(None);
        self.set_exponent(None);
        self.set_raw_exponent(0);
    }

    /// Check the float state parses the desired data.
    #[cfg(test)]
    fn check_extract(&mut self, digits: &'a [u8], expected: &ParseTestResult<Self>) {
        let expected = expected.as_ref();
        match self.extract(digits, 10) {
            Ok(_)       => {
                let expected = expected.unwrap();
                assert_eq!(self.integer(), expected.integer());
                assert_eq!(self.fraction(), expected.fraction());
                assert_eq!(self.exponent(), expected.exponent());
            },
            Err((c, _))  => assert_eq!(c, *expected.err().unwrap()),
        }
    }

    // Run series of tests.
    #[cfg(test)]
    fn run_tests<Iter>(&mut self, tests: Iter)
        where Iter: Iterator<Item=&'a (&'a str, ParseTestResult<Self>)>,
              Self: 'a
    {
        for value in tests {
            self.check_extract(value.0.as_bytes(), &value.1);
            self.clear();
        }
    }
}

/// Shared definition for all fast data interfaces.
macro_rules! fast_data_interface {
    (
        struct $name:ident,
        fields => { $( $field:ident : $type:tt, )* },
        integer_iter => ( $integer_iter:tt, $integer_iter_fn:ident ),
        fraction_iter => ( $fraction_iter:tt, $fraction_iter_fn:ident ),
        exponent_iter => ( $exponent_iter:tt, $exponent_iter_fn:ident ),
        format => $format:expr,
        slow_interface => $slow_interface:tt,
        consume_integer_digits => $consume_integer_digits:expr,
        consume_fraction_digits => $consume_fraction_digits:expr,
        extract_exponent => $extract_exponent:expr,
        validate_mantissa => $validate_mantissa:expr,
        validate_exponent => $validate_exponent:expr,
        validate_exponent_fraction => $validate_exponent_fraction:expr,
        validate_exponent_sign => $validate_exponent_sign:expr,
        ltrim_zero => $ltrim_zero:ident,
        ltrim_separator => $ltrim_separator:ident,
        rtrim_zero => $rtrim_zero:ident,
        rtrim_separator => $rtrim_separator:ident,
        new => $($new:tt)*
    ) => (
        pub(crate) struct $name<'a> {
            $( $field : $type, )*
            integer: &'a [u8],
            fraction: Option<&'a [u8]>,
            exponent: Option<&'a [u8]>,
            raw_exponent: i32
        }

        fast_data_interface_impl!($name);

        impl<'a> FastDataInterface<'a> for $name<'a> {
            type IntegerIter = $integer_iter<'a>;
            type FractionIter = $fraction_iter<'a>;
            type ExponentIter = $exponent_iter<'a>;

            #[cfg(feature = "correct")]
            type SlowInterface = $slow_interface<'a>;

            perftools_inline!{
            #[allow(unused_variables)]
            $($new)*
            }

            // DATA

            perftools_inline!{
            fn integer_iter(&self) -> Self::IntegerIter {
                $integer_iter_fn(self.integer, self.format().digit_separator())
            }}

            perftools_inline!{
            fn fraction_iter(&self) -> Self::FractionIter {
                let fraction = self.fraction.unwrap_or(&[]);
                $fraction_iter_fn(fraction, self.format().digit_separator())
            }}

            perftools_inline!{
            fn exponent_iter(&self) -> Self::ExponentIter {
                let exponent = self.exponent.unwrap_or(&[]);
                $exponent_iter_fn(exponent, self.format().digit_separator())
            }}

            perftools_inline!{
            fn format(&self) -> NumberFormat {
                $format(self)
            }}

            perftools_inline!{
            fn consume_integer_digits(&self, digits: &'a [u8], radix: u32)
                -> (&'a [u8], &'a [u8])
            {
                $consume_integer_digits(digits, radix, self.format())
            }}

            perftools_inline!{
            fn consume_fraction_digits(&self, digits: &'a [u8], radix: u32)
                -> (&'a [u8], &'a [u8])
            {
                $consume_fraction_digits(digits, radix, self.format())
            }}

            perftools_inline!{
            fn extract_exponent(&mut self, bytes: &'a [u8], radix: u32) -> &'a [u8]
            {
                $extract_exponent(self, bytes, radix, self.format())
            }}

            perftools_inline!{
            fn validate_mantissa(&self) -> ParseResult<()> {
                $validate_mantissa(self)
            }}

            perftools_inline!{
            fn validate_exponent(&self) -> ParseResult<()> {
                $validate_exponent(self)
            }}

            perftools_inline!{
            fn validate_exponent_fraction(&self) -> ParseResult<()> {
                $validate_exponent_fraction(self)
            }}

            perftools_inline!{
            fn validate_exponent_sign(&self) -> ParseResult<()> {
                $validate_exponent_sign(self)
            }}

            perftools_inline!{
            fn ltrim_zero(&self, bytes: &'a [u8]) -> (&'a [u8], usize) {
                $ltrim_zero(bytes, self.format().digit_separator())
            }}

            perftools_inline!{
            fn ltrim_separator(&self, bytes: &'a [u8]) -> (&'a [u8], usize) {
                $ltrim_separator(bytes, self.format().digit_separator())
            }}

            perftools_inline!{
            fn rtrim_zero(&self, bytes: &'a [u8]) -> (&'a [u8], usize) {
                $rtrim_zero(bytes, self.format().digit_separator())
            }}

            perftools_inline!{
            fn rtrim_separator(&self, bytes: &'a [u8]) -> (&'a [u8], usize) {
                $rtrim_separator(bytes, self.format().digit_separator())
            }}

            // TO SLOW DATA

            #[cfg(feature = "correct")]
            perftools_inline!{
            fn to_slow(self, truncated_digits: usize) -> Self::SlowInterface {
                let digits_start = self.digits_start();
                Self::SlowInterface {
                    $( $field: self.$field, )*
                    digits_start,
                    truncated_digits,
                    integer: self.integer,
                    fraction: self.fraction.unwrap_or(&[]),
                    raw_exponent: self.raw_exponent
                }
            }}
        }
    );
}

/// Data interface for moderate/slow float parsers.
#[cfg(feature = "correct")]
pub(crate) trait SlowDataInterface<'a>: SlowDataInterfaceImpl<'a> {
    /// Integer digits iterator type.
    type IntegerIter: ConsumedIterator<Item=&'a u8> + AsPtrIterator<'a, u8>;

    /// Float digits iterator type.
    type FractionIter: ConsumedIterator<Item=&'a u8> + AsPtrIterator<'a, u8>;

    /// Iterate over all integer digits.
    fn integer_iter(&self) -> Self::IntegerIter;

    perftools_inline!{
    /// Get number of all integer digits.
    fn integer_digits(&self) -> usize {
        self.integer_iter().count()
    }}

    /// Iterate over all fraction digits
    fn fraction_iter(&self) -> Self::FractionIter;

    perftools_inline!{
    /// Get number of all fraction digits.
    fn fraction_digits(&self) -> usize {
        self.fraction_iter().count()
    }}

    /// Iterate over significant fraction digits.
    fn significant_fraction_iter(&self) -> Self::FractionIter;

    perftools_inline!{
    /// Get number of significant fraction digits.
    fn significant_fraction_digits(&self) -> usize {
        self.significant_fraction_iter().count()
    }}

    perftools_inline!{
    /// Get the number of digits in the mantissa.
    /// Cannot overflow, since this is based off a single usize input string.
    fn mantissa_digits(&self) -> usize {
        self.integer_digits() + self.significant_fraction_digits()
    }}

    /// Get the number format.
    fn format(&self) -> NumberFormat;

    /// Get index to start of significant digits in the fraction.
    fn digits_start(&self) -> usize;

    /// Get number of truncated digits.
    fn truncated_digits(&self) -> usize;

    perftools_inline!{
    /// Get the mantissa exponent from the raw exponent.
    fn mantissa_exponent(&self) -> i32 {
        mantissa_exponent(self.raw_exponent(), self.fraction_digits(), self.truncated_digits())
    }}

    perftools_inline!{
    /// Get the scientific exponent from the raw exponent.
    fn scientific_exponent(&self) -> i32 {
        scientific_exponent(self.raw_exponent(), self.integer_digits(), self.digits_start())
    }}
}

/// Shared definition for all slow data interfaces.
macro_rules! slow_data_interface {
    (
        struct $name:ident,
        fields => { $( $field:ident : $type:tt, )* },
        integer_iter => ( $integer_iter:tt, $integer_iter_fn:ident ),
        fraction_iter => ( $fraction_iter:tt, $fraction_iter_fn:ident ),
        format => $format:expr
    ) => (
        #[cfg(feature = "correct")]
        pub(crate) struct $name<'a> {
            $( $field : $type, )*
            integer: &'a [u8],
            fraction: &'a [u8],
            digits_start: usize,
            truncated_digits: usize,
            raw_exponent: i32
        }

        #[cfg(feature = "correct")]
        slow_data_interface_impl!($name);

        #[cfg(feature = "correct")]
        impl<'a> SlowDataInterface<'a> for $name<'a> {
            type IntegerIter = $integer_iter<'a>;
            type FractionIter = $fraction_iter<'a>;

            // DATA

            perftools_inline!{
            fn integer_iter(&self) -> Self::IntegerIter {
                $integer_iter_fn(self.integer, self.format().digit_separator())
            }}

            perftools_inline!{
            fn fraction_iter(&self) -> Self::FractionIter {
                $fraction_iter_fn(self.fraction, self.format().digit_separator())
            }}

            perftools_inline!{
            fn significant_fraction_iter(&self) -> Self::FractionIter {
                let fraction = &index!(self.fraction[self.digits_start..]);
                $fraction_iter_fn(fraction, self.format().digit_separator())
            }}

            perftools_inline!{
            fn format(&self) -> NumberFormat {
                $format(self)
            }}

            perftools_inline!{
            fn digits_start(&self) -> usize {
                self.digits_start
            }}

            perftools_inline!{
            fn truncated_digits(&self) -> usize {
                self.truncated_digits
            }}
        }
    );
}

/// Shared definition for all data interfaces.
macro_rules! data_interface {
    (
        struct $fast:ident,
        struct $slow:ident,
        fields => { $( $field:ident : $type:tt, )* },
        integer_iter => ( $integer_iter:tt, $integer_iter_fn:ident ),
        fraction_iter => ( $fraction_iter:tt, $fraction_iter_fn:ident ),
        exponent_iter => ( $exponent_iter:tt, $exponent_iter_fn:ident ),
        format => $format:expr,
        consume_integer_digits => $consume_integer_digits:expr,
        consume_fraction_digits => $consume_fraction_digits:expr,
        extract_exponent => $extract_exponent:expr,
        validate_mantissa => $validate_mantissa:expr,
        validate_exponent => $validate_exponent:expr,
        validate_exponent_fraction => $validate_exponent_fraction:expr,
        validate_exponent_sign => $validate_exponent_sign:expr,
        ltrim_zero => $ltrim_zero:ident,
        ltrim_separator => $ltrim_separator:ident,
        rtrim_zero => $rtrim_zero:ident,
        rtrim_separator => $rtrim_separator:ident,
        new => $($new:tt)*
    ) => (
        fast_data_interface!(
            struct $fast,
            fields => { $( $field : $type , )* },
            integer_iter => ($integer_iter, $integer_iter_fn),
            fraction_iter => ($fraction_iter, $fraction_iter_fn),
            exponent_iter => ($exponent_iter, $exponent_iter_fn),
            format => $format,
            slow_interface => $slow,
            consume_integer_digits => $consume_integer_digits,
            consume_fraction_digits => $consume_fraction_digits,
            extract_exponent => $extract_exponent,
            validate_mantissa => $validate_mantissa,
            validate_exponent => $validate_exponent,
            validate_exponent_fraction => $validate_exponent_fraction,
            validate_exponent_sign => $validate_exponent_sign,
            ltrim_zero => $ltrim_zero,
            ltrim_separator => $ltrim_separator,
            rtrim_zero => $rtrim_zero,
            rtrim_separator => $rtrim_separator,
            new => $($new)*
        );

        slow_data_interface!(
            struct $slow,
            fields => { $( $field : $type , )* },
            integer_iter => ($integer_iter, $integer_iter_fn),
            fraction_iter => ($fraction_iter, $fraction_iter_fn),
            format => $format
        );
    );
}
