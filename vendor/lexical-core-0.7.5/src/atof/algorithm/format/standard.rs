//! Standard float-parsing data interface.

use crate::util::*;
use super::exponent::*;
use super::traits::*;
use super::trim::*;
use super::validate::*;

// Standard data interface for fast float parsers.
//
// Guaranteed to parse `NumberFormat::standard()`, and
// therefore will track that exact specification.
//
// The requirements:
//     1). Must contain significant digits.
//     2). Must contain exponent digits if an exponent is present.
//     3). Does not contain any digit separators.
data_interface!(
    struct StandardFastDataInterface,
    struct StandardSlowDataInterface,
    fields => {},
    integer_iter => (IteratorNoSeparator, iterate_digits_no_separator),
    fraction_iter => (IteratorNoSeparator, iterate_digits_no_separator),
    exponent_iter => (IteratorNoSeparator, iterate_digits_no_separator),
    format => |_| NumberFormat::default(),
    consume_integer_digits => consume_digits_no_separator,
    consume_fraction_digits =>  consume_digits_no_separator,
    extract_exponent => extract_exponent_no_separator,
    validate_mantissa => validate_permissive_mantissa,
    validate_exponent => validate_required_exponent,
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

// FROM

type DataTuple<'a> = (&'a [u8], Option<&'a [u8]>, Option<&'a [u8]>, i32);

// Add `From` to remove repition in unit-testing.
impl<'a> From<DataTuple<'a>> for StandardFastDataInterface<'a> {
    perftools_inline!{
    fn from(data: DataTuple<'a>) -> Self {
        StandardFastDataInterface {
            integer: data.0,
            fraction: data.1,
            exponent: data.2,
            raw_exponent: data.3
        }
    }}
}

// TESTS
// -----

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! standard {
        ($integer:expr, $fraction:expr, $exponent:expr, $raw_exponent:expr) => {
            StandardFastDataInterface {
                integer: $integer,
                fraction: $fraction,
                exponent: $exponent,
                raw_exponent: $raw_exponent
            }
        };
    }

    #[test]
    fn extract_test() {
        StandardFastDataInterface::new(NumberFormat::standard().unwrap()).run_tests([
            // Valid
            ("1.2345", Ok(standard!(b"1", Some(b!("2345")), None, 0))),
            ("12.345", Ok(standard!(b"12", Some(b!("345")), None, 0))),
            ("12345.6789", Ok(standard!(b"12345", Some(b!("6789")), None, 0))),
            ("1.2345e10", Ok(standard!(b"1", Some(b!("2345")), Some(b!("10")), 10))),
            ("1.2345e+10", Ok(standard!(b"1", Some(b!("2345")), Some(b!("+10")), 10))),
            ("1.2345e-10", Ok(standard!(b"1", Some(b!("2345")), Some(b!("-10")), -10))),
            ("100000000000000000000", Ok(standard!(b"100000000000000000000", None, None, 0))),
            ("100000000000000000001", Ok(standard!(b"100000000000000000001", None, None, 0))),
            ("179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791.9999999999999999999999999999999999999999999999999999999999999999999999", Ok(standard!(b"179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791", Some(b!("9999999999999999999999999999999999999999999999999999999999999999999999")), None, 0))),
            ("1009e-31", Ok(standard!(b"1009", None, Some(b!("-31")), -31))),
            ("001.0", Ok(standard!(b"1", Some(b!("")), None, 0))),
            ("1.", Ok(standard!(b"1", Some(b!("")), None, 0))),
            ("12.", Ok(standard!(b"12", Some(b!("")), None, 0))),
            ("1234567.", Ok(standard!(b"1234567", Some(b!("")), None, 0))),
            (".1", Ok(standard!(b"", Some(b!("1")), None, 0))),
            (".12", Ok(standard!(b"", Some(b!("12")), None, 0))),
            (".1234567", Ok(standard!(b"", Some(b!("1234567")), None, 0))),

            // Invalid
            ("1.2345e", Err(ErrorCode::EmptyExponent)),
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
            (".3e", Err(ErrorCode::EmptyExponent))
        ].iter());
    }

    #[test]
    fn fast_data_interface_test() {
        type Data<'a> = StandardFastDataInterface<'a>;

        // Check "1.2345".
        let data = Data {
            integer: b"1",
            fraction: Some(b!("2345")),
            exponent: None,
            raw_exponent: 0
        };
        assert!(data.integer_iter().eq(b"1".iter()));
        assert!(data.fraction_iter().eq(b"2345".iter()));

        #[cfg(feature = "correct")]
        assert_eq!(data.digits_start(), 0);
    }

    #[cfg(feature = "correct")]
    #[test]
    fn slow_data_interface_test() {
        type Data<'a> = StandardSlowDataInterface<'a>;
        // Check "1.2345", simple.
        let data = Data {
            integer: b"1",
            fraction: b"2345",
            digits_start: 0,
            truncated_digits: 0,
            raw_exponent: 0
        };
        assert_eq!(data.integer_digits(), 1);
        assert!(data.integer_iter().eq(b"1".iter()));
        assert_eq!(data.fraction_digits(), 4);
        assert!(data.fraction_iter().eq(b"2345".iter()));
        assert_eq!(data.significant_fraction_digits(), 4);
        assert!(data.significant_fraction_iter().eq(b"2345".iter()));
        assert_eq!(data.mantissa_digits(), 5);
        assert_eq!(data.digits_start(), 0);
        assert_eq!(data.truncated_digits(), 0);
        assert_eq!(data.raw_exponent(), 0);
        assert_eq!(data.mantissa_exponent(), -4);
        assert_eq!(data.scientific_exponent(), 0);

        // Check "0.12345", simple.
        let data = Data {
            integer: b"",
            fraction: b"12345",
            digits_start: 0,
            truncated_digits: 0,
            raw_exponent: 0
        };
        assert_eq!(data.integer_digits(), 0);
        assert!(data.integer_iter().eq(b"".iter()));
        assert_eq!(data.fraction_digits(), 5);
        assert!(data.fraction_iter().eq(b"12345".iter()));
        assert_eq!(data.significant_fraction_digits(), 5);
        assert!(data.significant_fraction_iter().eq(b"12345".iter()));
        assert_eq!(data.mantissa_digits(), 5);
        assert_eq!(data.digits_start(), 0);
        assert_eq!(data.truncated_digits(), 0);
        assert_eq!(data.raw_exponent(), 0);
        assert_eq!(data.mantissa_exponent(), -5);
        assert_eq!(data.scientific_exponent(), -1);
    }
}
