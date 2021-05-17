#![deny(unsafe_code)]
#![warn(missing_copy_implementations)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]
#![warn(nonstandard_style)]
#![warn(trivial_numeric_casts)]
#![warn(unreachable_pub)]
#![warn(unused)]


//! This is a library for formatting numbers with numeric prefixes, such as
//! turning “3000 metres” into “3 kilometres”, or “8705 bytes” into “8.5 KiB”.
//!
//!
//! # Usage
//!
//! The function [`NumberPrefix::decimal`](enum.NumberPrefix.html#method.decimal)
//! returns either a pair of the resulting number and its prefix, or a
//! notice that the number was too small to have any prefix applied to it. For
//! example:
//!
//! ```
//! use number_prefix::NumberPrefix;
//!
//! let amount = 8542_f32;
//! let result = match NumberPrefix::decimal(amount) {
//!     NumberPrefix::Standalone(bytes) => {
//!         format!("The file is {} bytes in size", bytes)
//!     }
//!     NumberPrefix::Prefixed(prefix, n) => {
//!         format!("The file is {:.1} {}B in size", n, prefix)
//!     }
//! };
//!
//! assert_eq!("The file is 8.5 kB in size", result);
//! ```
//!
//! The `{:.1}` part of the formatting string tells it to restrict the
//! output to only one decimal place. This value is calculated by repeatedly
//! dividing the number by 1000 until it becomes less than that, which in this
//! case results in 8.542, which gets rounded down. Because only one division
//! had to take place, the function also returns the decimal prefix `Kilo`,
//! which gets converted to its internationally-recognised symbol when
//! formatted as a string.
//!
//! If the value is too small to have any prefixes applied to it — in this case,
//! if it’s under 1000 — then the standalone value will be returned:
//!
//! ```
//! use number_prefix::NumberPrefix;
//!
//! let amount = 705_f32;
//! let result = match NumberPrefix::decimal(amount) {
//!     NumberPrefix::Standalone(bytes) => {
//!         format!("The file is {} bytes in size", bytes)
//!     }
//!     NumberPrefix::Prefixed(prefix, n) => {
//!         format!("The file is {:.1} {}B in size", n, prefix)
//!     }
//! };
//!
//! assert_eq!("The file is 705 bytes in size", result);
//! ```
//!
//! In this particular example, the user expects different formatting for
//! both bytes and kilobytes: while prefixed values are given more precision,
//! there’s no point using anything other than whole numbers for just byte
//! amounts. This is why the function pays attention to values without any
//! prefixes — they often need to be special-cased.
//!
//!
//! ## Binary Prefixes
//!
//! This library also allows you to use the *binary prefixes*, which use the
//! number 1024 (2<sup>10</sup>) as the multiplier, rather than the more common 1000
//! (10<sup>3</sup>). This uses the
//! [`NumberPrefix::binary`](enum.NumberPrefix.html#method.binary) function.
//! For example:
//!
//! ```
//! use number_prefix::NumberPrefix;
//!
//! let amount = 8542_f32;
//! let result = match NumberPrefix::binary(amount) {
//!     NumberPrefix::Standalone(bytes) => {
//!         format!("The file is {} bytes in size", bytes)
//!     }
//!     NumberPrefix::Prefixed(prefix, n) => {
//!         format!("The file is {:.1} {}B in size", n, prefix)
//!     }
//! };
//!
//! assert_eq!("The file is 8.3 KiB in size", result);
//! ```
//!
//! A kibibyte is slightly larger than a kilobyte, so the number is smaller
//! in the result; but other than that, it works in exactly the same way, with
//! the binary prefix being converted to a symbol automatically.
//!
//!
//! ## Which type of prefix should I use?
//!
//! There is no correct answer this question! Common practice is to use
//! the binary prefixes for numbers of *bytes*, while still using the decimal
//! prefixes for everything else. Computers work with powers of two, rather than
//! powers of ten, and by using the binary prefixes, you get a more accurate
//! representation of the amount of data.
//!
//!
//! ## Prefix Names
//!
//! If you need to describe your unit in actual words, rather than just with the
//! symbol, use one of the `upper`, `caps`, `lower`, or `symbol`, which output the
//! prefix in a variety of formats. For example:
//!
//! ```
//! use number_prefix::NumberPrefix;
//!
//! let amount = 8542_f32;
//! let result = match NumberPrefix::decimal(amount) {
//!     NumberPrefix::Standalone(bytes) => {
//!         format!("The file is {} bytes in size", bytes)
//!     }
//!     NumberPrefix::Prefixed(prefix, n) => {
//!         format!("The file is {:.1} {}bytes in size", n, prefix.lower())
//!     }
//! };
//!
//! assert_eq!("The file is 8.5 kilobytes in size", result);
//! ```
//!
//!
//! ## String Parsing
//!
//! There is a `FromStr` implementation for `NumberPrefix` that parses
//! strings containing numbers and trailing prefixes, such as `7.5E`.
//!
//! Currently, the only supported units are `b` and `B` for bytes, and `m` for
//! metres. Whitespace is allowed between the number and the rest of the string.
//!
//! ```
//! use number_prefix::{NumberPrefix, Prefix};
//!
//! assert_eq!("7.05E".parse::<NumberPrefix<_>>(),
//!            Ok(NumberPrefix::Prefixed(Prefix::Exa, 7.05_f64)));
//!
//! assert_eq!("7.05".parse::<NumberPrefix<_>>(),
//!            Ok(NumberPrefix::Standalone(7.05_f64)));
//!
//! assert_eq!("7.05 GiB".parse::<NumberPrefix<_>>(),
//!            Ok(NumberPrefix::Prefixed(Prefix::Gibi, 7.05_f64)));
//! ```


