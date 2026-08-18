use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NewGitRepoCtxError {
    #[error("failed to probe the git repo: {0}")]
    DiscoverGitRepo(Box<gix::discover::Error>),

    #[error("failed to open the main worktree's git repo: {0}")]
    OpenMainRepo(Box<gix::open::Error>),
}

/// A context handle for a particular git repo.
///
/// The repositories are stored as [`gix::ThreadSafeRepository`] so that this handle can live in a
/// process-wide, `Sync` context (a thread-local [`gix::Repository`] is not `Sync`). Callers obtain
/// a usable thread-local [`gix::Repository`] on demand via [`Self::repo`]/[`Self::main_repo`].
#[derive(Debug, Clone)]
pub struct GitRepoCtx {
    /// The repository initialized for the given absolute path.
    repo: gix::ThreadSafeRepository,

    /// The main worktree's repository.
    ///
    /// [`None`] when it is the same as [`Self::repo`] -- i.e. `repo` is the main worktree (or a
    /// repo with no linked worktrees) -- which avoids re-opening it. [`Some`] only for a linked
    /// worktree, where it holds the distinct main repository.
    main_repo: Option<gix::ThreadSafeRepository>,
}

impl GitRepoCtx {
    pub fn new(path: &Path) -> Result<Option<Self>, NewGitRepoCtxError> {
        let repo = match gix::discover(path) {
            Ok(repo) => repo,
            // Not being inside a git repository is a normal state, not an error.
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

        Ok(Some(Self {
            repo: repo.into_sync(),
            main_repo,
        }))
    }

    /// The repository for the current working directory.
    ///
    /// Call [`gix::ThreadSafeRepository::to_thread_local`] to obtain a usable [`gix::Repository`].
    pub fn repo(&self) -> &gix::ThreadSafeRepository {
        &self.repo
    }

    /// The main repo's repository.
    ///
    /// If you are in a worktree, this returns the repository of that worktree, otherwise returns
    /// the repository of the current working directory. Call
    /// [`gix::ThreadSafeRepository::to_thread_local`] to obtain a usable [`gix::Repository`].
    pub fn main_repo(&self) -> &gix::ThreadSafeRepository {
        self.main_repo.as_ref().unwrap_or(&self.repo)
    }
}
