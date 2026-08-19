/// Effectively-global application state, constructed once in `main` and threaded explicitly.
///
/// Runtime-free: safe to construct before the tokio runtime exists.
pub struct AppCtx {}

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
