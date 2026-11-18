//! Serde aliases of settings that have default values set by the config builder.
//!
//! The config builder sets each field's default under the field's canonical name, such as
//! `daemon.enabled`. A file that spells the field by its alias, such as `[daemon] enable = true`,
//! then leaves both names in the merged config, and serde rejects the pair as a duplicate field.
//! [`CanonicalAliases`] renames a file's aliases before the merge so that the file's value
//! replaces the default. The environment, layered on top, then overrides the file's value as
//! usual.

use atuin_common::config::MapExt as _;
use config::{Map, Source, Value};

/// A `#[serde(alias)]` of a settings field whose default the config builder sets.
struct Alias {
    /// The path of the table that holds the field.
    table: &'static [&'static str],
    /// The name that the field's `#[serde(alias)]` accepts.
    alias: &'static str,
    /// The field's own name, under which the config builder sets the default.
    canonical: &'static str,
}

/// Every `#[serde(alias)]` of a field that has a config builder default. A new alias on such a
/// field must be added here, or a file that uses the alias fails to load.
const ALIASES: [Alias; 2] = [
    Alias {
        table: &["daemon"],
        alias: "enable",
        canonical: "enabled",
    },
    Alias {
        table: &["pty_proxy"],
        alias: "enable",
        canonical: "enabled",
    },
];

/// A config file source with the [`ALIASES`] in it renamed to their canonical names.
///
/// A file that sets both the alias and the canonical name keeps both, and fails to deserialize
/// as a duplicate field.
#[derive(Debug)]
pub struct CanonicalAliases(Box<dyn Source + Send + Sync>);

impl CanonicalAliases {
    pub fn new(file: impl Source + Send + Sync + 'static) -> Self {
        Self(Box::new(file))
    }
}

impl Source for CanonicalAliases {
    fn clone_into_box(&self) -> Box<dyn Source + Send + Sync> {
        Box::new(Self(self.0.clone_into_box()))
    }

    fn collect(&self) -> Result<Map<String, Value>, config::ConfigError> {
        let mut file = self.0.collect()?;
        for Alias {
            table,
            alias,
            canonical,
        } in &ALIASES
        {
            let Some(map) = file.table_at_mut(table.iter().copied()) else {
                continue;
            };
            if map.contains_key(*canonical) {
                continue;
            }
            if let Some(value) = map.remove(*alias) {
                map.insert((*canonical).to_owned(), value);
            }
        }
        Ok(file)
    }
}
