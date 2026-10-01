//! How spec schemas map onto Rust types, and the checks that keep every schema on its mapping.
//!
//! progenitor's `with_conversion` matches a schema exactly, metadata aside: a password with a
//! `minLength` or an `x-` extension would silently become a generated `String` newtype that
//! `Debug` prints, and a `u64` field would silently become an `i64`. `with_replacement` hands a
//! whole component to a hand-written type, which ignores any property the hub adds later. [`check`]
//! fails the generation in both cases instead.

use std::iter;

use progenitor_impl::{GenerationSettings, InterfaceStyle, TypeImpl, TypePatch};
use quote::quote;
use schemars08::schema::{InstanceType, NumberValidation, SchemaObject};
use serde_json::{Map, Value};

/// A component schema the client (de)serializes through a hand-written type.
struct Replacement {
    title: &'static str,
    target: &'static str,
    /// The wire fields `target` (de)serializes, sorted.
    properties: &'static [&'static str],
}

/// The record types carry invariants and the frozen field names of the PASETO implicit assertion,
/// so the client uses atuin-domain's rather than generating look-alikes.
const REPLACEMENTS: [Replacement; 4] = [
    Replacement {
        title: "EncryptedData",
        target: "::atuin_domain::record::EncryptedData",
        properties: &["content_encryption_key", "data"],
    },
    Replacement {
        title: "Host",
        target: "::atuin_domain::record::Host",
        properties: &["id", "name"],
    },
    Replacement {
        title: "Record",
        target: "::atuin_domain::record::Record<::atuin_domain::record::EncryptedData>",
        properties: &["data", "host", "id", "idx", "tag", "timestamp", "version"],
    },
    Replacement {
        title: "RecordStatus",
        target: "::atuin_domain::record::RecordStatus",
        properties: &["hosts"],
    },
];

/// A primitive schema the client (de)serializes through a chosen type.
struct Conversion {
    /// The schema's `type`, as the schemars value `with_conversion` matches on.
    instance_type: InstanceType,
    /// The same `type` as the spec spells it (`string`, `integer`), for error messages.
    json_type: &'static str,
    /// The `format` that selects this conversion, e.g. `password`.
    format: &'static str,
    /// The `minimum` the schema must carry, if any; an `int64` without one stays `i64`.
    minimum: Option<i32>,
    /// Path of the Rust type the schema becomes, e.g. `crate::Secret`.
    target: &'static str,
    /// Traits `target` implements that generated code may rely on, e.g. `FromStr` for a query
    /// parameter.
    impls: &'static [TypeImpl],
}

/// Generated types the client compares with `==`. Types holding a `Secret` cannot be among
/// them: `SecretString` has no `PartialEq`, by design.
const COMPARABLE: [&str; 4] = ["ModelInfo", "ModelList", "UsageBucket", "UsageSnapshot"];

const CONVERSIONS: [Conversion; 4] = [
    Conversion {
        instance_type: InstanceType::String,
        json_type: "string",
        format: "password",
        minimum: None,
        target: "crate::Secret",
        impls: &[],
    },
    Conversion {
        instance_type: InstanceType::Integer,
        json_type: "integer",
        format: "int64",
        minimum: Some(0),
        target: "u64",
        impls: &[TypeImpl::Default, TypeImpl::Display, TypeImpl::FromStr],
    },
    Conversion {
        instance_type: InstanceType::String,
        json_type: "string",
        format: "date-time",
        minimum: None,
        target: "crate::DateTime",
        impls: &[],
    },
    Conversion {
        instance_type: InstanceType::String,
        json_type: "string",
        format: "uri",
        minimum: None,
        target: "::url::Url",
        impls: &[TypeImpl::Display, TypeImpl::FromStr],
    },
];

/// Keywords progenitor keeps out of a schema's shape, so `with_conversion` still matches with them.
/// `nullable` wraps the converted type in an `Option`.
const SHAPELESS: [&str; 9] = [
    "default",
    "deprecated",
    "description",
    "example",
    "externalDocs",
    "nullable",
    "readOnly",
    "title",
    "writeOnly",
];

/// Errors returned by [`check`] and [`crate::prepare::strip`].
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    #[error("the spec has no {0} object")]
    Missing(&'static str),
    #[error("{0} is not an object")]
    NotAnObject(String),
    #[error(".components.schemas.{title} is missing, but the client maps it onto {target}")]
    MissingReplacement {
        title: &'static str,
        target: &'static str,
    },
    #[error(
        ".components.schemas.{title} has properties {found:?}, but {target} (de)serializes \
         {expected:?}; change the type and this generator's mapping together"
    )]
    ReplacementShape {
        title: &'static str,
        target: &'static str,
        found: Vec<String>,
        expected: &'static [&'static str],
    },
    #[error(
        "{path}: {stray:?} make this `format: {format}` schema escape its mapping onto {target}, \
         which needs `type: {json_type}`{minimum}; drop them from the hub's schema or extend the \
         generator's mapping"
    )]
    Escaped {
        path: String,
        format: &'static str,
        json_type: &'static str,
        minimum: &'static str,
        target: &'static str,
        stray: Vec<String>,
    },
}

