//! Finds settings keys that are ignored when the config gets deserialized.
//!
//! Serde skips a key that no field names. When a user misspells a key, the setting that the user
//! meant to change silently stays at its default. [`deserialize`] records each skipped key that
//! the user's config file contains, so that the misspelling can be reported. A key that only the
//! defaults or the environment supply is not reported, because the environment holds operational
//! variables such as `ATUIN_SESSION`.

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};

use atuin_common::config::ValueExt as _;
use atuin_common::string::StrExt as _;
use config::{Config, ConfigError, Value, ValueKind};
use serde::de::DeserializeOwned;

/// A key that the config has renamed, and that still applies under its old name.
pub struct Renamed {
    pub old: &'static str,
    pub new: &'static str,
}

/// A deserialized value and the keys of the config file that it ignored.
pub struct Checked<T> {
    pub value: T,
    pub unknown: UnknownKeys,
}

/// The dotted paths of ignored config file keys, in sorted order.
///
/// The paths hold no values, so a secret in a misspelled key stays out of a warning.
#[derive(Debug, Default, derive_more::Deref)]
pub struct UnknownKeys(BTreeSet<String>);

impl UnknownKeys {
    /// Whether `key` is one of these keys, or a key inside a table that is one of them.
    #[must_use]
    pub fn covers(&self, key: &str) -> bool {
        self.0.iter().any(|unknown| key.has_dotted_prefix(unknown))
    }
}

impl fmt::Display for UnknownKeys {
    /// Lists the paths separated by commas.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, key) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(key)?;
        }
        Ok(())
    }
}

/// Deserializes `config`, recording the keys that it ignores and that `file` contains.
///
/// `file` is the user's config file without defaults or environment entries, and `None` when
/// there is no file. A key at or under the old name of a [`Renamed`] key is not reported,
/// because the rename still reads it. A typo inside a renamed table is reported under each name
/// that the file spells it with: the old name, the new name, or both. A key at or under one of
/// `unchecked` is not reported either.
pub fn deserialize<T: DeserializeOwned>(
    config: Config,
    file: Option<&Value>,
    renamed: &[Renamed],
    unchecked: &[&str],
) -> Result<Checked<T>, ConfigError> {
    let Some(file) = file else {
        return Ok(Checked {
            value: config.try_deserialize()?,
            unknown: UnknownKeys::default(),
        });
    };

    let mut unknown = BTreeSet::new();
    let mut key = String::new();
    let value = serde_ignored::deserialize(config, |path| {
        key.clear();
        let _ = write!(key, "{path}");

        let skipped = renamed.iter().map(|rename| rename.old).chain(unchecked.iter().copied());
        if skipped.into_iter().any(|prefix| key.has_dotted_prefix(prefix)) {
            return;
        }

        // The file may spell an ignored key under a renamed key's new name, its old name, or both.
        // Each spelling that the file has is reported, so that neither one looks valid.
        if value_at_path(file, &path).is_some() {
            unknown.insert(key.clone());
        }
        if let Some(old) = renamed.iter().find_map(|rename| old_spelling(&key, rename))
            // A renamed key has no quoted segments, so splitting it on dots is exact.
            && file.get_path(old.split('.')).is_some()
        {
            unknown.insert(old);
        }
    })?;

    Ok(Checked {
        value,
        unknown: UnknownKeys(unknown),
    })
}

/// The spelling of `key` under the old name of `rename`, if `key` is inside the new name.
///
/// For example, `threads.mouse` is `ai.sessions.mouse` under the rename of `ai.sessions`.
fn old_spelling(key: &str, rename: &Renamed) -> Option<String> {
    let rest = key.strip_dotted_prefix(rename.new)?;
    Some(if rest.is_empty() {
        rename.old.to_owned()
    } else {
        format!("{}.{rest}", rename.old)
    })
}

/// The value at the serde `path`, which keeps a key that contains dots as one segment.
fn value_at_path<'a>(root: &'a Value, path: &serde_ignored::Path<'_>) -> Option<&'a Value> {
    use serde_ignored::Path;

    match path {
        Path::Root => Some(root),
        Path::Map { parent, key } => value_at_path(root, parent)?.get_key(key),
        Path::Seq { parent, index } => match &value_at_path(root, parent)?.kind {
            ValueKind::Array(array) => array.get(*index),
            _ => None,
        },
        Path::Some { parent }
        | Path::NewtypeStruct { parent }
        | Path::NewtypeVariant { parent } => value_at_path(root, parent),
    }
}
