/// Effectively-global application state, constructed once in `main` and threaded explicitly.
///
/// Runtime-free: safe to construct before the tokio runtime exists.
pub struct AppCtx {}

// `AppCtx` is reconstructed in place at a couple of call sites (atuin-ai `mcp.rs`,
// `tui/app.rs`) on the assumption that it carries no construction-time state. If that
// stops being true, those sites would silently discard state — so pin it to zero-sized
// here and revisit them (e.g. derive `Clone` and clone instead) if this trips.
const _: () = assert!(
    std::mem::size_of::<AppCtx>() == 0,
    "AppCtx is no longer zero-sized; the in-place `AppCtx::new()` reconstructions in \
     atuin-ai (mcp.rs, tui/app.rs) may now silently drop state — clone instead."
);

impl AppCtx {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }

    /// The current session id, as exported by the shell integration in `ATUIN_SESSION`.
    ///
    /// [`None`] when the variable is unset (e.g. atuin invoked outside a hooked shell).
    #[must_use]
    pub fn session(&self) -> Option<String> {
        std::env::var("ATUIN_SESSION").ok()
    }
}

impl Default for AppCtx {
    fn default() -> Self {
        Self::new()
    }
}
