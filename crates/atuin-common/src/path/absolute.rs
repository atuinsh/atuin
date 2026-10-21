//! [`AbsPath`] and [`AbsPathBuf`]: [`Path`] and [`PathBuf`], checked to be absolute.

use std::borrow::{Borrow, Cow};
use std::cmp::Ordering;
use std::ffi::OsString;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ref_cast::{RefCastCustom, ref_cast_custom};
use serde::{Deserialize, Deserializer, Serialize, de};

/// A [`Path`] that [is absolute](Path::is_absolute), so it names the same file whatever the
/// working directory.
#[derive(
    RefCastCustom,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    derive_more::AsRef,
    derive_more::Debug,
    derive_more::Deref,
)]
#[as_ref(forward)]
#[debug("{_0:?}")]
#[serde(transparent)]
#[repr(transparent)]
pub struct AbsPath(Path);

/// An owned [`AbsPath`], like [`PathBuf`].
#[derive(
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    derive_more::AsRef,
    derive_more::Debug,
    derive_more::Into,
)]
#[as_ref(forward)]
#[debug("{_0:?}")]
#[into(PathBuf, OsString)]
#[serde(transparent)]
pub struct AbsPathBuf(PathBuf);

/// A path that had to be absolute but isn't.
#[derive(Debug, thiserror::Error)]
#[error("{0:?} is not an absolute path")]
pub struct NotAbsoluteError(PathBuf);

impl NotAbsoluteError {
    /// The path, handed back.
    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl AbsPath {
    #[ref_cast_custom]
    const fn new_unchecked(path: &Path) -> &Self;

    /// `path`, if it is absolute.
    pub fn new<P: AsRef<Path> + ?Sized>(path: &P) -> Option<&Self> {
        let path = path.as_ref();
        path.is_absolute().then(|| Self::new_unchecked(path))
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// The parent directory, which is absolute too; [`None`] for a root.
    pub fn parent(&self) -> Option<&Self> {
        self.0.parent().map(Self::new_unchecked)
    }

    /// `path` if it is absolute, otherwise `path` under `self`.
    ///
    /// Fails only on Windows, for a drive-relative path like `C:foo`, which [`Path::join`] returns
    /// unchanged.
    pub fn resolve(&self, path: impl AsRef<Path>) -> Result<AbsPathBuf, NotAbsoluteError> {
        AbsPathBuf::new(self.0.join(path))
    }
}

impl AbsPathBuf {
    /// `path`, if it is absolute; the error hands it back.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, NotAbsoluteError> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(NotAbsoluteError(path));
        }
        Ok(Self(path))
    }

    /// `path` made absolute against the working directory by [`std::path::absolute`], which keeps
    /// `..` and leaves symlinks unresolved.
    #[expect(clippy::disallowed_methods, reason = "this is the wrapper the lint references")]
    pub fn from_current_dir(path: impl AsRef<Path>) -> io::Result<Self> {
        std::path::absolute(path).map(Self)
    }

    /// `path` with symlinks and `..` resolved by [`std::fs::canonicalize`]; it must exist.
    #[expect(clippy::disallowed_methods, reason = "this is the wrapper the lint references")]
    pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<Self> {
        std::fs::canonicalize(path).map(Self)
    }

    /// [`Self::canonicalize`], off the async runtime's thread.
    #[expect(clippy::disallowed_methods, reason = "this is the wrapper the lint references")]
    pub async fn canonicalize_async(path: impl AsRef<Path>) -> io::Result<Self> {
        tokio::fs::canonicalize(path).await.map(Self)
    }

    #[must_use]
    pub fn as_abs_path(&self) -> &AbsPath {
        AbsPath::new_unchecked(&self.0)
    }

    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl Deref for AbsPathBuf {
    type Target = AbsPath;

    fn deref(&self) -> &AbsPath {
        self.as_abs_path()
    }
}

impl ToOwned for AbsPath {
    type Owned = AbsPathBuf;

