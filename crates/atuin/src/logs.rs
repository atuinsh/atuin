use std::env::VarError;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::str::FromStr;

use atuin_common::logs::{FileConfig, LogConfig, StderrConfig};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{ExporterBuildError, WithExportConfig as _};
use thiserror;
use tracing::Level;
use tracing_appender::rolling::{self, RollingFileAppender, Rotation};
use tracing_subscriber::filter::{self, EnvFilter, LevelFilter};
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::util::TryInitError;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum LogCtxEnableError {
    #[error("failed to build the otel collector: {0}")]
    OtelCtx(#[from] OtelCtxEnableError),
    #[error("failed to initialize subscriber: {0}")]
    Subscriber(#[from] TryInitError),
}

/// Application-level log context.
///
/// RAII object which must be kept alive for as long as you want to do logs through [`tracing`].
///
/// ## OpenTelemetry
///
/// [`opentelemetry`] traces can be enabled by passing an `ATUIN_OTEL=<url>` environment variable,
/// which will publish spans to the given endpoint, eg.
///
/// ```bash
/// ATUIN_OTEL=http://localhost:4318 atuin foo bar baz.
/// ```
///
pub struct LogCtx {
    /// Handle to open-telemetry traces. Kept alive because it's RAII.
    ///
    /// OTEL may be disabled, in which case this is [`Option::None`].
    otel: Option<OtelCtx>,
}

impl LogCtx {
    /// Check whether open-telemetry is enabled.
    #[must_use]
    pub const fn otel_enabled(&self) -> bool {
        self.otel.is_some()
    }

    /// Try to enable the logging for atuin.
    ///
    /// TODO(markovejnovic): Clean up this [`LogConfig`] structure. It feels very out-of-place where
    /// it is.
    #[must_use]
    pub fn try_enable(
        service_name: &'static str,
        config: &LogConfig,
    ) -> Result<Self, LogCtxEnableError> {
        // ATUIN_LOG env var overrides config file level settings
        let filter: EnvFilter = std::env::var("ATUIN_LOG")
            .map_or_else(|_| get_base_filter(config), |s| filter::Builder::default().parse_lossy(s))
            .add_directive("sqlx_sqlite::regexp=off".parse().unwrap());

        if let Some(file_config) = &config.file {
            clean_up_old_logs(file_config);
        }

        // A misconfigured log file is non-fatal: drop the file layer and warn once
        // the subscriber is up, so logging still works via stderr / otel.
        let file_layer = config.file.as_ref().map(|file| {
            let writer = make_file_writer(file)?;
            Ok::<_, FileWriterError>(
                fmt::layer().with_writer(writer).with_ansi(false).with_filter(filter.clone()),
            )
        });
        let (file_layer, file_error) = match file_layer.transpose() {
            Ok(layer) => (layer, None),
            Err(e) => (None, Some(e)),
        };

        let otel = OtelCtx::try_enable(service_name)?;

        let otel_layer = otel.as_ref().map(|ctx| {
            tracing_opentelemetry::layer()
                .with_tracer(ctx.provider.tracer(service_name))
                .with_context_activation(false)
                .with_filter(LevelFilter::TRACE)
        });

        // The stderr layer is added last because its type varies with the timer
        // dispatch below; the file and otel layers must be layered first so `base`
        // has a single type shared by both branches (otherwise the otel layer's
        // subscriber type `S` can't unify across the two stderr layer types).
        let base = tracing_subscriber::registry().with(file_layer).with(otel_layer);
        let show_time = matches!(
            config.stderr,
            Some(StderrConfig {
                show_time: true,
                ..
            })
        );
        if show_time {
            base.with(stderr_layer::<_, fmt::time::SystemTime>(config.stderr.as_ref(), filter))
                .try_init()?;
        } else {
            base.with(stderr_layer::<_, ()>(config.stderr.as_ref(), filter)).try_init()?;
        }

        // Non-fatal: the subscriber is live now, so this reaches stderr/otel if configured.
        if let Some(e) = file_error {
            tracing::warn!("failed to initialize the log file: {e}");
        }

        Ok(Self { otel })
    }
}

/// Build the stderr `fmt` layer, if stderr logging is configured.
///
/// The timer type `T` is a generic rather than a runtime value because
/// [`fmt::format::Format::with_timer`] bakes the timer into the event-formatter type;
/// `()` (no timestamp) and [`fmt::time::SystemTime`] are therefore distinct types.
/// Callers dispatch `T` so only this one axis is monomorphized instead of duplicating
/// the whole subscriber build. `S` is inferred at the `.with(...)` call site.
fn stderr_layer<S, T>(
    config: Option<&StderrConfig>,
    filter: EnvFilter,
) -> Option<impl tracing_subscriber::Layer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    T: fmt::time::FormatTime + Default + 'static,
{
    config.map(|cfg| {
        fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(std::io::stderr().is_terminal())
            .with_target(cfg.show_target)
            .map_event_format(|f| f.with_timer(T::default()))
            .with_filter(filter)
    })
}

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

struct OtelCtx {
    provider: opentelemetry_sdk::trace::SdkTracerProvider,
}

impl OtelCtx {
    /// Try to enable the opentelemetry logging context, if it was requested through the
    fn try_enable(service_name: &'static str) -> Result<Option<Self>, OtelCtxEnableError> {
        // TODO(markovejnovic): We should really have our own env-var parsing logic to avoid this
        // annoying error handling here.
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
}

impl Drop for OtelCtx {
    fn drop(&mut self) {
        if let Err(err) = self.provider.force_flush() {
            // Intentional eprintln! here since we cannot safely use error!
            eprintln!("Unexpected error flushing OTEL spans: {}", err);
        }

        if let Err(err) = self.provider.shutdown() {
            // Intentional eprintln! here since we cannot safely use error!
            eprintln!("Unexpected error shutting down the OTEL tracc provider: {}", err);
        }
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
