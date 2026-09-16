//! A string proven to hold something other than whitespace.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};

/// A string with at least one non-whitespace character, held verbatim. `T` is any string type:
/// `String`, `&str`, `Cow<str>`, `Box<str>`, ...
///
/// Deserializing a blank value fails and the JSON schema says so (`minLength: 1`), so a free-text
/// parameter such as a search query carries its own validation.
#[derive(
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    derive_more::AsRef,
    derive_more::Debug,
    derive_more::Deref,
    derive_more::Display,
)]
#[as_ref(forward)]
#[debug("{_0:?}")]
#[deref(forward)]
#[display("{_0}")]
pub struct NonBlank<T = String>(T);

pub type NonBlankString = NonBlank<String>;

/// The error returned when a string is empty or all whitespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("string is blank")]
pub struct Blank;

impl<T: AsRef<str>> NonBlank<T> {
    /// Wrap `inner`, or fail if it is empty or all whitespace.
    pub fn new(inner: T) -> Result<Self, Blank> {
        if inner.as_ref().trim().is_empty() {
            Err(Blank)
        } else {
            Ok(Self(inner))
        }
    }

    /// The wrapped string as a slice.
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<'de, T: AsRef<str> + Deserialize<'de>> Deserialize<'de> for NonBlank<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(T::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl<T> JsonSchema for NonBlank<T> {
    fn schema_name() -> Cow<'static, str> {
        "NonBlank".into()
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
    #[case::plain("disk")]
    #[case::padded("  disk full \n")]
    fn holds_the_text_verbatim(#[case] input: &str) {
        let borrowed: NonBlank<&str> = NonBlank::new(input).unwrap();
        assert_eq!(borrowed.as_str(), input);
        assert_eq!(&*borrowed, input);
        assert_eq!(borrowed.to_string(), input);
        assert_eq!(NonBlankString::new(input.to_owned()).unwrap().as_str(), input);
        let params: Params = serde_json::from_value(json!({"query": input})).unwrap();
        assert_eq!(params.query.as_str(), input);
        assert_eq!(serde_json::to_value(&params.query).unwrap(), json!(input));
        assert_eq!(params.query.into_inner(), input);
    }

    #[rstest]
    #[case::empty("")]
    #[case::whitespace(" \t\n")]
    fn rejects_blank_text(#[case] input: &str) {
        assert_eq!(NonBlank::new(input), Err(Blank));
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
