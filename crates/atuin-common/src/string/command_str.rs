//! A command string sanitized at ingest: everything up to its first NUL byte.
//!
//! Coding agents occasionally hand `atuin hook` a command carrying a NUL byte
//! and trailing junk. A C string ends at its first NUL, and so does this: the
//! stored value holds no interior NUL, so the parts of Atuin that treat a
//! command as text never see one.

use std::ffi::CString;
use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A command truncated at its first NUL byte.
///
/// Every constructor keeps only the bytes before the first NUL, so the wrapped
/// [`CString`] never contains an interior NUL and always dereferences to valid
/// UTF-8.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CommandStr(CString);

impl CommandStr {
    /// Build a `CommandStr`, keeping only the text before the first NUL byte.
    pub fn new(s: impl AsRef<str>) -> Self {
        let s = s.as_ref();
        let end = s.find('\0').unwrap_or(s.len());
        // `s[..end]` stops before the first NUL, so it has no interior NUL and
        // `CString::new` cannot fail.
        Self(CString::new(&s.as_bytes()[..end]).expect("slice has no interior NUL byte"))
    }

    /// The command as a string slice — always valid UTF-8 by construction.
    pub fn as_str(&self) -> &str {
        self.0
            .to_str()
            .expect("constructed from valid UTF-8, so always valid UTF-8")
    }
}

impl Deref for CommandStr {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for CommandStr {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for CommandStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for CommandStr {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for CommandStr {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl Serialize for CommandStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CommandStr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(s))
    }
}

#[cfg(test)]
mod tests {
    use super::CommandStr;
    use proptest::prelude::*;
    use rstest::rstest;

    /// Exact truncation cases. Each is its own test so a failure names the input.
    #[rstest]
    // No NUL: preserved verbatim.
    #[case("echo hello", "echo hello")]
    #[case("", "")]
    // Multi-byte UTF-8 before a NUL survives intact.
    #[case("🦀 build\0junk", "🦀 build")]
    // Interior NUL: everything from the NUL onward is dropped (issue #3589).
    #[case("echo hi\0rm -rf /", "echo hi")]
    // Trailing NUL: the NUL and anything after it.
    #[case("ls\0", "ls")]
    // Leading NUL: nothing survives.
    #[case("\0danger", "")]
    // Only the first NUL matters.
    #[case("a\0b\0c", "a")]
    fn truncates_at_first_nul(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(CommandStr::new(input).as_str(), expected);
    }

    #[test]
    fn default_is_empty() {
        assert_eq!(CommandStr::default().as_str(), "");
        assert!(CommandStr::default().is_empty());
    }

    #[test]
    fn equal_when_truncations_match() {
        assert_eq!(CommandStr::new("a\0b"), CommandStr::new("a"));
        assert_ne!(CommandStr::new("a"), CommandStr::new("b"));
    }

    #[test]
    fn derefs_to_str() {
        let c = CommandStr::new("echo hi\0x");
        assert_eq!(c.len(), 7);
        assert!(c.starts_with("echo"));
        assert!(!c.is_empty());
    }

    #[test]
    fn display_shows_truncated_text() {
        assert_eq!(CommandStr::new("ls -la\0\0").to_string(), "ls -la");
    }

    #[test]
    fn from_str_and_string_agree() {
        assert_eq!(
            CommandStr::from("cat\0x"),
            CommandStr::from(String::from("cat\0y"))
        );
    }

    #[test]
    fn serializes_as_plain_string() {
        let json = serde_json::to_string(&CommandStr::new("echo hi\0x")).unwrap();
        assert_eq!(json, r#""echo hi""#);
    }

    #[test]
    fn deserializes_and_truncates() {
        // `\u0000` is a NUL escaped inside a JSON string.
        let c: CommandStr = serde_json::from_str(r#""echo hi\u0000rm -rf /""#).unwrap();
        assert_eq!(c.as_str(), "echo hi");
    }

    #[test]
    fn deserialize_rejects_non_string() {
        // A number is not a command; serde surfaces a data-category error.
        let err = serde_json::from_str::<CommandStr>("42").unwrap_err();
        assert!(err.is_data());
    }

    proptest! {
        /// A string with no NUL is preserved byte-for-byte.
        #[test]
        fn nul_free_is_unchanged(s in r"[^\x00]*") {
            let cs = CommandStr::new(&s);
            prop_assert_eq!(cs.as_str(), s.as_str());
        }

        /// The stored value never contains a NUL, whatever the input.
        #[test]
        fn never_contains_nul(s in r"(?s).*") {
            let cs = CommandStr::new(&s);
            prop_assert!(!cs.as_str().contains('\0'));
        }

        /// The result is exactly the input up to (excluding) its first NUL.
        #[test]
        fn equals_prefix_before_first_nul(s in r"(?s).*") {
            let cs = CommandStr::new(&s);
            let expected = s.split('\0').next().unwrap();
            prop_assert_eq!(cs.as_str(), expected);
        }

        /// serialize → deserialize round-trips any already-NUL-free command.
        #[test]
        fn serde_round_trip(s in r"[^\x00]*") {
            let original = CommandStr::new(&s);
            let json = serde_json::to_string(&original).unwrap();
            let back: CommandStr = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(back, original);
        }
    }
}
