/// A path to an item in a TOML document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct TomlPath<'a>(pub Vec<&'a str>);

impl<'a> TomlPath<'a> {
    /// The path to the document's root table.
    pub const fn root() -> Self {
        Self(Vec::new())
    }

    /// Parse a dot-separated path, like `ai.sessions.resume`.
    ///
    /// This uses the same parsing logic as [`super::set_deep_key`], in that it does not handle
    /// quoted keys like `a."b.c".d`.
    pub fn parse_dotted_simple(dotted: &'a str) -> Self {
        Self(dotted.split('.').collect())
    }

    /// Append `name` to this path.
    pub fn child(&self, name: &'a str) -> Self {
        let mut components = Vec::with_capacity(self.0.len() + 1);
        components.extend(&self.0);
        components.push(name);
        Self(components)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn joined<'a>(names: &[&'a str]) -> TomlPath<'a> {
        names.iter().fold(TomlPath::root(), |path, name| path.child(name))
    }

    #[rstest]
    #[case::a_key("search_mode", &["search_mode"])]
    #[case::a_nested_key("ai.sessions.resume", &["ai", "sessions", "resume"])]
    fn parse(#[case] dotted: &str, #[case] expected: &[&str]) {
        assert_eq!(TomlPath::parse_dotted_simple(dotted), joined(expected));
    }

    #[rstest]
    fn the_root_has_no_components() {
        assert!(TomlPath::root().0.is_empty());
    }

    #[rstest]
    #[case::a_name_with_a_dot(&["a.b", "c"])]
    #[case::an_empty_name(&["", "c"])]
    fn join_takes_any_name(#[case] names: &[&str]) {
        assert_eq!(joined(names).0, names);
    }

    proptest! {
        /// Parsing names joined with dots gives back the names, as long as none has a dot.
        #[test]
        fn parse_dotted_names(names in prop::collection::vec("[^.]*", 1..5)) {
            let dotted = names.join(".");
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            prop_assert_eq!(TomlPath::parse_dotted_simple(&dotted), joined(&names));
        }
    }
}
