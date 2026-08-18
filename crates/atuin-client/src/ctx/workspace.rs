use std::fmt;
use std::path::{Path, PathBuf};

use atuin_common::sync::EagerFutureCell;

use crate::ctx::GitRepoCtx;
use crate::ctx::git_ctx::NewGitRepoCtxError;

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
        let logical = std::env::var_os("PWD")
            .map(PathBuf::from)
            .unwrap_or_else(|| absolute.clone());
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
/// A workspace is a directory in which `atuin` is invoked. This takes on two meanings in code due
/// to the daemon, non-daemon path.
pub struct WorkspaceCtx {
    cwd: Cwd,

    /// The git context.
    ///
    /// Git discovery is expensive (filesystem I/O), so it runs eagerly in the background from
    /// construction and is awaited on demand via [`Self::git_ctx`].
    git_ctx: EagerFutureCell<Result<Option<GitRepoCtx>, NewGitRepoCtxError>>,
}

impl WorkspaceCtx {
    /// Create a new workspace context, kicking off git discovery in the background.
    ///
    /// Panics if the current working directory cannot be determined; see [`Cwd::resolve`].
    // Not `Default`: this constructor reads the cwd, spawns background work, and can panic — none
    // of which fits `Default`'s cheap-and-infallible contract.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let cwd = Cwd::resolve();

        let discover_from = cwd.as_ref().to_path_buf();
        Self {
            // Git discovery is blocking filesystem I/O, so run it on the blocking pool to keep it
            // off the async executor.
            git_ctx: EagerFutureCell::new(
                async move {
                    tokio::task::spawn_blocking(move || GitRepoCtx::new(&discover_from))
                        .await
                        .expect("git discovery task panicked")
                },
                &tokio::runtime::Handle::current(),
            ),
            cwd,
        }
    }

    /// The directory atuin was invoked in. See [`Cwd`].
    pub fn cwd(&self) -> &Cwd {
        &self.cwd
    }

    /// Grab a handle to the active git repo.
    ///
    /// Returns `Ok(Option::None)` if the cwd is not a git repo.
    /// Returns `Err(NewGitRepoCtxError)` if there was an error querying the git context.
    pub async fn git_ctx(&self) -> Result<Option<&GitRepoCtx>, &NewGitRepoCtxError> {
        self.git_ctx.get().await.as_ref().map(Option::as_ref)
    }
}
