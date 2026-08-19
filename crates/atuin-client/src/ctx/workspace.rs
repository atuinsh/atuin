use std::fmt;
use std::path::{Path, PathBuf};

use crate::ctx::app::AppCtx;

/// The working directory atuin was invoked in.
///
/// [`Display`](fmt::Display) renders the *logical* path as reported by `$PWD` — the form that
/// preserves the symlinks the user navigated through, and the one atuin records and shows. The
/// path view ([`AsRef<Path>`]) yields the *absolute*, symlink-resolved path for filesystem work
/// such as git discovery.
#[derive(Debug, Clone)]
pub struct Cwd {
    /// Logical path from `$PWD`, or the absolute path when `$PWD` is unset.
    logical: PathBuf,
    /// Absolute (symlink-resolved) path from [`std::env::current_dir`].
    absolute: PathBuf,
}

impl Cwd {
    /// Resolve the current working directory.
    ///
    /// Panics if the physical cwd cannot be determined (e.g. it was deleted out from under the
    /// process) — atuin cannot meaningfully run from a directory it cannot resolve.
    #[must_use]
    pub fn resolve() -> Self {
        let absolute =
            std::env::current_dir().expect("failed to determine the current working directory");
        let logical =
            std::env::var_os("PWD").map(PathBuf::from).unwrap_or_else(|| absolute.clone());
        Self { logical, absolute }
    }
}

impl fmt::Display for Cwd {
    /// The logical (`$PWD`) path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.logical.display().fmt(f)
    }
}

impl AsRef<Path> for Cwd {
    /// The absolute, symlink-resolved path.
    fn as_ref(&self) -> &Path {
        &self.absolute
    }
}

/// Stores information on the current active workspace.
///
/// A workspace is a directory in which `atuin` is invoked.
pub struct WorkspaceCtx {
    cwd: Cwd,
}

impl WorkspaceCtx {
    /// Create a new workspace context.
    ///
    /// Panics if the current working directory cannot be determined; see [`Cwd::resolve`].
    pub fn new(_app: &AppCtx) -> Self {
        Self { cwd: Cwd::resolve() }
    }

    /// The directory atuin was invoked in. See [`Cwd`].
    pub fn cwd(&self) -> &Cwd {
        &self.cwd
    }
}
