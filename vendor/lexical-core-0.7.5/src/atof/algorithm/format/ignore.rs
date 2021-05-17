//! Ignore float-parsing data interface.

use crate::util::*;
use super::exponent::*;
use super::traits::*;
use super::trim::*;
use super::validate::*;

// Ignore data interface for fast float parsers.
//
// Guaranteed to parse `NumberFormat::ignore(digit_separator)`.
//
// The requirements:
//     1). Must contain significant digits.
data_interface!(
    struct IgnoreFastDataInterface,
    struct IgnoreSlowDataInterface,
    fields => {
        format: NumberFormat,
    },
    integer_iter => (IteratorSeparator, iterate_digits_ignore_separator),
    fraction_iter => (IteratorSeparator, iterate_digits_ignore_separator),
    exponent_iter => (IteratorSeparator, iterate_digits_ignore_separator),
    format => |this: &Self| this.format,
    consume_integer_digits => consume_digits_ignore_separator,
    consume_fraction_digits => consume_digits_ignore_separator,
    extract_exponent => extract_exponent_ignore_separator,
    validate_mantissa => validate_permissive_mantissa,
    validate_exponent => validate_optional_exponent,
    validate_exponent_fraction => validate_exponent_optional_fraction,
    validate_exponent_sign => validate_optional_exponent_sign,
    ltrim_zero => ltrim_zero_separator,
    ltrim_separator => ltrim_separator_separator,
    rtrim_zero => rtrim_zero_separator,
    rtrim_separator => rtrim_separator_separator,
    new => fn new(format: NumberFormat) -> Self {
        Self {
            format,
            integer: &[],
            fraction: None,
            exponent: None,
            raw_exponent: 0
        }
    }
);

// TESTS
// -----

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! ignore {
        ($integer:expr, $fraction:expr, $exponent:expr, $raw_exponent:expr) => {
            IgnoreFastDataInterface {
                format: NumberFormat::ignore(b'_').unwrap(),
                integer: $integer,
                fraction: $fraction,
                exponent: $exponent,
                raw_exponent: $raw_exponent
            }
        };
    }

    #[test]
    fn extract_test() {
        IgnoreFastDataInterface::new(NumberFormat::ignore(b'_').unwrap()).run_tests([
            // Valid
            ("1.2345", Ok(ignore!(b"1", Some(b!("2345")), None, 0))),
            ("12.345", Ok(ignore!(b"12", Some(b!("345")), None, 0))),
            ("12345.6789", Ok(ignore!(b"12345", Some(b!("6789")), None, 0))),
            ("1.2345e10", Ok(ignore!(b"1", Some(b!("2345")), Some(b!("10")), 10))),
            ("1.2345e+10", Ok(ignore!(b"1", Some(b!("2345")), Some(b!("+10")), 10))),
            ("1.2345e-10", Ok(ignore!(b"1", Some(b!("2345")), Some(b!("-10")), -10))),
            ("100000000000000000000", Ok(ignore!(b"100000000000000000000", None, None, 0))),
            ("100000000000000000001", Ok(ignore!(b"100000000000000000001", None, None, 0))),
            ("179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791.9999999999999999999999999999999999999999999999999999999999999999999999", Ok(ignore!(b"179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791", Some(b!("9999999999999999999999999999999999999999999999999999999999999999999999")), None, 0))),
            ("1009e-31", Ok(ignore!(b"1009", None, Some(b!("-31")), -31))),
            ("001.0", Ok(ignore!(b"1", Some(b!("")), None, 0))),
            ("1.", Ok(ignore!(b"1", Some(b!("")), None, 0))),
            ("12.", Ok(ignore!(b"12", Some(b!("")), None, 0))),
            ("1234567.", Ok(ignore!(b"1234567", Some(b!("")), None, 0))),
            (".1", Ok(ignore!(b"", Some(b!("1")), None, 0))),
            (".12", Ok(ignore!(b"", Some(b!("12")), None, 0))),
            (".1234567", Ok(ignore!(b"", Some(b!("1234567")), None, 0))),
            ("1.2345e", Ok(ignore!(b"1", Some(b!("2345")), Some(b!("")), 0))),
            (".3e", Ok(ignore!(b"", Some(b!("3")), Some(b!("")), 0))),
            ("_1.2345_e_+_10", Ok(ignore!(b"1", Some(b!("2345")), Some(b!("_+_10")), 10))),
            ("12__1_._23__45e+1__0_", Ok(ignore!(b"12__1_", Some(b!("_23__45")), Some(b!("+1__0_")), 10))),

            // Invalid
            ("", Err(ErrorCode::EmptyMantissa)),
            ("+", Err(ErrorCode::EmptyMantissa)),
            ("-", Err(ErrorCode::EmptyMantissa)),
            (".", Err(ErrorCode::EmptyMantissa)),
            ("+.", Err(ErrorCode::EmptyMantissa)),
            ("-.", Err(ErrorCode::EmptyMantissa)),
            ("e", Err(ErrorCode::EmptyMantissa)),
            ("E", Err(ErrorCode::EmptyMantissa)),
            ("e1", Err(ErrorCode::EmptyMantissa)),
            ("e+1", Err(ErrorCode::EmptyMantissa)),
            ("e-1", Err(ErrorCode::EmptyMantissa)),
            (".e", Err(ErrorCode::EmptyMantissa)),
            (".E", Err(ErrorCode::EmptyMantissa)),
            (".e1", Err(ErrorCode::EmptyMantissa)),
            (".e+1", Err(ErrorCode::EmptyMantissa)),
            (".e-1", Err(ErrorCode::EmptyMantissa)),
        ].iter());
    }
}
