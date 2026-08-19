mod ctx;
mod repo;

use thiserror::Error;

pub use ctx::GitCtx;
pub use repo::GitRepo;

#[derive(Debug, Error)]
pub enum NewGitRepoCtxError {
    #[error("failed to probe the git repo: {0}")]
    DiscoverGitRepo(Box<gix::discover::Error>),

    #[error("failed to open the main worktree's git repo: {0}")]
    OpenMainRepo(Box<gix::open::Error>),
}
