//! OpenTelemetry span export, gated behind the `profiling-traced` feature.

#[cfg(feature = "profiling-traced")]
mod enabled;
#[cfg(feature = "profiling-traced")]
pub(super) use enabled::OtelCtx;
#[cfg(feature = "profiling-traced")]
pub use enabled::OtelCtxEnableError;

#[cfg(not(feature = "profiling-traced"))]
mod disabled;
#[cfg(not(feature = "profiling-traced"))]
pub(super) use disabled::OtelCtx;
#[cfg(not(feature = "profiling-traced"))]
pub use disabled::OtelCtxEnableError;
