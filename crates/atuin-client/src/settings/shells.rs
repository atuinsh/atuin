use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

/// Controls which shells' commands are included in interactive search.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Shells {
    /// Include all commands.
    All,

    #[default]
    /// Include commands run from the current shell, or commands that have no recorded shell.
    Auto,

    /// Include commands run by any shell in a list. The empty string will include commands that
    /// have no recorded shell.
    ///
    /// An empty list is treated the same as [`Self::All`], but [`Self::All`] should be preferred.
    List(Vec<String>),
}

impl Shells {
    /// Turn this setting into a concrete list of shells.
    ///
    /// The returned vector is suitable for passing to [`Database::search`] via
    /// [`OptFilters::shell`].
    ///
    /// [`Database::search`]: crate::database::Database::search
    /// [`OptFilters::shell`]: crate::database::OptFilters::shell
    pub fn to_list(&self) -> Vec<String> {
        match self {
            Self::All => vec![],
            Self::Auto => match std::env::var("ATUIN_SHELL") {
                // Show results from the current shell, plus entries that have no shell recorded.
                Ok(shell) => vec![shell, "".into()],
                // Show all results if no shell is detected.
                Err(_) => vec![],
            },
            Self::List(shells) => shells.clone(),
        }
    }
}

impl<'a> Deserialize<'a> for Shells {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        struct Visitor;

        impl<'a> de::Visitor<'a> for Visitor {
            type Value = Shells;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(r#""all", "auto", or an array of strings"#)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "all" => Ok(Shells::All),
                    "auto" => Ok(Shells::Auto),
                    other => Err(E::invalid_value(de::Unexpected::Str(other), &self)),
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'a>,
            {
                let mut shells = Vec::new();
                while let Some(shell) = seq.next_element()? {
                    shells.push(shell);
                }
                Ok(Shells::List(shells))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl Serialize for Shells {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Shells::All => serializer.serialize_str("all"),
            Shells::Auto => serializer.serialize_str("auto"),
            Shells::List(shells) => shells.serialize(serializer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Shells;
    use rstest::rstest;
    use serde::Deserialize;

    #[rstest]
    #[case::all(r#""all""#, Some(Shells::All))]
    #[case::auto(r#""auto""#, Some(Shells::Auto))]
    #[case::array(
        r#"["bash", "", "zsh"]"#,
        Some(Shells::List(vec!["bash".to_owned(), "".to_owned(), "zsh".to_owned()])),
    )]
    #[case::invalid_string(r#""hello""#, None)]
    fn deserialize(#[case] toml: &str, #[case] expected: Option<Shells>) {
        let deserializer = toml::de::ValueDeserializer::parse(toml).unwrap();
        let result = Shells::deserialize(deserializer);
        assert_eq!(result.as_ref().ok(), expected.as_ref(), "{result:?}");
    }
}
