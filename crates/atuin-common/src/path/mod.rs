//! Filesystem path utilities and extension traits.

pub mod display_rich;

use std::path::{Path, PathBuf};

pub use display_rich::{DisplayRichExt, RichDisplay};

/// Utility extensions for paths in atuin.
pub trait PathExt {
    /// Check whether the given path is a symlink, and a dangling one at that.
    fn is_dangling_symlink(&self) -> bool;
}

impl<P: AsRef<Path>> PathExt for P {
    fn is_dangling_symlink(&self) -> bool {
        let path: &Path = self.as_ref();
        path.is_symlink() && !path.exists()
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
    pub primary: PathBuf,

    /// The path that would be used if none of the relevant environment variables were set.
    ///
    /// For example, `/tmp/example.txt` even when `$TMPDIR` is set.
    pub envless: Option<PathBuf>,
}
