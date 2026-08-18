//! Process-wide application context.
//!
//! The following useful modules exist:
//!
//!   - [`app()`] returns an [`AppCtx`] which generally holds application's effectively-global
//!     state.
//!   - [`AppCtx::workspace`] then offers [`WorkspaceCtx`] which contains data coupled to the
//!     workspace. The workspace is defined as the working directory a user invokes Atuin in.
//!   - [`WorkspaceCtx::git_ctx`] gives you [`GitRepoCtx`] which is the context of the git repo (if
//!     any), associated with the workspace the user is working in.

mod app;
mod git_ctx;
mod workspace;

pub use app::AppCtx;
pub use git_ctx::GitRepoCtx;
pub use workspace::{Cwd, WorkspaceCtx};

use std::sync::LazyLock;

static APP: LazyLock<AppCtx> = LazyLock::new(AppCtx::new);

/// The process-wide [`AppCtx`].
#[must_use]
pub fn app() -> &'static AppCtx {
    &APP
}