#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
mod parse;

#[cfg(not(feature = "std"))]
use core::ops::{Neg, Div};

#[cfg(feature = "std")]
use std::{fmt, ops::{Neg, Div}};


/// A numeric prefix, either binary or decimal.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Prefix {

    /// _kilo_, 10<sup>3</sup> or 1000<sup>1</sup>.
    /// From the Greek ‘χίλιοι’ (‘chilioi’), meaning ‘thousand’.
    Kilo,

    /// _mega_, 10<sup>6</sup> or 1000<sup>2</sup>.
    /// From the Ancient Greek ‘μέγας’ (‘megas’), meaning ‘great’.
    Mega,

    /// _giga_, 10<sup>9</sup> or 1000<sup>3</sup>.
    /// From the Greek ‘γίγας’ (‘gigas’), meaning ‘giant’.
    Giga,

    /// _tera_, 10<sup>12</sup> or 1000<sup>4</sup>.
    /// From the Greek ‘τέρας’ (‘teras’), meaning ‘monster’.
    Tera,

    /// _peta_, 10<sup>15</sup> or 1000<sup>5</sup>.
    /// From the Greek ‘πέντε’ (‘pente’), meaning ‘five’.
    Peta,

    /// _exa_, 10<sup>18</sup> or 1000<sup>6</sup>.
    /// From the Greek ‘ἕξ’ (‘hex’), meaning ‘six’.
    Exa,

    /// _zetta_, 10<sup>21</sup> or 1000<sup>7</sup>.
    /// From the Latin ‘septem’, meaning ‘seven’.
    Zetta,

    /// _yotta_, 10<sup>24</sup> or 1000<sup>8</sup>.
    /// From the Green ‘οκτώ’ (‘okto’), meaning ‘eight’.
    Yotta,

    /// _kibi_, 2<sup>10</sup> or 1024<sup>1</sup>.
    /// The binary version of _kilo_.
    Kibi,

    /// _mebi_, 2<sup>20</sup> or 1024<sup>2</sup>.
    /// The binary version of _mega_.
    Mebi,

    /// _gibi_, 2<sup>30</sup> or 1024<sup>3</sup>.
    /// The binary version of _giga_.
    Gibi,

    /// _tebi_, 2<sup>40</sup> or 1024<sup>4</sup>.
    /// The binary version of _tera_.
    Tebi,

    /// _pebi_, 2<sup>50</sup> or 1024<sup>5</sup>.
    /// The binary version of _peta_.
    Pebi,

    /// _exbi_, 2<sup>60</sup> or 1024<sup>6</sup>.
    /// The binary version of _exa_.
    Exbi,
    // you can download exa binaries at https://exa.website/#installation

    /// _zebi_, 2<sup>70</sup> or 1024<sup>7</sup>.
    /// The binary version of _zetta_.
    Zebi,

    /// _yobi_, 2<sup>80</sup> or 1024<sup>8</sup>.
    /// The binary version of _yotta_.
    Yobi,
}


