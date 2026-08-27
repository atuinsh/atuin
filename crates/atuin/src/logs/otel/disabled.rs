//! Stub used when the `profiling-traced` feature is off: OpenTelemetry is not compiled in.

use tracing_subscriber::Layer;
use tracing_subscriber::registry::LookupSpan;

#[derive(Debug, thiserror::Error)]
pub enum OtelCtxEnableError {
    #[error(
        "this build of atuin has no OpenTelemetry support: rebuild with the `profiling-traced` \
         feature (e.g. `cargo build-traced`) to export `ATUIN_OTEL` traces"
    )]
    NotCompiled,
}

pub enum OtelCtx {}

impl OtelCtx {
    /// Refuse to enable OpenTelemetry if it was requested, since it was compiled out.
    pub fn try_enable(_service_name: &'static str) -> Result<Option<Self>, OtelCtxEnableError> {
        if std::env::var_os("ATUIN_OTEL").is_some() {
            return Err(OtelCtxEnableError::NotCompiled);
        }
        Ok(None)
    }

    #[allow(clippy::unused_self)]
    pub fn layer<S>(&self, _service_name: &'static str) -> impl Layer<S> + Send + Sync
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
    {
        // Normally not reachable, but we can't call unreachable! here since the type signature
        // requires something that `impl`'s `Layer`, we fake it.
        None::<tracing_subscriber::fmt::Layer<S>>
    }
}
