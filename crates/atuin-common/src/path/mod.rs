//! Filesystem path utilities and extension traits.

mod absolute;
pub mod display_rich;
mod serde_with;

use std::path::{Path, PathBuf};

pub use absolute::{AbsPath, AbsPathBuf, NotAbsoluteError};
pub use display_rich::{DisplayRichExt, RichDisplay};

// `self::`, or this reads as a re-export from the `serde_with` crate.
pub use self::serde_with::{PathAnchor, RelativeTo, ShellExpanded};

/// Utility extensions for paths in atuin.
pub trait PathExt {
    /// Check whether the given path is a symlink, and a dangling one at that.
    fn is_dangling_symlink(&self) -> bool;

    /// Check whether two paths resolve to the same path.
    ///
    /// This function checks whether the paths are the same after
    /// [canonicalizing](Path::canonicalize) them into absolute paths with intermediate components
    /// normalized and symlinks resolved. This requires both paths to exist; if they do not, this
    /// function performs a naive comparison of the path contents.
    fn is_same_path(&self, other: impl AsRef<Path>) -> bool;
}

impl<P: AsRef<Path>> PathExt for P {
    fn is_dangling_symlink(&self) -> bool {
        let path: &Path = self.as_ref();
        path.is_symlink() && !path.exists()
    }

    fn is_same_path(&self, other: impl AsRef<Path>) -> bool {
        let (this, other) = (self.as_ref(), other.as_ref());
        if this == other {
            // Fast path: avoids calling `canonicalize`.
            return true;
        }
        let (Ok(this), Ok(other)) =
            (AbsPathBuf::canonicalize(this), AbsPathBuf::canonicalize(other))
        else {
            return false;
        };
        this == other
    }
}

/// Extension trait for [`PathBuf`].
pub trait PathBufExt: Sized + PathExt {
    /// Like [`Path::join`], but reuses the [`PathBuf`].
    ///
    /// When you have a [`PathBuf`], prefer this method to [`Path::join`], unless you need to keep
    /// the original [`PathBuf`] around.
    #[must_use]
    fn joined(self, path: impl AsRef<Path>) -> Self;
}

impl PathBufExt for PathBuf {
    fn joined(mut self, path: impl AsRef<Path>) -> Self {
        self.push(path);
        self
    }
}

/// An owned path that is dependent on environment variables.
///
/// This type is used as part of a workaround to handle the case where the daemon may have been
/// spawned in an environment with `$TMPDIR` unset, but where `$TMPDIR` *is* set when the client is
/// run. This type contains *both* paths the client needs to try to connect to.
pub struct EnvDependentPathBuf {
    /// The primary form of the path.
    ///
    /// For example, `$TMPDIR/example.txt`.
    pub primary: AbsPathBuf,

    /// The path that would be used if none of the relevant environment variables were set.
    ///
    /// For example, `/tmp/example.txt` even when `$TMPDIR` is set.
    pub envless: Option<AbsPathBuf>,
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn is_same_path_compares_paths() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();

        assert!(a.is_same_path(&a));
        assert!(a.is_same_path(dir.path().join("b").joined("..").joined("a")));
        assert!(!a.is_same_path(&b));
        // Paths that don't exist are only the same if they're equal.
        assert!(dir.path().join("x").is_same_path(dir.path().join("x")));
        assert!(!dir.path().join("x").is_same_path(dir.path().join("y")));
    }

    #[cfg(unix)]
    #[rstest]
    fn is_same_path_sees_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(link.is_same_path(&target));
    }
}
