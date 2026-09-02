use std::io::IsTerminal;

use atuin_common::logs::{FileConfig, LogConfig, StderrConfig};
use tracing::Level;
use tracing_appender::rolling::{self, RollingFileAppender, Rotation};
use tracing_subscriber::filter::{self, EnvFilter};
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::util::TryInitError;

mod otel;
use otel::OtelCtx;
pub use otel::OtelCtxEnableError;

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
/// OpenTelemetry traces can be enabled by passing an `ATUIN_OTEL=<url>` environment variable,
/// which will publish spans to the given endpoint, eg.
///
/// ```bash
/// ATUIN_OTEL=http://localhost:4318 atuin foo bar baz.
/// ```
///
/// OpenTelemetry support must be compiled in via the `profiling-traced` feature; without it, setting
/// `ATUIN_OTEL` is an error (see [`OtelCtx`]).
///
pub struct LogCtx {
    /// Handle to open-telemetry traces. Kept alive because it's RAII.
    ///
    /// OTEL may be disabled, in which case this is [`Option::None`].
    _otel: Option<OtelCtx>,
}

impl LogCtx {
    /// Try to enable the logging for atuin.
    ///
    /// TODO(markovejnovic): Clean up this [`LogConfig`] structure. It feels very out-of-place where
    /// it is.
    #[must_use = "returns an RAII guard which is required for tracing support"]
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
        let otel_layer = otel.as_ref().map(|ctx| ctx.layer(service_name));

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
            base.with(
                config.stderr.as_ref().map(|c| stderr_layer::<_, fmt::time::SystemTime>(c, filter)),
            )
            .try_init()?;
        } else {
            base.with(config.stderr.as_ref().map(|c| stderr_layer::<_, ()>(c, filter)))
                .try_init()?;
        }

        // Non-fatal: the subscriber is live now, so this reaches stderr/otel if configured.
        if let Some(e) = file_error {
            tracing::warn!("failed to initialize the log file: {e}");
        }

        Ok(Self { _otel: otel })
    }
}

/// [`tracing_subscriber::Layer`] is generic, so we need a monomorphic helper here.
///
/// See <https://github.com/tokio-rs/tracing/issues/3180> for more details.
fn stderr_layer<S, T>(config: &StderrConfig, filter: EnvFilter) -> impl tracing_subscriber::Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    T: fmt::time::FormatTime + Default + 'static,
{
    fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(config.show_target)
        .map_event_format(|f| f.with_timer(T::default()))
        .with_filter(filter)
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