    fn to_owned(&self) -> AbsPathBuf {
        AbsPathBuf(self.0.to_path_buf())
    }
}

impl Borrow<AbsPath> for AbsPathBuf {
    fn borrow(&self) -> &AbsPath {
        self.as_abs_path()
    }
}

impl Borrow<Path> for AbsPathBuf {
    fn borrow(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Self> for AbsPath {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl AsRef<AbsPath> for AbsPathBuf {
    fn as_ref(&self) -> &AbsPath {
        self.as_abs_path()
    }
}

impl From<&AbsPath> for AbsPathBuf {
    fn from(path: &AbsPath) -> Self {
        path.to_owned()
    }
}

impl From<Cow<'_, AbsPath>> for AbsPathBuf {
    fn from(path: Cow<'_, AbsPath>) -> Self {
        path.into_owned()
    }
}

impl<'a> From<&'a AbsPath> for Cow<'a, AbsPath> {
    fn from(path: &'a AbsPath) -> Self {
        Cow::Borrowed(path)
    }
}

impl<'a> From<&'a AbsPathBuf> for Cow<'a, AbsPath> {
    fn from(path: &'a AbsPathBuf) -> Self {
        Cow::Borrowed(path.as_abs_path())
    }
}

impl From<AbsPathBuf> for Cow<'_, AbsPath> {
    fn from(path: AbsPathBuf) -> Self {
        Cow::Owned(path)
    }
}

impl<'a> TryFrom<&'a Path> for &'a AbsPath {
    type Error = NotAbsoluteError;

    fn try_from(path: &'a Path) -> Result<Self, NotAbsoluteError> {
        AbsPath::new(path).ok_or_else(|| NotAbsoluteError(path.to_path_buf()))
    }
}

impl TryFrom<PathBuf> for AbsPathBuf {
    type Error = NotAbsoluteError;

    fn try_from(path: PathBuf) -> Result<Self, NotAbsoluteError> {
        Self::new(path)
    }
}

impl FromStr for AbsPathBuf {
    type Err = NotAbsoluteError;

