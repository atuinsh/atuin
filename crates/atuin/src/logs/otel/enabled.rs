//! The real OpenTelemetry exporter, compiled in with the `profiling-traced` feature.

use std::env::VarError;
use std::ffi::OsString;
use std::str::FromStr;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{ExporterBuildError, WithExportConfig as _};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::registry::LookupSpan;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum OtelCtxEnableError {
    #[error("the given ATUIN_OTEL URL does not appear to be a utf-8 string")]
    NonUtf8EnvVar(OsString),
    #[error("the given ATUIN_OTEL failed to parse as URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("the given ATUIN_OTEL URL does not appear to be an HTTP(s) URL")]
    NonHttpUrl,
    #[error("failed to construct the exporter: {0}")]
    ExporterBuild(#[from] ExporterBuildError),
}

pub struct OtelCtx {
    provider: opentelemetry_sdk::trace::SdkTracerProvider,
}

impl OtelCtx {
    /// Try to enable the opentelemetry logging context, if it was requested through the
    /// `ATUIN_OTEL` environment variable.
    pub fn try_enable(service_name: &'static str) -> Result<Option<Self>, OtelCtxEnableError> {
        // TODO(markovejnovic): We should really have our own env-var parsing logic to avoid
        // this annoying error handling here.
        let otel_env: Option<String> = match std::env::var("ATUIN_OTEL") {
            Ok(v) => Some(v),
            Err(e) => match e {
                VarError::NotPresent => None,
                VarError::NotUnicode(e) => {
                    return Err(OtelCtxEnableError::NonUtf8EnvVar(e));
                }
            },
        };

        let otel_env: String = match otel_env {
            Some(var) => var,
            None => {
                return Ok(None);
            }
        };

        // TODO(markovejnovic): A better env library could also handle this.
        // TODO(markovejnovic): I want an HttpUrl type so bad...
        let mut otel_url = Url::from_str(&otel_env)?;
        if !(otel_url.scheme() == "http" || otel_url.scheme() == "https") {
            return Err(OtelCtxEnableError::NonHttpUrl);
        }

        if !otel_url.path().ends_with("/v1/traces") {
            otel_url
                .path_segments_mut()
                .expect("checked above that it's an http url")
                .extend(["v1", "traces"]);
        }

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .with_endpoint(otel_url.as_str())
            .build()?;

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(
                opentelemetry_sdk::Resource::builder().with_service_name(service_name).build(),
            )
            .build();

        Ok(Some(Self { provider }))
    }

    /// The [`tracing_opentelemetry`] layer that exports spans through this context.
    pub fn layer<S>(&self, service_name: &'static str) -> impl Layer<S> + Send + Sync
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
    {
        tracing_opentelemetry::layer()
            .with_tracer(self.provider.tracer(service_name))
            .with_context_activation(false)
            .with_filter(LevelFilter::TRACE)
    }
}

impl Drop for OtelCtx {
    fn drop(&mut self) {
        if let Err(err) = self.provider.force_flush() {
            // Intentional eprintln! here since we cannot safely use error!
            eprintln!("Unexpected error flushing OTEL spans: {err}");
        }

        if let Err(err) = self.provider.shutdown() {
            // Intentional eprintln! here since we cannot safely use error!
            eprintln!("Unexpected error shutting down the OTEL tracc provider: {err}");
        }
    }
}
