//! Utilities for the [`config`] crate.

use ::config::{Map, Value, ValueKind};

/// Lookups of the values inside a [`Value`] that is a table.
pub trait ValueExt {
    /// The value under `key`, if this value is a table that has `key`.
    fn get_key(&self, key: &str) -> Option<&Value>;

    /// The value at `path`, one table key per segment.
    ///
    /// The result is `None` when a segment is missing, or when a segment before the last one
    /// names a value that is not a table.
    fn get_path<'a>(&self, path: impl IntoIterator<Item = &'a str>) -> Option<&Value>;
}

impl ValueExt for Value {
    fn get_key(&self, key: &str) -> Option<&Value> {
        match &self.kind {
            ValueKind::Table(table) => table.get(key),
            _ => None,
        }
    }

    fn get_path<'a>(&self, path: impl IntoIterator<Item = &'a str>) -> Option<&Value> {
        path.into_iter().try_fold(self, |current, segment| current.get_key(segment))
    }
}

/// Lookups of the tables inside a table's map.
pub trait MapExt {
    /// The table at `path`, one table key per segment, if every segment names a table.
    ///
    /// An empty `path` is this table itself.
    fn table_at_mut<'a>(
        &mut self,
        path: impl IntoIterator<Item = &'a str>,
    ) -> Option<&mut Map<String, Value>>;
}

impl MapExt for Map<String, Value> {
    fn table_at_mut<'a>(
        &mut self,
        path: impl IntoIterator<Item = &'a str>,
    ) -> Option<&mut Map<String, Value>> {
        path.into_iter().try_fold(self, |current, segment| {
            match &mut current.get_mut(segment)?.kind {
                ValueKind::Table(table) => Some(table),
                _ => None,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use ::config::{Config, File, FileFormat};
    use rstest::{fixture, rstest};

    use super::{MapExt, Value, ValueExt};

    #[fixture]
    fn tree() -> Value {
        Config::builder()
            .add_source(File::from_str(
                "top = 1\n[a]\nleaf = \"x\"\nlist = [1]\n[a.b]\nleaf = true\n",
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .cache
    }

    #[rstest]
    #[case::the_root(&[], Some("table"))]
    #[case::a_root_key(&["top"], Some("1"))]
    #[case::a_table(&["a", "b"], Some("table"))]
    #[case::a_nested_key(&["a", "b", "leaf"], Some("true"))]
    #[case::a_missing_key(&["a", "nope"], None)]
    #[case::a_key_inside_a_value(&["top", "x"], None)]
    #[case::a_key_inside_a_list(&["a", "list", "0"], None)]
    fn get_path(tree: Value, #[case] path: &[&str], #[case] expected: Option<&str>) {
        let found = tree.get_path(path.iter().copied()).map(|value| match value.kind {
            ::config::ValueKind::Table(_) => "table".to_owned(),
            _ => value.to_string(),
        });
        assert_eq!(found.as_deref(), expected);
    }

    #[rstest]
    #[case::the_root(&[], true)]
    #[case::a_table(&["a"], true)]
    #[case::a_nested_table(&["a", "b"], true)]
    #[case::a_value(&["top"], false)]
    #[case::a_missing_key(&["nope"], false)]
    fn table_at_mut(tree: Value, #[case] path: &[&str], #[case] found: bool) {
        let mut map = tree.into_table().unwrap();
        assert_eq!(map.table_at_mut(path.iter().copied()).is_some(), found);
    }

    #[rstest]
    fn table_at_mut_edits_the_table_in_place(tree: Value) {
        let mut map = tree.into_table().unwrap();
        map.table_at_mut(["a", "b"]).unwrap().insert("new".to_owned(), Value::from(2));
        assert_eq!(
            Value::from(map).get_path(["a", "b", "new"]).map(ToString::to_string).as_deref(),
            Some("2")
        );
    }
}
