//! Permissive float-parsing data interface.

use crate::util::*;
use super::exponent::*;
use super::traits::*;
use super::trim::*;
use super::validate::*;

// Permissive data interface for fast float parsers.
//
// Guaranteed to parse `NumberFormat::permissive()`.
//
// The requirements:
//     1). Must contain significant digits.
//     2). Does not contain any digit separators.
data_interface!(
    struct PermissiveFastDataInterface,
    struct PermissiveSlowDataInterface,
    fields => {},
    integer_iter => (IteratorNoSeparator, iterate_digits_no_separator),
    fraction_iter => (IteratorNoSeparator, iterate_digits_no_separator),
    exponent_iter => (IteratorNoSeparator, iterate_digits_no_separator),
    format => |_| NumberFormat::default(),
    consume_integer_digits => consume_digits_no_separator,
    consume_fraction_digits =>  consume_digits_no_separator,
    extract_exponent => extract_exponent_no_separator,
    validate_mantissa => validate_permissive_mantissa,
    validate_exponent => validate_optional_exponent,
    validate_exponent_fraction => validate_exponent_optional_fraction,
    validate_exponent_sign => validate_optional_exponent_sign,
    ltrim_zero => ltrim_zero_no_separator,
    ltrim_separator => ltrim_separator_no_separator,
    rtrim_zero => rtrim_zero_no_separator,
    rtrim_separator => rtrim_separator_no_separator,
    new => fn new(format: NumberFormat) -> Self {
        Self {
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

    macro_rules! permissive {
        ($integer:expr, $fraction:expr, $exponent:expr, $raw_exponent:expr) => {
            PermissiveFastDataInterface {
                integer: $integer,
                fraction: $fraction,
                exponent: $exponent,
                raw_exponent: $raw_exponent
            }
        };
    }

    #[test]
    fn extract_test() {
        PermissiveFastDataInterface::new(NumberFormat::permissive().unwrap()).run_tests([
            // Valid
            ("1.2345", Ok(permissive!(b"1", Some(b!("2345")), None, 0))),
            ("12.345", Ok(permissive!(b"12", Some(b!("345")), None, 0))),
            ("12345.6789", Ok(permissive!(b"12345", Some(b!("6789")), None, 0))),
            ("1.2345e10", Ok(permissive!(b"1", Some(b!("2345")), Some(b!("10")), 10))),
            ("1.2345e+10", Ok(permissive!(b"1", Some(b!("2345")), Some(b!("+10")), 10))),
            ("1.2345e-10", Ok(permissive!(b"1", Some(b!("2345")), Some(b!("-10")), -10))),
            ("100000000000000000000", Ok(permissive!(b"100000000000000000000", None, None, 0))),
            ("100000000000000000001", Ok(permissive!(b"100000000000000000001", None, None, 0))),
            ("179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791.9999999999999999999999999999999999999999999999999999999999999999999999", Ok(permissive!(b"179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791", Some(b!("9999999999999999999999999999999999999999999999999999999999999999999999")), None, 0))),
            ("1009e-31", Ok(permissive!(b"1009", None, Some(b!("-31")), -31))),
            ("001.0", Ok(permissive!(b"1", Some(b!("")), None, 0))),
            ("1.", Ok(permissive!(b"1", Some(b!("")), None, 0))),
            ("12.", Ok(permissive!(b"12", Some(b!("")), None, 0))),
            ("1234567.", Ok(permissive!(b"1234567", Some(b!("")), None, 0))),
            (".1", Ok(permissive!(b"", Some(b!("1")), None, 0))),
            (".12", Ok(permissive!(b"", Some(b!("12")), None, 0))),
            (".1234567", Ok(permissive!(b"", Some(b!("1234567")), None, 0))),
            ("1.2345e", Ok(permissive!(b"1", Some(b!("2345")), Some(b!("")), 0))),
            (".3e", Ok(permissive!(b"", Some(b!("3")), Some(b!("")), 0))),

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
