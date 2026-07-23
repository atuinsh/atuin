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
    /// The returned list is suitable for passing to [`Database::search`] via
    /// [`OptFilters::shells`].
    ///
    /// Instead of returning a [`Vec`], this method returns a helper type that can be viewed as a
    /// slice without allocating, or turned into a [`Vec`].
    ///
    /// [`Database::search`]: crate::database::Database::search
    /// [`OptFilters::shells`]: crate::database::OptFilters::shells
    pub fn to_list(&self) -> ShellList<'_> {
        self.to_list_with(|| std::env::var("ATUIN_SHELL").ok())
    }

    /// Like [`Self::to_list`], but takes the current shell as a parameter.
    pub fn to_list_with<F>(&self, current_shell: F) -> ShellList<'_>
    where
        F: FnOnce() -> Option<String>,
    {
        let inner = match self {
            Self::All => ShellListInner::Reference(&[]),
            Self::Auto => match current_shell() {
                // Show results from the current shell, plus entries that have no shell recorded.
                Some(shell) => ShellListInner::Inline([shell, "".into()]),
                // Show all results if no shell is detected.
                None => ShellListInner::Reference(&[]),
            },
            Self::List(shells) => ShellListInner::Reference(shells),
        };
        ShellList(inner)
    }
}

/// Helper type for [`ShellList`] to avoid exposing the enum variants directly.
enum ShellListInner<'a> {
    Inline([String; 2]),
    Reference(&'a [String]),
}

/// Helper type that allows [`Shells`] to be viewed as a slice without allocating.
///
/// Returned by [`Shells::to_list`].
pub struct ShellList<'a>(ShellListInner<'a>);

impl ShellList<'_> {
    pub fn as_slice(&self) -> &[String] {
        match &self.0 {
            ShellListInner::Inline(array) => array.as_slice(),
            ShellListInner::Reference(slice) => slice,
        }
    }

    pub fn to_vec(&self) -> Vec<String> {
        self.as_slice().into()
    }
}

impl AsRef<[String]> for ShellList<'_> {
    fn as_ref(&self) -> &[String] {
        self.as_slice()
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

    #[rstest]
    #[case::all_bash(Shells::All, Some("bash"), &[])]
    #[case::all_none(Shells::All, None, &[])]
    #[case::auto_bash(Shells::Auto, Some("bash"), &["bash", ""])]
    #[case::auto_none(Shells::Auto, None, &[])]
    #[case::list_bash_zsh(Shells::List(vec!["bash".into()]), Some("zsh"), &["bash"])]
    #[case::list_bash_unknown_zsh(
        Shells::List(["bash", ""].map(str::to_owned).into()),
        Some("zsh"),
        &["bash", ""],
    )]
    #[case::list_bash_zsh_none(
        Shells::List(["bash", "zsh"].map(str::to_owned).into()),
        None,
        &["bash", "zsh"],
    )]
    #[case::list_empty_bash(Shells::List(vec![]), Some("bash"), &[])]
    fn to_list(
        #[case] settings: Shells,
        #[case] current_shell: Option<&str>,
        #[case] expected: &[&str],
    ) {
        let list = settings.to_list_with(|| current_shell.map(Into::into));
        let slice = list.as_slice();
        assert!(slice.iter().eq(expected), "{slice:?} != {expected:?}");
    }
}
