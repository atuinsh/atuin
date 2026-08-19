use atuin_common::sync::EagerFutureCell;

use super::{GitRepo, NewGitRepoCtxError};
use crate::ctx::workspace::WorkspaceCtx;

/// The git context for a workspace.
///
/// Git discovery is expensive (filesystem I/O), so it runs eagerly in the background from
/// construction and is awaited on demand via [`Self::repo_ctx`].
pub struct GitCtx {
    cell: EagerFutureCell<Result<Option<GitRepo>, NewGitRepoCtxError>>,
}

impl GitCtx {
    /// Kick off git discovery for `workspace`'s cwd in the background.
    ///
    /// Requires a live tokio runtime (the eager cell spawns onto [`tokio::runtime::Handle::current`]).
    pub fn new(workspace: &WorkspaceCtx) -> Self {
        let discover_from = workspace.cwd().as_ref().to_path_buf();
        Self {
            // Git discovery is blocking filesystem I/O, so run it on the blocking pool to keep it
            // off the async executor.
            cell: EagerFutureCell::new(
                async move {
                    tokio::task::spawn_blocking(move || GitRepo::discover(&discover_from))
                        .await
                        .expect("git discovery task panicked")
                },
                &tokio::runtime::Handle::current(),
            ),
        }
    }

    /// Grab a handle to the active git repo.
    ///
    /// Returns `Ok(None)` if the cwd is not a git repo, or `Err` if discovery failed.
    pub async fn repo_ctx(&self) -> Result<Option<&GitRepo>, &NewGitRepoCtxError> {
        self.cell.get().await.as_ref().map(Option::as_ref)
    }
}
