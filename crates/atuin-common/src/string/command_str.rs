//! A string proven to contain no NUL bytes.

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A string proven to contain no NUL byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandStr<T = String>(T);

/// The error returned when a string contains a NUL byte.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("string contains a NUL byte at index {index}")]
pub struct ContainsNul {
    /// Byte index of the first NUL.
    pub index: usize,
}

impl<T: AsRef<str>> CommandStr<T> {
    /// Wrap `inner`, or fail if it contains a NUL byte.
    pub fn new(inner: T) -> Result<Self, ContainsNul> {
        match inner.as_ref().find('\0') {
            Some(index) => Err(ContainsNul { index }),
            None => Ok(Self(inner)),
        }
    }

    /// The wrapped command as a string slice.
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: AsRef<str>> Deref for CommandStr<T> {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: AsRef<str>> AsRef<str> for CommandStr<T> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: AsRef<str>> fmt::Display for CommandStr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_ref())
    }
}

impl<T: AsRef<str>> Serialize for CommandStr<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.as_ref())
    }
}

impl<'de, T> Deserialize<'de> for CommandStr<T>
where
    T: Deserialize<'de> + AsRef<str>,
{
    /// Deserialize the backing value, then validate it — a NUL byte is a
    /// deserialization error, not something to trim away.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let inner = T::deserialize(deserializer)?;
        Self::new(inner).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandStr, ContainsNul};
    use proptest::prelude::*;
    use rstest::rstest;

    /// A string with no NUL wraps unchanged.
    #[rstest]
    #[case("echo hello")]
    #[case("")]
    #[case("🦀 build --release")]
    #[case("ls -la | grep foo")]
    fn wraps_nul_free_strings(#[case] input: &str) {
        assert_eq!(CommandStr::new(input).unwrap().as_str(), input);
    }

    /// A string with a NUL is rejected, pointing at the first NUL.
    #[rstest]
    #[case::interior("echo hi\0rm -rf /", 7)]
    #[case::trailing("ls\0", 2)]
    #[case::leading("\0danger", 0)]
    #[case::first_of_many("a\0b\0c", 1)]
    fn rejects_strings_with_nul(#[case] input: &str, #[case] index: usize) {
        assert_eq!(CommandStr::new(input), Err(ContainsNul { index }));
    }

    #[test]
    fn wraps_any_str_backing() {
        // Owned or borrowed, the same contents share a str view.
        let owned = CommandStr::new(String::from("cat file")).unwrap();
        let borrowed = CommandStr::new("cat file").unwrap();
        assert_eq!(owned.as_str(), borrowed.as_str());
    }

    #[test]
    fn derefs_to_str() {
        let c = CommandStr::new("echo hi").unwrap();
        assert_eq!(c.len(), 7);
        assert!(c.starts_with("echo"));
        assert!(!c.is_empty());
    }

    #[test]
    fn display_shows_the_command() {
        assert_eq!(CommandStr::new("ls -la").unwrap().to_string(), "ls -la");
    }

    #[test]
    fn serializes_as_plain_string() {
        let json = serde_json::to_string(&CommandStr::new("echo hi").unwrap()).unwrap();
        assert_eq!(json, r#""echo hi""#);
    }

    #[test]
    fn deserializes_nul_free_string() {
        let c: CommandStr<String> = serde_json::from_str(r#""echo hi""#).unwrap();
        assert_eq!(c.as_str(), "echo hi");
    }

    #[test]
    fn deserialize_rejects_nul() {
        // A JSON string carrying a NUL fails to deserialize — it is not trimmed.
        let json = serde_json::to_string("echo hi\0rm -rf /").unwrap();
        let err = serde_json::from_str::<CommandStr<String>>(&json).unwrap_err();
        assert!(err.is_data());
    }

    #[test]
    fn deserialize_rejects_non_string() {
        // A number is not a command; serde surfaces a data-category error.
        let err = serde_json::from_str::<CommandStr<String>>("42").unwrap_err();
        assert!(err.is_data());
    }

    proptest! {
        /// Wrapping succeeds iff there is no NUL, and reports the first NUL's index.
        #[test]
        fn validates_against_nul(s in r"(?s).*") {
            let result = CommandStr::new(s.as_str());
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
            let original = CommandStr::new(s).unwrap();
            let json = serde_json::to_string(&original).unwrap();
            let back: CommandStr<String> = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(back, original);
        }
    }
}
