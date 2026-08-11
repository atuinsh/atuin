use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// The type of data a [`Record`] stores (e.g. history, kv).
///
/// Serialised as a bare string, byte-for-byte identical to the historical tag strings.
/// This value is part of the PASETO implicit assertion and the sync wire format, so its
/// serialised form must never change. Unrecognised tags (from the local DB, or records
/// synced off other/newer clients) are preserved losslessly in [`RecordTag::Other`] so
/// sync stays forward-compatible.
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
    /// Any tag without a dedicated variant. `default` makes `FromStr` infallible; `transparent`
    /// makes `AsRef<str>`/`Display` return the inner string rather than the variant name.
    #[strum(default, transparent)]
    Other(String),
}

impl RecordTag {
    /// Borrow the tag's canonical string form (zero-alloc). Equals the serialised bytes.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }

    /// Stable rank per variant, used only to make [`Ord`] total and consistent with [`Eq`]
    /// when two values share an `as_str()` (only reachable if `Other` is hand-built with a
    /// known tag string — the `From`/`FromStr` constructors never produce that).
    fn variant_rank(&self) -> u8 {
        match self {
            RecordTag::History => 0,
            RecordTag::Kv => 1,
            RecordTag::Script => 2,
            RecordTag::DotfilesVar => 3,
            RecordTag::ConfigShellAlias => 4,
            RecordTag::Other(_) => 5,
        }
    }
}

impl Ord for RecordTag {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Order by the canonical string so sync diffs/operations sort exactly as the former
        // `String` tags did; tie-break by variant so the order is total and Eq-consistent.
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

// Relies on the `strum`-derived `From<&str>` (the `#[strum(default)]` `Other` variant makes it
// infallible). Reuses the owned allocation when the tag is unknown.
impl From<String> for RecordTag {
    fn from(s: String) -> Self {
        // The strum attributes on the variants are the single source of truth for the
        // strings; reuse the owned allocation when the tag is unknown.
        match Self::from(s.as_str()) {
            Self::Other(_) => Self::Other(s),
            known => known,
        }
    }
}

impl Serialize for RecordTag {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_ref())
    }
}

impl<'de> Deserialize<'de> for RecordTag {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Accept a borrowed or owned string, then map into the enum infallibly.
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        Ok(Self::from(s.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_tag_known_variants_round_trip() {
        for (tag, s) in [
            (RecordTag::History, "history"),
            (RecordTag::Kv, "kv"),
            (RecordTag::Script, "script"),
            (RecordTag::DotfilesVar, "dotfiles-var"),
            (RecordTag::ConfigShellAlias, "config-shell-alias"),
        ] {
            assert_eq!(tag.as_str(), s, "as_str mismatch");
            assert_eq!(AsRef::<str>::as_ref(&tag), s, "as_ref mismatch");
            assert_eq!(tag.to_string(), s, "Display mismatch");
            assert_eq!(RecordTag::from(s), tag, "From<&str> mismatch");
            assert_eq!(RecordTag::from(s.to_owned()), tag, "From<String> mismatch");
        }
    }

    #[test]
    fn record_tag_unknown_falls_back_to_other() {
        let t = RecordTag::from("banana");
        assert_eq!(t, RecordTag::Other("banana".to_owned()));
        assert_eq!(t.as_str(), "banana");
        assert_eq!(t.to_string(), "banana");
        // A known string never lands in Other.
        assert_eq!(RecordTag::from("history"), RecordTag::History);
        assert_ne!(
            RecordTag::from("history"),
            RecordTag::Other("history".to_owned())
        );
    }

    #[test]
    fn record_tag_serializes_as_a_bare_string() {
        assert_eq!(
            serde_json::to_string(&RecordTag::History).unwrap(),
            r#""history""#
        );
        assert_eq!(
            serde_json::to_string(&RecordTag::DotfilesVar).unwrap(),
            r#""dotfiles-var""#
        );
        assert_eq!(
            serde_json::to_string(&RecordTag::Other("x".to_owned())).unwrap(),
            r#""x""#
        );
        assert_eq!(
            serde_json::from_str::<RecordTag>(r#""kv""#).unwrap(),
            RecordTag::Kv
        );
        assert_eq!(
            serde_json::from_str::<RecordTag>(r#""nope""#).unwrap(),
            RecordTag::Other("nope".to_owned())
        );
    }

    #[test]
    fn record_tag_orders_lexicographically_by_str() {
        // Matches the former `String` tag sort so sync diffs/operations are unchanged.
        let mut tags = [
            RecordTag::Kv,
            RecordTag::History,
            RecordTag::Other("zzz".to_owned()),
            RecordTag::ConfigShellAlias,
            RecordTag::DotfilesVar,
            RecordTag::Script,
        ];
        tags.sort();
        let as_strs: Vec<&str> = tags.iter().map(RecordTag::as_str).collect();
        assert_eq!(
            as_strs,
            vec![
                "config-shell-alias",
                "dotfiles-var",
                "history",
                "kv",
                "script",
                "zzz"
            ]
        );
        // Ord is consistent with Eq: equal values compare Equal, distinct values never Equal.
        assert_eq!(
            RecordTag::History.cmp(&RecordTag::History),
            std::cmp::Ordering::Equal
        );
        assert_ne!(
            RecordTag::History.cmp(&RecordTag::Other("history".to_owned())),
            std::cmp::Ordering::Equal
        );
    }
}
