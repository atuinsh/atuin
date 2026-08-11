use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// The format version the data in a [`super::Record`] conforms to (e.g. "v0", "v1").
///
/// Historically we used to use a bare string instead of this enum. Note that this is a footgun --
/// not all tags support all versions, and it would be significantly better if we had correct
/// versions for each record type, but this is an 80-20 to reduce `String::clone`s (which was the
/// status quo).
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    strum_macros::AsRefStr,
    strum_macros::Display,
    strum_macros::EnumString,
)]
pub enum RecordVersion {
    #[strum(serialize = "v0")]
    V0,
    #[strum(serialize = "v1")]
    V1,
    #[strum(serialize = "v2")]
    V2,
    /// Any version string without a dedicated variant.
    #[strum(default, transparent)]
    Other(String),
}

impl RecordVersion {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }
}

impl From<String> for RecordVersion {
    fn from(s: String) -> Self {
        // TODO(markovejnovic): Figure out a better way to avoid the implicit `Clone` in here.
        Self::from(s.as_str())
    }
}

impl Serialize for RecordVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_ref())
    }
}

impl<'de> Deserialize<'de> for RecordVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        // TODO(markovejnovic): Figure out a better way to avoid the implicit `Clone` in here.
        Ok(Self::from(s.as_ref()))
    }
}