/// The progenitor settings the client is generated with.
pub fn settings() -> GenerationSettings {
    let mut settings = GenerationSettings::new();
    settings.with_interface(InterfaceStyle::Positional).with_inner_type(quote!(crate::HookState));
    for replacement in &REPLACEMENTS {
        settings.with_replacement(replacement.title, replacement.target, iter::empty());
    }
    for conversion in &CONVERSIONS {
        settings.with_conversion(
            conversion.schema(),
            conversion.target,
            conversion.impls.iter().copied(),
        );
    }
    let mut comparable = TypePatch::default();
    comparable.with_derive("PartialEq").with_derive("Eq");
    for title in COMPARABLE {
        settings.with_patch(title, &comparable);
    }
    settings
}

/// Fail when a replaced component is missing or reshaped, or a schema escapes its conversion.
pub fn check(spec: &Value) -> Result<(), SpecError> {
    check_replacements(spec)?;
    check_conversions(spec, "")
}

fn check_replacements(spec: &Value) -> Result<(), SpecError> {
    let schemas = spec
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .ok_or(SpecError::Missing(".components.schemas"))?;
    for replacement in &REPLACEMENTS {
        let schema = schemas.get(replacement.title).ok_or(SpecError::MissingReplacement {
            title: replacement.title,
            target: replacement.target,
        })?;
        let mut found: Vec<String> = schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default();
        found.sort_unstable();
        if found != replacement.properties {
            return Err(SpecError::ReplacementShape {
                title: replacement.title,
                target: replacement.target,
                found,
                expected: replacement.properties,
            });
        }
    }
    Ok(())
}

/// Walk every object in the spec; `path` is the jq filter that reaches `value`.
fn check_conversions(value: &Value, path: &str) -> Result<(), SpecError> {
    match value {
        Value::Object(object) => {
            check_schema(object, path)?;
            for (key, child) in object {
                check_conversions(child, &child_path(path, key))?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                check_conversions(item, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}

fn check_schema(schema: &Map<String, Value>, path: &str) -> Result<(), SpecError> {
    let Some(conversion) = CONVERSIONS.iter().find(|conversion| conversion.claims(schema)) else {
        return Ok(());
    };
    let stray = conversion.stray(schema);
    if stray.is_empty() {
        return Ok(());
    }
    Err(SpecError::Escaped {
        path: path.to_owned(),
        format: conversion.format,
        json_type: conversion.json_type,
        minimum: if conversion.minimum.is_some() {
            " and `minimum: 0`"
        } else {
            ""
        },
        target: conversion.target,
        stray,
    })
}

fn child_path(path: &str, key: &str) -> String {
    let identifier = key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if identifier {
        format!("{path}.{key}")
    } else {
        format!("{path}[{key:?}]")
    }
}

impl Conversion {
    fn schema(&self) -> SchemaObject {
        SchemaObject {
            instance_type: Some(self.instance_type.into()),
            format: Some(self.format.to_owned()),
            number: self.minimum.map(|minimum| {
                Box::new(NumberValidation {
                    minimum: Some(f64::from(minimum)),
                    ..NumberValidation::default()
                })
            }),
            ..SchemaObject::default()
        }
    }

    /// Whether `schema` is one this conversion must map.
    fn claims(&self, schema: &Map<String, Value>) -> bool {
        schema.get("format").and_then(Value::as_str) == Some(self.format)
            && (self.minimum.is_none() || schema.contains_key("minimum"))
    }

    /// The keywords of a claimed `schema` that defeat the conversion; `type` when it is missing.
    fn stray(&self, schema: &Map<String, Value>) -> Vec<String> {
        let mut stray: Vec<String> = schema
            .iter()
            .filter(|(key, value)| !self.allows(key, value))
            .map(|(key, _)| key.clone())
            .collect();
        if !schema.contains_key("type") {
            stray.push("type".to_owned());
        }
        stray
    }

    fn allows(&self, key: &str, value: &Value) -> bool {
        match key {
            "type" => value.as_str() == Some(self.json_type),
            "format" => true,
            "minimum" => self.minimum.is_some_and(|minimum| value.as_i64() == Some(minimum.into())),
            _ => SHAPELESS.contains(&key),
        }
    }
}
