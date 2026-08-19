use std::sync::LazyLock;

use super::workspace::WorkspaceCtx;

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
}