/// The result of trying to apply a prefix to a floating-point value.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum NumberPrefix<F> {

	/// A **standalone** value is returned when the number is too small to
	/// have any prefixes applied to it. This is commonly a special case, so
	/// is handled separately.
    Standalone(F),

    /// A **prefixed** value *is* large enough for prefixes. This holds the
    /// prefix, as well as the resulting value.
    Prefixed(Prefix, F),
}

impl<F: Amounts> NumberPrefix<F> {

    /// Formats the given floating-point number using **decimal** prefixes.
    ///
    /// This function accepts both `f32` and `f64` values. If you’re trying to
    /// format an integer, you’ll have to cast it first.
    ///
    /// # Examples
    ///
    /// ```
    /// use number_prefix::{Prefix, NumberPrefix};
    ///
    /// assert_eq!(NumberPrefix::decimal(1_000_000_000_f32),
    ///            NumberPrefix::Prefixed(Prefix::Giga, 1_f32));
    /// ```
    pub fn decimal(amount: F) -> Self {
        use self::Prefix::*;
        Self::format_number(amount, Amounts::NUM_1000, [Kilo, Mega, Giga, Tera, Peta, Exa, Zetta, Yotta])
    }

    /// Formats the given floating-point number using **binary** prefixes.
    ///
    /// This function accepts both `f32` and `f64` values. If you’re trying to
    /// format an integer, you’ll have to cast it first.
    ///
    /// # Examples
    ///
    /// ```
    /// use number_prefix::{Prefix, NumberPrefix};
    ///
    /// assert_eq!(NumberPrefix::binary(1_073_741_824_f64),
    ///            NumberPrefix::Prefixed(Prefix::Gibi, 1_f64));
    /// ```
    pub fn binary(amount: F) -> Self {
        use self::Prefix::*;
        Self::format_number(amount, Amounts::NUM_1024, [Kibi, Mebi, Gibi, Tebi, Pebi, Exbi, Zebi, Yobi])
    }

    fn format_number(mut amount: F, kilo: F, prefixes: [Prefix; 8]) -> Self {

        // For negative numbers, flip it to positive, do the processing, then
        // flip it back to negative again afterwards.
        let was_negative = if amount.is_negative() { amount = -amount; true } else { false };

        let mut prefix = 0;
        while amount >= kilo && prefix < 8 {
            amount = amount / kilo;
            prefix += 1;
        }

        if was_negative {
            amount = -amount;
        }

        if prefix == 0 {
            NumberPrefix::Standalone(amount)
        }
        else {
            NumberPrefix::Prefixed(prefixes[prefix - 1], amount)
        }
    }
}

#[cfg(feature = "std")]
impl fmt::Display for Prefix {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "{}", self.symbol())
	}
}

impl Prefix {

