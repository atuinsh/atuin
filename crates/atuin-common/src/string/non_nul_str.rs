//! A string proven to contain no NUL bytes.

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A string proven to contain no NUL byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NonNulStr<T = String>(T);

/// The error returned when a string contains a NUL byte.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("string contains a NUL byte at index {index}")]
pub struct ContainsNul {
    /// Byte index of the first NUL.
    pub index: usize,
}

impl<T: AsRef<str>> NonNulStr<T> {
    /// Wrap `inner`, or fail if it contains a NUL byte.
    pub fn new(inner: T) -> Result<Self, ContainsNul> {
        match inner.as_ref().find('\0') {
            Some(index) => Err(ContainsNul { index }),
            None => Ok(Self(inner)),
        }
    }

    /// The wrapped string as a slice.
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: AsRef<str>> Deref for NonNulStr<T> {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: AsRef<str>> AsRef<str> for NonNulStr<T> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: AsRef<str>> fmt::Display for NonNulStr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_ref())
    }
}

impl<T: AsRef<str>> Serialize for NonNulStr<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.as_ref())
    }
}

impl<'de, T> Deserialize<'de> for NonNulStr<T>
where
    T: Deserialize<'de> + AsRef<str>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let inner = T::deserialize(deserializer)?;
        Self::new(inner).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::{NonNulStr, ContainsNul};
    use proptest::prelude::*;
    use rstest::rstest;

    #[rstest]
    #[case("echo hello")]
    #[case("")]
    #[case("🦀 build --release")]
    #[case("ls -la | grep foo")]
    fn wraps_nul_free_strings(#[case] input: &str) {
        assert_eq!(NonNulStr::new(input).unwrap().as_str(), input);
    }

    #[rstest]
    #[case::interior("echo hi\0rm -rf /", 7)]
    #[case::trailing("ls\0", 2)]
    #[case::leading("\0danger", 0)]
    #[case::first_of_many("a\0b\0c", 1)]
    fn rejects_strings_with_nul(#[case] input: &str, #[case] index: usize) {
        assert_eq!(NonNulStr::new(input), Err(ContainsNul { index }));
    }

    #[test]
    fn serializes_as_plain_string() {
        let json = serde_json::to_string(&NonNulStr::new("echo hi").unwrap()).unwrap();
        assert_eq!(json, r#""echo hi""#);
    }

    #[test]
    fn deserializes_nul_free_string() {
        let c: NonNulStr<String> = serde_json::from_str(r#""echo hi""#).unwrap();
        assert_eq!(c.as_str(), "echo hi");
    }

    #[test]
    fn deserialize_rejects_nul() {
        // A JSON string carrying a NUL fails to deserialize — it is not trimmed.
        let json = serde_json::to_string("echo hi\0rm -rf /").unwrap();
        let err = serde_json::from_str::<NonNulStr<String>>(&json).unwrap_err();
        assert!(err.is_data());
    }

    #[test]
    fn deserialize_rejects_non_string() {
        // A number is not a command; serde surfaces a data-category error.
        let err = serde_json::from_str::<NonNulStr<String>>("42").unwrap_err();
        assert!(err.is_data());
    }

    proptest! {
        /// Wrapping succeeds iff there is no NUL, and reports the first NUL's index.
        #[test]
        fn validates_against_nul(s in r"(?s).*") {
            let result = NonNulStr::new(s.as_str());
            match s.find('\0') {
                None => {
                    let command = result.unwrap();
                    prop_assert_eq!(command.as_str(), s.as_str());
                }
                Some(index) => prop_assert_eq!(result.unwrap_err().index, index),
            }
        }

        /// A NUL-free command serialize → deserialize round-trips unchanged.
        #[test]
        fn serde_round_trip(s in r"[^\x00]*") {
            let original = NonNulStr::new(s).unwrap();
            let json = serde_json::to_string(&original).unwrap();
            let back: NonNulStr<String> = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(back, original);
        }
    }
}
