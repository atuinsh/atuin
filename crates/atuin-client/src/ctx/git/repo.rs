use std::path::Path;

use super::NewGitRepoCtxError;

/// A handle to a particular git repo: the repo for the current working directory, plus the
/// main worktree's repo when the cwd is a linked worktree.
///
/// Stored as [`gix::ThreadSafeRepository`] so this handle can live in a `Sync` context (a
/// thread-local [`gix::Repository`] is not `Sync`). Obtain a usable thread-local repository on
/// demand via [`gix::ThreadSafeRepository::to_thread_local`].
#[derive(Debug, Clone)]
pub struct GitRepo {
    /// The repository initialized for the given absolute path.
    repo: gix::ThreadSafeRepository,

    /// The main worktree's repository.
    ///
    /// [`None`] when it is the same as [`Self::repo`] (i.e. `repo` is the main worktree, or a repo
    /// with no linked worktrees), which avoids re-opening it. [`Some`] only for a linked worktree.
    main_repo: Option<gix::ThreadSafeRepository>,
}

impl GitRepo {
    /// Discover the git repo containing `path`.
    ///
    /// Returns `Ok(None)` when `path` is not inside a git repository (a normal state, not an error).
    pub fn discover(path: &Path) -> Result<Option<Self>, NewGitRepoCtxError> {
        let repo = match gix::discover(path) {
            Ok(repo) => repo,
            Err(gix::discover::Error::Discover(
                gix::discover::upwards::Error::NoGitRepository { .. }
                | gix::discover::upwards::Error::NoGitRepositoryWithinCeiling { .. }
                | gix::discover::upwards::Error::NoGitRepositoryWithinFs { .. },
            )) => return Ok(None),
            Err(e) => return Err(NewGitRepoCtxError::DiscoverGitRepo(Box::new(e))),
        };

        let main_repo = if repo.git_dir() == repo.common_dir() {
            None
        } else {
            Some(
                repo.main_repo()
                    .map_err(|e| NewGitRepoCtxError::OpenMainRepo(Box::new(e)))?
                    .into_sync(),
            )
        };

        Ok(Some(Self { repo: repo.into_sync(), main_repo }))
    }

    /// The repository for the current working directory.
    pub fn repo(&self) -> &gix::ThreadSafeRepository {
        &self.repo
    }

    /// The main repo's repository (the worktree's main repo, or the cwd repo when not in a linked
    /// worktree).
    pub fn main_repo(&self) -> &gix::ThreadSafeRepository {
        self.main_repo.as_ref().unwrap_or(&self.repo)
    }
}
