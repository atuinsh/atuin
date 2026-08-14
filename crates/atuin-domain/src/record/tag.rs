use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// The type of data a [`super::Record`] stores (e.g. history, kv).
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    strum_macros::AsRefStr,
    strum_macros::Display,
    strum_macros::EnumString,
)]
pub enum RecordTag {
    #[strum(serialize = "history")]
    History,
    #[strum(serialize = "kv")]
    Kv,
    #[strum(serialize = "script")]
    Script,
    #[strum(serialize = "dotfiles-var")]
    DotfilesVar,
    #[strum(serialize = "config-shell-alias")]
    ConfigShellAlias,
    #[strum(serialize = "packfile")]
    Packfile,
    /// Legacy code supported arbitrary types, so we need to support this.
    #[strum(default, transparent)]
    Other(String),
}

impl RecordTag {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }

    fn variant_rank(&self) -> u8 {
        match self {
            RecordTag::History => 0,
            RecordTag::Kv => 1,
            RecordTag::Script => 2,
            RecordTag::DotfilesVar => 3,
            RecordTag::ConfigShellAlias => 4,
            RecordTag::Packfile => 5,
            RecordTag::Other(_) => 6,
        }
    }
}

impl Ord for RecordTag {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str()
            .cmp(other.as_str())
            .then_with(|| self.variant_rank().cmp(&other.variant_rank()))
    }
}

impl PartialOrd for RecordTag {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl From<String> for RecordTag {
    fn from(s: String) -> Self {
        // TODO(markovejnovic): Figure out a better way to avoid the implicit `Clone` in here.
        Self::from(s.as_str())
    }
}

impl Serialize for RecordTag {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_ref())
    }
}

impl<'de> Deserialize<'de> for RecordTag {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        // TODO(markovejnovic): Figure out a better way to avoid the implicit `Clone` in here.
        Ok(Self::from(s.as_ref()))
    }
}
