use atuin_common::logs::{FileConfig, LogConfig, StderrConfig};
use std::fs::OpenOptions;
use std::io::IsTerminal;
use tracing::Level;
use tracing_appender::rolling::{self, RollingFileAppender, Rotation};
use tracing_subscriber::filter::{self, EnvFilter, LevelFilter};
use tracing_subscriber::fmt;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::prelude::*;

pub fn init_logging(config: &LogConfig) {
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
    let level = config
        .file
        .as_ref()
        .map_or(Level::WARN, |f| f.level.to_tracing());
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
    let prefix = config
        .name()
        .to_str()
        .ok_or(FileWriterError::NonUtf8Filename)?;
    let writer = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .build(config.directory())?;
    Ok(writer)
}

fn with_stderr_time<StderrTime>(config: &LogConfig)
where
    StderrTime: fmt::time::FormatTime + Default + Send + Sync + 'static,
{
    // ATUIN_LOG env var overrides config file level settings
    let filter: EnvFilter = std::env::var("ATUIN_LOG")
        .map_or_else(
            |_| get_base_filter(config),
            |s| filter::Builder::default().parse_lossy(s),
        )
        .add_directive("sqlx_sqlite::regexp=off".parse().unwrap());

    if let Some(file) = &config.file {
        clean_up_old_logs(file);
    }

    let file_layer = config.file.as_ref().map(|file| {
        let writer = make_file_writer(file)?;
        let layer = fmt::layer()
            .with_writer(writer)
            .with_ansi(false)
            .with_filter(filter.clone());
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

    let span_layer = std::env::var("ATUIN_SPAN").ok().and_then(|value| {
        let path = if value.is_empty() {
            "atuin-spans.json".to_owned()
        } else {
            value
        };
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .ok()?;
        let layer = fmt::layer()
            .json()
            .with_writer(file)
            .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
            .with_filter(LevelFilter::TRACE);
        Some(layer)
    });

    let (file_layer, file_error) = match file_layer.transpose() {
        Ok(layer) => (layer, None),
        Err(e) => (None, Some(e)),
    };
    let has_stderr_layer = stderr_layer.is_some();

    if let Err(e) = tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .with(span_layer)
        .try_init()
    {
        if has_stderr_layer || cfg!(debug_assertions) {
            eprintln!("failed to initialize logging: {e}");
        }
        return;
    }

    if let Some(e) = file_error {
        if has_stderr_layer {
            tracing::warn!("failed to initialize log file: {e}");
        } else if cfg!(debug_assertions) {
            eprintln!("failed to initialize log file: {e}");
        }
    }
}
