//! OpenTelemetry span export, gated behind the `profiling-traced` feature.
//!
//! Two symmetric `OtelCtx` implementations -- `enabled` (the real exporter) and `disabled` (a
//! stub that errors if `ATUIN_OTEL` is set) -- keep the parent module free of `cfg`. Both expose
//! `try_enable` and a `layer` returning `Box<dyn Layer<..>>`, so the signatures are identical.

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