	/// Returns the name in uppercase, such as “KILO”.
	///
	/// # Examples
	///
	/// ```
	/// use number_prefix::Prefix;
	///
	/// assert_eq!("GIGA", Prefix::Giga.upper());
	/// assert_eq!("GIBI", Prefix::Gibi.upper());
	/// ```
    pub fn upper(self) -> &'static str {
        use self::Prefix::*;
        match self {
            Kilo => "KILO",  Mega => "MEGA",  Giga  => "GIGA",   Tera  => "TERA",
            Peta => "PETA",  Exa  => "EXA",   Zetta => "ZETTA",  Yotta => "YOTTA",
            Kibi => "KIBI",  Mebi => "MEBI",  Gibi  => "GIBI",   Tebi  => "TEBI",
            Pebi => "PEBI",  Exbi => "EXBI",  Zebi  => "ZEBI",   Yobi  => "YOBI",
        }
    }

    /// Returns the name with the first letter capitalised, such as “Mega”.
    ///
	/// # Examples
	///
	/// ```
	/// use number_prefix::Prefix;
	///
	/// assert_eq!("Giga", Prefix::Giga.caps());
	/// assert_eq!("Gibi", Prefix::Gibi.caps());
	/// ```
    pub fn caps(self) -> &'static str {
        use self::Prefix::*;
        match self {
            Kilo => "Kilo",  Mega => "Mega",  Giga  => "Giga",   Tera  => "Tera",
            Peta => "Peta",  Exa  => "Exa",   Zetta => "Zetta",  Yotta => "Yotta",
            Kibi => "Kibi",  Mebi => "Mebi",  Gibi  => "Gibi",   Tebi  => "Tebi",
            Pebi => "Pebi",  Exbi => "Exbi",  Zebi  => "Zebi",   Yobi  => "Yobi",
        }
    }

    /// Returns the name in lowercase, such as “giga”.
    ///
    /// # Examples
    ///
    /// ```
    /// use number_prefix::Prefix;
    ///
    /// assert_eq!("giga", Prefix::Giga.lower());
    /// assert_eq!("gibi", Prefix::Gibi.lower());
    /// ```
    pub fn lower(self) -> &'static str {
        use self::Prefix::*;
        match self {
            Kilo => "kilo",  Mega => "mega",  Giga  => "giga",   Tera  => "tera",
            Peta => "peta",  Exa  => "exa",   Zetta => "zetta",  Yotta => "yotta",
            Kibi => "kibi",  Mebi => "mebi",  Gibi  => "gibi",   Tebi  => "tebi",
            Pebi => "pebi",  Exbi => "exbi",  Zebi  => "zebi",   Yobi  => "yobi",
        }
    }

    /// Returns the short-hand symbol, such as “T” (for “tera”).
    ///
	/// # Examples
	///
	/// ```
	/// use number_prefix::Prefix;
	///
	/// assert_eq!("G",  Prefix::Giga.symbol());
	/// assert_eq!("Gi", Prefix::Gibi.symbol());
	/// ```
    pub fn symbol(self) -> &'static str {
        use self::Prefix::*;
        match self {
            Kilo => "k",   Mega => "M",   Giga  => "G",   Tera  => "T",
            Peta => "P",   Exa  => "E",   Zetta => "Z",   Yotta => "Y",
            Kibi => "Ki",  Mebi => "Mi",  Gibi  => "Gi",  Tebi  => "Ti",
            Pebi => "Pi",  Exbi => "Ei",  Zebi  => "Zi",  Yobi  => "Yi",
        }
    }
}

/// Traits for floating-point values for both the possible multipliers. They
/// need to be Copy, have defined 1000 and 1024s, and implement a bunch of
/// operators.
pub trait Amounts: Copy + Sized + PartialOrd + Div<Output=Self> + Neg<Output=Self> {

    /// The constant representing 1000, for decimal prefixes.
    const NUM_1000: Self;

    /// The constant representing 1024, for binary prefixes.
    const NUM_1024: Self;

    /// Whether this number is negative.
    /// This is used internally.
    fn is_negative(self) -> bool;
}

impl Amounts for f32 {
    const NUM_1000: Self = 1000_f32;
    const NUM_1024: Self = 1024_f32;

    fn is_negative(self) -> bool {
        self.is_sign_negative()
    }
}

impl Amounts for f64 {
    const NUM_1000: Self = 1000_f64;
    const NUM_1024: Self = 1024_f64;

