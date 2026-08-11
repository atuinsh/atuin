use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// The format version the data in a [`super::Record`] conforms to (e.g. "v0", "v1").
///
/// Serialised as a bare string, byte-for-byte identical to the historical version strings —
/// this value is part of the PASETO implicit assertion and the sync wire format, so its
/// serialised form must never change. The version namespace is per-tag ("v1" means a different
/// format for `kv` than for `history`); this enum only captures the shared string
/// representation, each store interprets it in its own context. Unrecognised versions are
/// preserved losslessly in [`RecordVersion::Other`] so the record layer stays forward-compatible.
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
        // The strum attributes are the single source of truth for the strings; reuse the owned
        // allocation when the version is unknown.
        match Self::from(s.as_str()) {
            Self::Other(_) => Self::Other(s),
            known => known,
        }
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
        Ok(Self::from(s.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::RecordVersion;

    #[test]
    fn known_versions_round_trip() {
        for (v, s) in [
            (RecordVersion::V0, "v0"),
            (RecordVersion::V1, "v1"),
            (RecordVersion::V2, "v2"),
        ] {
            assert_eq!(v.as_str(), s);
            assert_eq!(AsRef::<str>::as_ref(&v), s);
            assert_eq!(v.to_string(), s);
            assert_eq!(RecordVersion::from(s), v);
            assert_eq!(RecordVersion::from(s.to_owned()), v);
        }
    }

    #[test]
    fn unknown_version_falls_back_to_other() {
        let v = RecordVersion::from("v99");
        assert_eq!(v, RecordVersion::Other("v99".to_owned()));
        assert_eq!(v.as_str(), "v99");
        assert_eq!(RecordVersion::from("v1"), RecordVersion::V1);
    }

    #[test]
    fn serializes_as_a_bare_string() {
        assert_eq!(
            serde_json::to_string(&RecordVersion::V1).unwrap(),
            r#""v1""#
        );
        assert_eq!(
            serde_json::to_string(&RecordVersion::Other("x".to_owned())).unwrap(),
            r#""x""#
        );
        assert_eq!(
            serde_json::from_str::<RecordVersion>(r#""v2""#).unwrap(),
            RecordVersion::V2
        );
        assert_eq!(
            serde_json::from_str::<RecordVersion>(r#""nope""#).unwrap(),
            RecordVersion::Other("nope".to_owned())
        );
    }
}
