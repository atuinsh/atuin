//! A string proven to hold something other than whitespace.

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};

/// A string with at least one non-whitespace character, stored without surrounding whitespace.
///
/// Deserializing a blank value fails and the JSON schema says so (`minLength: 1`), so a free-text
/// parameter such as a search query carries its own validation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct NonBlankString(String);

/// The error returned when a string is empty or all whitespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("string is blank")]
pub struct Blank;

impl NonBlankString {
    /// Trim `inner`, or fail if nothing is left.
    pub fn new(inner: impl AsRef<str>) -> Result<Self, Blank> {
        match inner.as_ref().trim() {
            "" => Err(Blank),
            trimmed => Ok(Self(trimmed.to_owned())),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for NonBlankString {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for NonBlankString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NonBlankString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for NonBlankString {
    type Err = Blank;

    fn from_str(s: &str) -> Result<Self, Blank> {
        Self::new(s)
    }
}

impl<'de> Deserialize<'de> for NonBlankString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(<Cow<'de, str>>::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for NonBlankString {
    fn schema_name() -> Cow<'static, str> {
        "NonBlankString".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "minLength": 1 })
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Deserialize, JsonSchema)]
    struct Params {
        query: NonBlankString,
    }

    #[rstest]
    #[case::plain("disk", "disk")]
    #[case::trimmed("  disk full \n", "disk full")]
    fn keeps_the_trimmed_text(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(NonBlankString::new(input).unwrap().as_str(), expected);
        let params: Params = serde_json::from_value(json!({"query": input})).unwrap();
        assert_eq!(params.query.as_str(), expected);
    }

    #[rstest]
    #[case::empty("")]
    #[case::whitespace(" \t\n")]
    fn rejects_blank_text(#[case] input: &str) {
        assert_eq!(NonBlankString::new(input), Err(Blank));
        assert!(serde_json::from_value::<Params>(json!({"query": input})).is_err());
    }

    #[rstest]
    fn json_schema_requires_a_character() {
        let schema = schemars::schema_for!(Params);
        let query = &schema.as_value()["properties"]["query"];
        assert_eq!(query["type"], "string");
        assert_eq!(query["minLength"], 1);
        assert_eq!(schema.as_value()["required"], json!(["query"]));
    }
}
