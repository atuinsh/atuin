use std::sync::LazyLock;

use super::workspace::WorkspaceCtx;
use atuin_domain::{AtuinHostname, AtuinUsername};

/// Effectively-global application state, constructed once and held by the [`app()`](super::app)
/// static.
pub struct AppCtx {
    /// Constructed lazily since some subcommands of atuin don't need workspace-specific
    /// information, so we can save the some cycles.
    workspace: LazyLock<WorkspaceCtx>,
}

impl AppCtx {
    pub(crate) fn new() -> Self {
        Self {
            workspace: LazyLock::new(WorkspaceCtx::new),
        }
    }

    /// A workspace is the current working directory that atuin is invoked in.
    #[must_use]
    pub fn workspace(&self) -> &WorkspaceCtx {
        &self.workspace
    }

    /// The current session id, as exported by the shell integration in `ATUIN_SESSION`.
    ///
    /// [`None`] when the variable is unset (e.g. atuin invoked outside a hooked shell). Probed
    /// live, as the value is fixed for the life of a process but set by the environment.
    #[must_use]
    pub fn session(&self) -> Option<String> {
        std::env::var("ATUIN_SESSION").ok()
    }

    /// The atuin-registered active hostname.
    ///
    /// Note that this always returns a new owned object as there is no way of knowing whether the
    /// hostname has changed or not at any given point.
    ///
    /// TODO(markovejnovic): A future implementation could have a refresh background task that
    ///                      refreshes the value periodically, avoiding an allocation.
    #[must_use]
    pub fn hostname(&self) -> AtuinHostname {
        AtuinHostname::probe()
    }

    /// The atuin-registered active username.
    ///
    /// Note that this always returns a new owned object as there is no way of knowing whether the
    /// hostname has changed or not at any given point.
    ///
    /// TODO(markovejnovic): A future implementation could have a refresh background task that
    ///                      refreshes the value periodically, avoiding an allocation.
    #[must_use]
    pub fn username(&self) -> AtuinUsername {
        AtuinUsername::probe()
    }
}
