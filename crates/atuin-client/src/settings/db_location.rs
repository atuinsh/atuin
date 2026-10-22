use std::ffi::OsStr;
use std::path::{Display, Path, PathBuf};

use atuin_common::db::SqliteDbUrl;
use atuin_common::path::{AbsPath, AbsPathBuf, PathAnchor, RelativeTo, ShellExpanded};
use serde::{Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

/// Where a SQLite database setting points: a file, or a connection string SQLite reads itself,
/// `:memory:` or a `sqlite:` or `file:` URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbLocation {
    File(AbsPathBuf),
    /// Opened as written, so a relative file in it is relative to the working directory.
    Url(SqliteDbUrl),
}

impl DbLocation {
    /// The file this names by path, if it isn't a URL.
    #[must_use]
    pub fn file(&self) -> Option<&AbsPath> {
        match self {
            Self::File(path) => Some(path),
            Self::Url(_) => None,
        }
    }

    /// The path or URL, as SQLite opens it.
    #[must_use]
    pub fn as_os_str(&self) -> &OsStr {
        match self {
            Self::File(path) => path.as_os_str(),
            Self::Url(url) => OsStr::new(url.as_str()),
        }
    }

    /// Equivalent to [`Path::display`], for the path or URL.
    #[must_use]
    pub fn display(&self) -> Display<'_> {
        Path::new(self.as_os_str()).display()
    }
}

impl AsRef<OsStr> for DbLocation {
    fn as_ref(&self) -> &OsStr {
        self.as_os_str()
    }
}

/// Resolves a file like any path setting, and takes `:memory:` or a URI as written.
impl<'de, A: PathAnchor> DeserializeAs<'de, DbLocation> for RelativeTo<A> {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<DbLocation, D::Error> {
        let path: PathBuf = ShellExpanded::deserialize_as(deserializer)?;
        match path.to_str() {
            Some(url)
                if url == ":memory:" || url.starts_with("sqlite:") || url.starts_with("file:") =>
            {
                Ok(DbLocation::Url(SqliteDbUrl(url.to_owned())))
            }
            _ => Self::resolve(path).map(DbLocation::File),
        }
    }
}

impl<A> SerializeAs<DbLocation> for RelativeTo<A> {
    fn serialize_as<S: Serializer>(source: &DbLocation, serializer: S) -> Result<S::Ok, S::Error> {
        match source {
            DbLocation::File(path) => path.serialize(serializer),
            DbLocation::Url(url) => serializer.serialize_str(url.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use atuin_common::env::MockEnv;
    use rstest::rstest;
    use serde_json::Value;
    use serde_with::As;

    use super::*;

    struct Anchor;

    impl PathAnchor for Anchor {
        fn dir() -> io::Result<AbsPathBuf> {
            AbsPathBuf::from_current_dir("anchor")
        }
    }

    fn file(relative: &str) -> DbLocation {
        DbLocation::File(Anchor::dir().unwrap().resolve(relative).unwrap())
    }

    fn url(url: &str) -> DbLocation {
        DbLocation::Url(SqliteDbUrl(url.to_owned()))
    }

    #[rstest]
    #[case::relative("history.db", file("history.db"))]
    #[case::unset_variable("$UNSET/history.db", file("$UNSET/history.db"))]
    #[case::memory(":memory:", url(":memory:"))]
    #[case::sqlite_uri("sqlite:///tmp/history.db", url("sqlite:///tmp/history.db"))]
    #[case::file_uri("file:history.db?mode=memory", url("file:history.db?mode=memory"))]
    #[case::variable_holding_a_uri("$DB", url("sqlite:history.db"))]
    fn db_location_deserializes(#[case] raw: &str, #[case] expected: DbLocation) {
        let env = MockEnv::install();
        env.set("DB", "sqlite:history.db");

        let location: DbLocation = As::<RelativeTo<Anchor>>::deserialize(Value::from(raw)).unwrap();
        assert_eq!(location, expected);
    }

    #[rstest]
    fn an_absolute_db_file_is_kept() {
        let absolute = AbsPathBuf::from_current_dir("history.db").unwrap();
        let raw = Value::from(absolute.to_str().unwrap());

        let location: DbLocation = As::<RelativeTo<Anchor>>::deserialize(raw).unwrap();
        assert_eq!(location, DbLocation::File(absolute));
    }
}
