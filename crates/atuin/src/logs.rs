use std::io::IsTerminal;

use atuin_common::logs::{FileConfig, LogConfig, StderrConfig};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig as _;
use tracing::Level;
use tracing_appender::rolling::{self, RollingFileAppender, Rotation};
use tracing_subscriber::filter::{self, EnvFilter, LevelFilter};
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

/// Held for the lifetime of a command. When `ATUIN_OTEL` is set it owns the
/// OpenTelemetry tracer provider; dropping it flushes and shuts down the OTLP
/// exporter, so callers must keep it alive until the command has finished.
#[must_use = "dropping the guard immediately flushes and shuts down the OTLP exporter"]
pub struct LogGuard {
    _otel: Option<OtelGuard>,
}

impl LogGuard {
    /// A guard that owns nothing, for when logging was not initialized.
    pub(crate) fn disabled() -> Self {
        Self { _otel: None }
    }
}

/// Owns the OpenTelemetry tracer provider for a command's lifetime. Dropping it
/// flushes any batched spans and stops the exporter's background thread. The
/// default exporter is blocking HTTP driven from a dedicated thread, so both
/// export and this shutdown need no tokio runtime -- it is safe to drop from
/// anywhere, including outside `block_on`.
struct OtelGuard(opentelemetry_sdk::trace::SdkTracerProvider);

impl Drop for OtelGuard {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

/// Build an OTLP tracer provider when `ATUIN_OTEL` is set. Returns `None` when
/// disabled, or if the exporter cannot be built.
///
/// The env var's value selects the collector's OTLP/HTTP endpoint:
/// - a URL (e.g. `http://localhost:4318`) is used as the endpoint; the standard
///   `/v1/traces` path is appended if absent. Note this is the OTLP *ingest* port
///   (4318), NOT the Jaeger UI (16686).
/// - anything else (e.g. `1`) just enables export to the default,
///   `http://localhost:4318/v1/traces`.
///
/// Uses the blocking HTTP exporter and the default dedicated-thread batch
/// processor, so it requires no async runtime and is safe to construct here
/// (which runs before the tokio runtime is entered).
fn build_otel_provider() -> Option<opentelemetry_sdk::trace::SdkTracerProvider> {
    // Unset (or non-utf8) env var -> OTel disabled.
    let value = std::env::var("ATUIN_OTEL").ok()?;

    let mut builder = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(opentelemetry_otlp::Protocol::HttpBinary);

    let endpoint = value.trim();
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        builder = builder.with_endpoint(otlp_traces_endpoint(endpoint));
    }

    let exporter = builder
        .build()
        .map_err(|e| eprintln!("atuin: failed to initialize OTLP exporter: {e}"))
        .ok()?;

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(opentelemetry_sdk::Resource::builder().with_service_name("atuin").build())
        .build();

    Some(provider)
}

/// The OTLP/HTTP traces endpoint for a base URL. `SpanExporter::with_endpoint`
/// uses its argument verbatim (it does not append the OTLP signal path), so add
/// the standard `/v1/traces` suffix when the caller supplied only a base.
fn otlp_traces_endpoint(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1/traces") {
        base.to_owned()
    } else {
        format!("{base}/v1/traces")
    }
}

pub fn init_logging(config: &LogConfig) -> LogGuard {
    // We have to dispatch the time config statically; see
    // https://github.com/tokio-rs/tracing/issues/3180
    match &config.stderr {
        Some(StderrConfig {
            show_time: false, ..
        }) => with_stderr_time::<()>(config),
        _ => with_stderr_time::<fmt::time::SystemTime>(config),
    }
}

fn get_base_filter(config: &LogConfig) -> EnvFilter {
    let level = config.file.as_ref().map_or(Level::WARN, |f| f.level.to_tracing());
    EnvFilter::default().add_directive(level.into())
}

fn clean_up_old_logs(config: &FileConfig) {
    let Some(cutoff) = config
        .retention_days
        .checked_mul(24 * 60 * 60)
        .and_then(|s| std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(s)))
    else {
        return;
    };

    let Ok(entries) = std::fs::read_dir(config.directory()) else {
        return;
    };

    let Some(prefix) = config.name().to_str() else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // Match files like "search.log.2024-02-23" or "daemon.log.2024-02-23"
        if !name.starts_with(prefix) || name == prefix {
            continue;
        }

        if let Ok(metadata) = entry.metadata()
            && let Ok(modified) = metadata.modified()
            && modified < cutoff
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum FileWriterError {
    #[error("log file name must be utf-8")]
    NonUtf8Filename,
    #[error("{0}")]
    RollingFileAppender(#[from] rolling::InitError),
}

fn make_file_writer(config: &FileConfig) -> Result<RollingFileAppender, FileWriterError> {
    let prefix = config.name().to_str().ok_or(FileWriterError::NonUtf8Filename)?;
    let writer = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .build(config.directory())?;
    Ok(writer)
}

fn with_stderr_time<StderrTime>(config: &LogConfig) -> LogGuard
where
    StderrTime: fmt::time::FormatTime + Default + Send + Sync + 'static,
{
    // ATUIN_LOG env var overrides config file level settings
    let filter: EnvFilter = std::env::var("ATUIN_LOG")
        .map_or_else(|_| get_base_filter(config), |s| filter::Builder::default().parse_lossy(s))
        .add_directive("sqlx_sqlite::regexp=off".parse().unwrap());

    if let Some(file) = &config.file {
        clean_up_old_logs(file);
    }

    let file_layer = config.file.as_ref().map(|file| {
        let writer = make_file_writer(file)?;
        let layer = fmt::layer().with_writer(writer).with_ansi(false).with_filter(filter.clone());
        Ok::<_, FileWriterError>(layer)
    });

    let stderr_layer = config.stderr.as_ref().map(|stderr| {
        fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(std::io::stderr().is_terminal())
            .with_target(stderr.show_target)
            .map_event_format(|f| f.with_timer(StderrTime::default()))
            .with_filter(filter)
    });

    let (file_layer, file_error) = match file_layer.transpose() {
        Ok(layer) => (layer, None),
        Err(e) => (None, Some(e)),
    };
    let has_stderr_layer = stderr_layer.is_some();

    // OpenTelemetry -> OTLP -> Jaeger. `ATUIN_OTEL` exports the `#[instrument]`
    // spans as an async-native waterfall (view at http://localhost:16686). Built
    // inline so the layer's subscriber type is inferred at the registry site.
    // Requires the `#[instrument(level = "trace")]` spans to be compiled in, i.e.
    // a debug build or the `profiling-traced` profile.
    let (otel_layer, otel_guard) = build_otel_provider().map_or((None, None), |provider| {
        let layer = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("atuin"))
            // Parent spans by the `tracing` span tree (the `#[instrument]` nesting)
            // rather than the OpenTelemetry thread-local context, which nothing here
            // populates -- with the default (`true`) every span becomes its own root
            // trace instead of nesting under the command span.
            .with_context_activation(false)
            .with_filter(LevelFilter::TRACE);
        (Some(layer), Some(OtelGuard(provider)))
    });

    if let Err(e) = tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .with(otel_layer)
        .try_init()
    {
        if has_stderr_layer || cfg!(debug_assertions) {
            eprintln!("failed to initialize logging: {e}");
        }
        return LogGuard::disabled();
    }

    if let Some(e) = file_error {
        if has_stderr_layer {
            tracing::warn!("failed to initialize log file: {e}");
        } else if cfg!(debug_assertions) {
            eprintln!("failed to initialize log file: {e}");
        }
    }

    LogGuard { _otel: otel_guard }
}