    fn is_negative(self) -> bool {
        self.is_sign_negative()
    }
}


#[cfg(test)]
mod test {
    use super::{NumberPrefix, Prefix};

	#[test]
	fn decimal_minus_one_billion() {
	    assert_eq!(NumberPrefix::decimal(-1_000_000_000_f64),
	               NumberPrefix::Prefixed(Prefix::Giga, -1f64))
	}

    #[test]
    fn decimal_minus_one() {
        assert_eq!(NumberPrefix::decimal(-1f64),
                   NumberPrefix::Standalone(-1f64))
    }

    #[test]
    fn decimal_0() {
        assert_eq!(NumberPrefix::decimal(0f64),
                   NumberPrefix::Standalone(0f64))
    }

    #[test]
    fn decimal_999() {
        assert_eq!(NumberPrefix::decimal(999f32),
                   NumberPrefix::Standalone(999f32))
    }

    #[test]
    fn decimal_1000() {
        assert_eq!(NumberPrefix::decimal(1000f32),
                   NumberPrefix::Prefixed(Prefix::Kilo, 1f32))
    }

    #[test]
    fn decimal_1030() {
        assert_eq!(NumberPrefix::decimal(1030f32),
                   NumberPrefix::Prefixed(Prefix::Kilo, 1.03f32))
    }

    #[test]
    fn decimal_1100() {
        assert_eq!(NumberPrefix::decimal(1100f64),
                   NumberPrefix::Prefixed(Prefix::Kilo, 1.1f64))
    }

    #[test]
    fn decimal_1111() {
        assert_eq!(NumberPrefix::decimal(1111f64),
                   NumberPrefix::Prefixed(Prefix::Kilo, 1.111f64))
    }

    #[test]
    fn binary_126456() {
        assert_eq!(NumberPrefix::binary(126_456f32),
                   NumberPrefix::Prefixed(Prefix::Kibi, 123.492188f32))
    }

    #[test]
    fn binary_1048576() {
        assert_eq!(NumberPrefix::binary(1_048_576f64),
                   NumberPrefix::Prefixed(Prefix::Mebi, 1f64))
    }

    #[test]
    fn binary_1073741824() {
        assert_eq!(NumberPrefix::binary(2_147_483_648f32),
                   NumberPrefix::Prefixed(Prefix::Gibi, 2f32))
    }

    #[test]
    fn giga() {
    	assert_eq!(NumberPrefix::decimal(1_000_000_000f64),
    	           NumberPrefix::Prefixed(Prefix::Giga, 1f64))
    }

    #[test]
    fn tera() {
    	assert_eq!(NumberPrefix::decimal(1_000_000_000_000f64),
    	           NumberPrefix::Prefixed(Prefix::Tera, 1f64))
    }

    #[test]
    fn peta() {
    	assert_eq!(NumberPrefix::decimal(1_000_000_000_000_000f64),
    	           NumberPrefix::Prefixed(Prefix::Peta, 1f64))
    }

    #[test]
    fn exa() {
    	assert_eq!(NumberPrefix::decimal(1_000_000_000_000_000_000f64),
    	           NumberPrefix::Prefixed(Prefix::Exa, 1f64))
    }

    #[test]
    fn zetta() {
    	assert_eq!(NumberPrefix::decimal(1_000_000_000_000_000_000_000f64),
    	           NumberPrefix::Prefixed(Prefix::Zetta, 1f64))
    }

    #[test]
    fn yotta() {
    	assert_eq!(NumberPrefix::decimal(1_000_000_000_000_000_000_000_000f64),
    	           NumberPrefix::Prefixed(Prefix::Yotta, 1f64))
    }

    #[test]
    #[allow(overflowing_literals)]
    fn and_so_on() {
    	// When you hit yotta, don't keep going
		assert_eq!(NumberPrefix::decimal(1_000_000_000_000_000_000_000_000_000f64),
		           NumberPrefix::Prefixed(Prefix::Yotta, 1000f64))
    }
}
