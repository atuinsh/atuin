//! Application context, constructed in `main` and threaded explicitly.
//!
//!   - [`AppCtx`] holds effectively-global state (session).
//!   - [`WorkspaceCtx`] (built via [`WorkspaceCtx::new`] from an [`AppCtx`]) holds data coupled to
//!     the workspace — the working directory atuin was invoked in.
//!   - [`GitCtx`] (built via [`GitCtx::new`] from a [`WorkspaceCtx`]) holds the git context of the
//!     workspace, if any.

pub mod app;
mod git;
pub mod workspace;

pub use app::AppCtx;
pub use git::{GitCtx, GitRepo, NewGitRepoCtxError};
pub use workspace::{Cwd, WorkspaceCtx};