    fn from_str(path: &str) -> Result<Self, NotAbsoluteError> {
        Self::new(path)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for &'a AbsPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <&Path>::deserialize(deserializer)?.try_into().map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for AbsPathBuf {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(PathBuf::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// Compares two path types as [`Path`]s, both ways round, like std does for [`PathBuf`] and
/// [`Path`].
macro_rules! impl_cmp {
    (<$($life:lifetime),*> $lhs:ty, $rhs:ty) => {
        impl<$($life),*> PartialEq<$rhs> for $lhs {
            fn eq(&self, other: &$rhs) -> bool {
                AsRef::<Path>::as_ref(self) == AsRef::<Path>::as_ref(other)
            }
        }

        impl<$($life),*> PartialEq<$lhs> for $rhs {
            fn eq(&self, other: &$lhs) -> bool {
                AsRef::<Path>::as_ref(self) == AsRef::<Path>::as_ref(other)
            }
        }

        impl<$($life),*> PartialOrd<$rhs> for $lhs {
            fn partial_cmp(&self, other: &$rhs) -> Option<Ordering> {
                AsRef::<Path>::as_ref(self).partial_cmp(AsRef::<Path>::as_ref(other))
            }
        }

        impl<$($life),*> PartialOrd<$lhs> for $rhs {
            fn partial_cmp(&self, other: &$lhs) -> Option<Ordering> {
                AsRef::<Path>::as_ref(self).partial_cmp(AsRef::<Path>::as_ref(other))
            }
        }
    };
}

impl_cmp!(<> AbsPathBuf, AbsPath);
impl_cmp!(<'a> AbsPathBuf, &'a AbsPath);
impl_cmp!(<> AbsPathBuf, Path);
impl_cmp!(<> AbsPathBuf, PathBuf);
impl_cmp!(<'a> AbsPathBuf, &'a Path);
impl_cmp!(<> AbsPath, Path);
impl_cmp!(<> AbsPath, PathBuf);
impl_cmp!(<'a> &'a AbsPath, PathBuf);

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    #[cfg(unix)]
    use proptest::prelude::*;
    use rstest::rstest;
    use serde_json::Value;

    use super::*;

    #[rstest]
    #[cfg_attr(unix, case::root("/", true))]
    #[cfg_attr(unix, case::absolute("/a/b", true))]
    #[cfg_attr(unix, case::tilde("~/a", false))]
    #[cfg_attr(windows, case::drive(r"C:\a", true))]
    #[cfg_attr(windows, case::unc(r"\\server\share\a", true))]
    #[cfg_attr(windows, case::rooted_without_a_drive(r"\a", false))]
    #[cfg_attr(windows, case::drive_relative("C:a", false))]
    #[case::relative("a/b", false)]
    #[case::dot("./a", false)]
    #[case::empty("", false)]
    fn new_accepts_only_absolute_paths(#[case] raw: &str, #[case] absolute: bool) {
        assert_eq!(AbsPath::new(raw).map(AbsPath::as_path), absolute.then_some(Path::new(raw)));
        let owned = AbsPathBuf::new(raw).map_err(NotAbsoluteError::into_path_buf);
        assert_eq!(
            owned,
            if absolute {
                Ok(AbsPathBuf(raw.into()))
            } else {
                Err(raw.into())
            }
        );
    }

    #[rstest]
    #[cfg_attr(unix, case::relative("/base", "a/b", Some("/base/a/b")))]
    #[cfg_attr(unix, case::absolute("/base", "/a", Some("/a")))]
    #[cfg_attr(unix, case::parent_is_kept("/base", "../a", Some("/base/../a")))]
    #[cfg_attr(windows, case::relative(r"C:\base", "a", Some(r"C:\base\a")))]
    #[cfg_attr(windows, case::absolute(r"C:\base", r"D:\a", Some(r"D:\a")))]
    #[cfg_attr(windows, case::rooted_keeps_the_drive(r"C:\base", r"\a", Some(r"C:\a")))]
    #[cfg_attr(windows, case::drive_relative(r"C:\base", "D:a", None))]
    fn resolve_puts_relative_paths_under_the_base(
        #[case] base: &str,
        #[case] path: &str,
        #[case] expected: Option<&str>,
    ) {
        let resolved = AbsPath::new(base).unwrap().resolve(path).ok();
        assert_eq!(resolved.as_deref().map(AbsPath::as_path), expected.map(Path::new));
    }

    #[rstest]
    #[case::accepted(if cfg!(windows) { r"C:\a" } else { "/a" }, Ok(()))]
    #[case::rejected("a", Err("\"a\" is not an absolute path"))]
    fn parses_and_round_trips_through_serde_only_when_absolute(
        #[case] raw: &str,
        #[case] expected: Result<(), &str>,
    ) {
        let parsed = raw.parse::<AbsPathBuf>().map_err(|err| err.to_string());
        let deserialized =
            serde_json::from_value::<AbsPathBuf>(Value::from(raw)).map_err(|err| err.to_string());
        assert_eq!(parsed, deserialized);
        match expected {
            Ok(()) => assert_eq!(serde_json::to_value(deserialized.unwrap()).unwrap(), raw),
            Err(message) => assert_eq!(deserialized.unwrap_err(), message),
        }
    }

    /// Both forms compare, hash and print as the path they hold, so either finds the other in a map.
    #[cfg(unix)]
    #[rstest]
    fn compares_and_hashes_as_a_path() {
        let owned = AbsPathBuf::new("/a/b").unwrap();
        let borrowed = AbsPath::new("/a//b").unwrap();
        assert_eq!(owned, borrowed);
        assert_eq!(owned, Path::new("/a/b"));
        assert_eq!(PathBuf::from("/a/b"), owned);
        assert_eq!(format!("{owned:?}"), format!("{:?}", Path::new("/a/b")));

        let map = HashMap::from([(owned, ())]);
        assert!(map.contains_key(borrowed));
        assert!(map.contains_key(Path::new("/a/b")));
    }

    #[cfg(unix)]
    proptest! {
        /// `parent` keeps the guarantee without checking it, and `resolve` never fails on Unix.
        #[test]
        fn derived_paths_stay_absolute(base in "/.*", path in ".*") {
            let base = AbsPath::new(&base).unwrap();
            prop_assert!(base.parent().is_none_or(|parent| parent.is_absolute()));
            prop_assert!(base.resolve(&path).is_ok(), "{base:?} and {path:?}");
        }
    }
}
