use atuin_common::filter::{self, OrFilter};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Controls which shells' commands are included in interactive search.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Shells {
    #[default]
    /// Include commands run from the current shell, or commands that have no recorded shell.
    ///
    /// If the current shell cannot be detected or is blank, all commands will be shown.
    Auto,

    /// Include commands run by any shell in the filter. The empty string will include commands
    /// that have no recorded shell.
    Fixed(OrFilter<Vec<String>>),
}

impl Shells {
    /// Include commands from every shell.
    #[must_use]
    pub const fn all() -> Self {
        Self::Fixed(OrFilter::all())
    }

    /// Turn this setting into a concrete shell filter.
    ///
    /// This method returns a helper type that allows you to obtain a [`OrFilter`] without
    /// allocating; see [`ShellFilter::as_filter`].
    #[must_use]
    pub fn to_filter(&self) -> ShellFilter<'_> {
        self.to_filter_with(|| std::env::var("ATUIN_SHELL").ok())
    }

    /// Like [`Self::to_filter`], but takes the current shell as a parameter.
    pub fn to_filter_with<F>(&self, current_shell: F) -> ShellFilter<'_>
    where
        F: FnOnce() -> Option<String>,
    {
        ShellFilter(match self {
            Self::Auto => match current_shell() {
                Some(shell) if !shell.is_empty() => {
                    // This array upholds the "sorted and deduped" invariant: `shell` is not empty,
                    // and an empty string always compares earlier.
                    ShellFilterInner::Inline([String::new(), shell])
                }
                _ => ShellFilterInner::Borrowed(OrFilter::all()),
            },
            Self::Fixed(filter) => ShellFilterInner::Borrowed(filter.as_slice_filter()),
        })
    }
}

/// A concrete shell filter, returned by [`Shells::to_filter`].
///
/// This is a helper type to allow you to obtain a [`OrFilter`] from a [`Shells`] object
/// without allocating. See [`ShellFilter::as_filter`].
pub struct ShellFilter<'a>(ShellFilterInner<'a>);

/// Helper type to hide enum variants from the public API.
enum ShellFilterInner<'a> {
    Borrowed(OrFilter<&'a [String]>),
    /// Always sorted and deduped.
    Inline([String; 2]),
}

impl ShellFilter<'_> {
    /// View this filter as a [`OrFilter`].
    #[must_use]
    pub fn as_filter(&self) -> OrFilter<&[String]> {
        match &self.0 {
            ShellFilterInner::Borrowed(filter) => *filter,
            ShellFilterInner::Inline(items) => OrFilter::new_unchecked(items),
        }
    }

    /// Convert this filter into an owned [`OrFilter<Vec<String>>`].
    #[must_use]
    pub fn to_vec_filter(&self) -> OrFilter<Vec<String>> {
        self.as_filter().to_vec_filter()
    }
}

impl<'a> Deserialize<'a> for Shells {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum Keyword {
            All,
            Auto,
        }

        #[derive(Deserialize)]
        #[serde(untagged, expecting = r#""all", "auto", or an array of strings"#)]
        enum Repr {
            Keyword(Keyword),
            List(Vec<String>),
        }

        Ok(match Repr::deserialize(deserializer)? {
            Repr::Keyword(Keyword::All) => Self::all(),
            Repr::Keyword(Keyword::Auto) => Self::Auto,
            // Empty array is the same as "all", but "all" is preferred.
            Repr::List(shells) => Self::Fixed(OrFilter::from_list(shells).unwrap_or_default()),
        })
    }
}

impl Serialize for Shells {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Fixed(filter) => match filter.items() {
                filter::Items::All => serializer.serialize_str("all"),
                filter::Items::Some(items) => items.serialize(serializer),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::filter::{self, OrFilter};
    use rstest::rstest;
    use serde::Deserialize;

    use super::Shells;

    fn parse(toml: &str) -> Result<Shells, toml::de::Error> {
        Shells::deserialize(toml::de::ValueDeserializer::parse(toml).unwrap())
    }

    #[rstest]
    #[case::all(r#""all""#, Some(Shells::all()))]
    #[case::auto(r#""auto""#, Some(Shells::Auto))]
    #[case::array(r#"["bash", "", "zsh"]"#, Some(fixed(&["bash", "", "zsh"])))]
    #[case::array_with_duplicates(r#"["zsh", "bash", "zsh"]"#, Some(fixed(&["bash", "zsh"])))]
    #[case::empty_array(r"[]", Some(Shells::all()))]
    #[case::invalid_string(r#""hello""#, None)]
    fn deserialize(#[case] toml: &str, #[case] expected: Option<Shells>) {
        let result = parse(toml);
        assert_eq!(result.as_ref().ok(), expected.as_ref(), "{result:?}");
    }

    #[test]
    fn all_and_the_empty_array_are_the_same_value() {
        assert_eq!(parse(r#""all""#).unwrap(), parse("[]").unwrap());
        assert_eq!(parse(r#""all""#).unwrap(), Shells::all());
    }

    #[rstest]
    #[case::auto(Shells::Auto, r#""auto""#)]
    #[case::all(Shells::all(), r#""all""#)]
    #[case::empty_array(fixed(&[]), r#""all""#)]
    #[case::array(fixed(&["zsh", "bash"]), r#"["bash", "zsh"]"#)]
    fn serialize(#[case] shells: Shells, #[case] expected: &str) {
        let toml = toml::Value::try_from(&shells).unwrap().to_string();
        assert_eq!(toml, expected);
    }

    #[rstest]
    #[case::all_bash(Shells::all(), Some("bash"), &[])]
    #[case::all_none(Shells::all(), None, &[])]
    #[case::auto_bash(Shells::Auto, Some("bash"), &["", "bash"])]
    #[case::auto_none(Shells::Auto, None, &[])]
    #[case::fixed_bash_zsh(fixed(&["bash"]), Some("zsh"), &["bash"])]
    #[case::fixed_bash_unknown_zsh(fixed(&["bash", ""]), Some("zsh"), &["", "bash"])]
    #[case::fixed_bash_zsh_none(fixed(&["bash", "zsh"]), None, &["bash", "zsh"])]
    #[case::fixed_empty_bash(fixed(&[]), Some("bash"), &[])]
    #[case::fixed_empty_none(fixed(&[]), None, &[])]
    #[case::auto_empty(Shells::Auto, Some(""), &[])]
    fn to_filter(
        #[case] settings: Shells,
        #[case] current_shell: Option<&str>,
        #[case] expected: &[&str],
    ) {
        let shell_filter = settings.to_filter_with(|| current_shell.map(Into::into));
        let filter = shell_filter.as_filter();
        let items = match filter.items() {
            filter::Items::All => &[],
            filter::Items::Some(items) => items,
        };
        assert!(items.iter().eq(expected), "{items:?} != {expected:?}");
        assert_eq!(filter.is_all(), expected.is_empty());
        assert_eq!(shell_filter.to_vec_filter(), filter);
    }

    /// Helper for creating a [`Shells::Fixed`].
    fn fixed(items: &[&str]) -> Shells {
        Shells::Fixed(
            OrFilter::from_list(items.iter().copied().map(str::to_owned).collect::<Vec<_>>())
                .unwrap_or_default(),
        )
    }
}
