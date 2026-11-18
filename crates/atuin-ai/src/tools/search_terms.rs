//! The terms a search tool looks for.

use std::borrow::Cow;

use atuin_common::string::NonBlankString;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer};

/// At least one term, each holding more than whitespace. A term of several words is a phrase.
#[derive(Clone, derive_more::Debug)]
#[debug("{_0:?}")]
pub struct SearchTermsArg(Vec<NonBlankString>);

impl SearchTermsArg {
    /// Each term, as the search takes it.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(NonBlankString::as_str)
    }
}

impl<'de> Deserialize<'de> for SearchTermsArg {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Models often pass one string where the schema asks for a list; take it as one term.
        let terms = match serde_json::Value::deserialize(deserializer)? {
            term @ serde_json::Value::String(_) => vec![term],
            other => Vec::deserialize(other).map_err(serde::de::Error::custom)?,
        };
        let terms = terms
            .into_iter()
            .map(NonBlankString::deserialize)
            .collect::<Result<Vec<_>, _>>()
            .map_err(serde::de::Error::custom)?;
        if terms.is_empty() {
            return Err(serde::de::Error::invalid_length(0, &"at least one term"));
        }
        Ok(Self(terms))
    }
}

impl JsonSchema for SearchTermsArg {
    fn schema_name() -> Cow<'static, str> {
        "SearchTerms".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "array",
            "items": generator.subschema_for::<NonBlankString>(),
            "minItems": 1,
        })
    }
}
