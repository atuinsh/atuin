use std::{error::Error, fmt, str};

use super::{NumberPrefix, Prefix};


impl<T: str::FromStr> str::FromStr for NumberPrefix<T> {
    type Err = NumberPrefixParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let splitted = s.find(|p| {
            p == 'k' || p == 'K' || p == 'M' || p == 'G' || p == 'T' ||
            p == 'P' || p == 'E' || p == 'Z' || p == 'Y'
        });

        let num_prefix = s.split_at(splitted.unwrap_or(s.len()));
        let num = match num_prefix.0.trim().parse::<T>() {
            Ok(n)  => n,
            Err(_) => return Err(NumberPrefixParseError(())),
        };

        let prefix_unit = num_prefix.1.trim_matches(|p|
                p == 'b' || p == 'B' || p == 'm'
            );

        let prefix = match prefix_unit {
            "k"  |
            "K"  => Prefix::Kilo,
            "M"  => Prefix::Mega,
            "G"  => Prefix::Giga,
            "T"  => Prefix::Tera,
            "P"  => Prefix::Peta,
            "E"  => Prefix::Exa,
            "Z"  => Prefix::Zetta,
            "Y"  => Prefix::Yotta,
            "Ki" => Prefix::Kibi,
            "Mi" => Prefix::Mebi,
            "Gi" => Prefix::Gibi,
            "Ti" => Prefix::Tebi,
            "Pi" => Prefix::Pebi,
            "Ei" => Prefix::Exbi,
            "Zi" => Prefix::Zebi,
            "Yi" => Prefix::Yobi,
            ""   => return Ok(NumberPrefix::Standalone(num)),
            _    => return Err(NumberPrefixParseError(())),
        };

        Ok(NumberPrefix::Prefixed(prefix, num))
    }
}


/// The error returned when a `NumberPrefix` is failed to be parsed.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct NumberPrefixParseError(());

impl fmt::Display for NumberPrefixParseError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str("invalid prefix syntax")
    }
}

impl Error for NumberPrefixParseError {
}


#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_examples() {
        let parse_example_a = "7.05E".parse::<NumberPrefix<f64>>();
        let parse_example_b = "7.05".parse::<NumberPrefix<f64>>();
        let parse_example_c = "7.05 GiB".parse::<NumberPrefix<f64>>();

        assert_eq!(parse_example_a, Ok(NumberPrefix::Prefixed(Prefix::Exa, 7.05_f64)));
        assert_eq!(parse_example_b, Ok(NumberPrefix::Standalone(7.05_f64)));
        assert_eq!(parse_example_c, Ok(NumberPrefix::Prefixed(Prefix::Gibi, 7.05_f64)));
    }

    #[test]
    fn bad_parse() {
        let parsed = "bogo meters per second".parse::<NumberPrefix<f64>>();

        assert_ne!(parsed, Ok(NumberPrefix::Prefixed(Prefix::Kilo, 7.05_f64)));
    }
}
