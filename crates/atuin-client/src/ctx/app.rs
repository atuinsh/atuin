use super::workspace::WorkspaceCtx;

/// Effectively-global application state, constructed once and held by the
/// [`app()`](super::app) static. Group process-wide state here (as sub-context
/// structs) rather than scattering module statics across the crate; access it
/// through typed accessors like [`AppCtx::workspace`].
pub struct AppCtx {
    workspace: WorkspaceCtx,
}

impl AppCtx {
    pub(crate) fn new() -> Self {
        Self {
            workspace: WorkspaceCtx::new(),
        }
    }

    /// Workspace (git work-tree root) resolution and its cache.
    #[must_use]
    pub fn workspace(&self) -> &WorkspaceCtx {
        &self.workspace
    }
}
