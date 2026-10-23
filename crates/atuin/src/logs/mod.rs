//! Logging for every atuin process: one logfmt file, and warnings on stderr.
//!
//! Each command picks a [`LogMode`]. Events go to `~/.atuin/logs/atuin.<date>.log`, one line per
//! event starting with `process=daemon|cli pid=<n>`, filtered by `ATUIN_LOG` (`EnvFilter`
//! directives, `warn,atuin=info` when unset). Warnings and errors also print on stderr, unless the
//! process owns the terminal.

use std::io::IsTerminal;
use std::time::SystemTime;

use atuin_common::logs::{LogMode, USER};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::prelude::*;
use tracing_subscriber::util::TryInitError;
use tracing_subscriber::{EnvFilter, Registry};

mod file;
mod filter;
mod format;
mod otel;

use file::LogFile;
use filter::FileFilter;
use format::ProcessTagged;
use otel::OtelCtx;
pub use otel::OtelCtxEnableError;

use crate::i18n::fl;

#[derive(Debug, thiserror::Error)]
pub enum LogCtxEnableError {
    #[error("{}", fl!("log-otel-collector-failed", error = .0.to_string()))]
    OtelCtx(#[from] OtelCtxEnableError),
    #[error("{}", fl!("log-subscriber-init-failed", error = .0.to_string()))]
    Subscriber(#[from] TryInitError),
}

/// Which kind of process wrote a log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, derive_more::Display)]
enum Process {
    #[display("daemon")]
    Daemon,
    #[display("cli")]
    Cli,
}

impl From<LogMode> for Process {
    fn from(mode: LogMode) -> Self {
        match mode {
            LogMode::Daemon { .. } => Self::Daemon,
            LogMode::Stderr | LogMode::FileAndStderr | LogMode::File => Self::Cli,
        }
    }
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
    /// Try to enable the logging for atuin, as `mode` asks.
    ///
    /// The log file's filter comes from the `ATUIN_LOG` environment variable. A log file that
    /// can't be opened is not an error: logging goes on without it and says so on stderr.
    #[must_use = "returns an RAII guard which is required for tracing support"]
    pub fn try_enable(
        service_name: &'static str,
        mode: LogMode,
    ) -> Result<Self, LogCtxEnableError> {
        // Only a process that writes the file reads ATUIN_LOG. The others run at shell startup and
        // on hooks, where a warning about it would print every time.
        let FileFilter { filter, warnings } = if mode.has_file() {
            FileFilter::from_env()
        } else {
            FileFilter {
                filter: EnvFilter::default(),
                warnings: Vec::new(),
            }
        };

        let file =
            (mode.has_file() && filter.max_level_hint() != Some(LevelFilter::OFF)).then(|| {
                let dir = atuin_common::dirs::logs_dir();
                let log_file = LogFile::new(dir.clone());
                log_file.prune(SystemTime::now());
                log_file.appender().map_err(|e| (dir, e))
            });
        let (file, file_error) = match file.transpose() {
            Ok(file) => (file, None),
            Err(e) => (None, Some(e)),
        };

        let otel = OtelCtx::try_enable(service_name)?;
        let mut layers =
            Self::layers(mode, &filter, file, std::io::stderr, std::io::stderr().is_terminal());
        if let Some(otel) = &otel {
            layers.push(otel.layer(service_name).boxed());
        }
        tracing_subscriber::registry().with(layers).try_init()?;

        // Only now is there a subscriber to report these to.
        for warning in warnings {
            tracing::warn!(target: USER, "{warning}");
        }
        if let Some((dir, e)) = file_error {
            tracing::warn!(
                target: USER,
                "{}",
                fl!("log-file-failed", dir = dir.display().to_string(), error = e.to_string())
            );
        }

        Ok(Self { _otel: otel })
    }

    /// The layers `mode` asks for, each with its own filter: the log file, warnings on `stderr`,
    /// and the daemon's `--show-logs` copy of every event.
    fn layers<F, E>(
        mode: LogMode,
        filter: &EnvFilter,
        file: Option<F>,
        stderr: E,
        ansi: bool,
    ) -> Vec<Box<dyn tracing_subscriber::Layer<Registry> + Send + Sync>>
    where
        F: for<'w> MakeWriter<'w> + Send + Sync + 'static,
        E: for<'w> MakeWriter<'w> + Clone + Send + Sync + 'static,
    {
        let process = Process::from(mode);
        let mut layers = Vec::new();

        if mode.has_file()
            && let Some(file) = file
        {
            layers.push(
                tracing_logfmt::builder()
                    .layer()
                    .map_event_format(|inner| ProcessTagged::new(process, inner))
                    .with_ansi(false)
                    .with_writer(file)
                    .with_filter(filter.clone())
                    .boxed(),
            );
        }

        if mode.has_stderr() {
            layers.push(
                fmt::layer()
                    .without_time()
                    .with_target(false)
                    .with_writer(stderr.clone())
                    .with_ansi(ansi)
                    .with_filter(LevelFilter::WARN)
                    .boxed(),
            );
        }

        if mode == (LogMode::Daemon { show_logs: true }) {
            layers.push(
                fmt::layer()
                    .with_writer(stderr)
                    .with_ansi(ansi)
                    .with_filter(filter.clone())
                    .boxed(),
            );
        }

        layers
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;

    use atuin_common::logs::LogMode;
    use parking_lot::Mutex;
    use rstest::rstest;
    use tracing::dispatcher::{self, Dispatch};
    use tracing_subscriber::Registry;
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::prelude::*;

    use super::LogCtx;
    use super::filter::FileFilter;

    /// An in-memory writer to assert on what a layer wrote.
    #[derive(Clone, Default)]
    pub(in crate::logs) struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        pub(in crate::logs) fn text(&self) -> String {
            String::from_utf8(self.0.lock().clone()).expect("logs are utf-8")
        }
    }

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl MakeWriter<'_> for Capture {
        type Writer = Self;

        fn make_writer(&self) -> Self::Writer {
            self.clone()
        }
    }

    /// The layers of one mode, with the file and stderr captured.
    struct Logged {
        file: Capture,
        stderr: Capture,
        dispatch: Dispatch,
    }

    impl Logged {
        fn new(mode: LogMode) -> Self {
            let file = Capture::default();
            let stderr = Capture::default();
            let filter = FileFilter::parse(None, LevelFilter::TRACE).filter;
            let layers = LogCtx::layers(mode, &filter, Some(file.clone()), stderr.clone(), false);
            Self {
                file,
                stderr,
                dispatch: Dispatch::new(Registry::default().with(layers)),
            }
        }

        fn emit(&self, events: impl FnOnce()) {
            dispatcher::with_default(&self.dispatch, events);
        }
    }

    #[rstest]
    #[case::file_and_stderr(LogMode::FileAndStderr, true, true, false)]
    #[case::file(LogMode::File, true, false, false)]
    #[case::stderr(LogMode::Stderr, false, true, false)]
    #[case::daemon(LogMode::Daemon { show_logs: false }, true, false, false)]
    #[case::daemon_show_logs(LogMode::Daemon { show_logs: true }, true, true, true)]
    fn routes_by_mode(
        #[case] mode: LogMode,
        #[case] in_file: bool,
        #[case] warning_on_stderr: bool,
        #[case] info_on_stderr: bool,
    ) {
        let logged = Logged::new(mode);

        logged.emit(|| {
            tracing::warn!(target: "atuin_client::sync", "worrying");
            tracing::info!(target: "atuin_client::sync", "chatter");
        });

        let file = logged.file.text();
        let stderr = logged.stderr.text();
        assert_eq!(file.contains("worrying") && file.contains("chatter"), in_file, "file: {file}");
        assert_eq!(
            stderr.matches("worrying").count(),
            usize::from(warning_on_stderr),
            "stderr: {stderr}"
        );
        assert_eq!(stderr.contains("chatter"), info_on_stderr, "stderr: {stderr}");
    }

    #[rstest]
    #[case::cli(LogMode::FileAndStderr, "cli")]
    #[case::daemon(LogMode::Daemon { show_logs: false }, "daemon")]
    fn file_lines_are_tagged_from_any_thread(#[case] mode: LogMode, #[case] process: &str) {
        let logged = Logged::new(mode);

        logged.emit(|| tracing::warn!(target: "atuin_client::sync", "from the test thread"));
        let dispatch = logged.dispatch.clone();
        std::thread::spawn(move || {
            dispatcher::with_default(&dispatch, || {
                tracing::warn!(target: "atuin_client::sync", "from another thread");
            });
        })
        .join()
        .expect("the logging thread finishes");

        let file = logged.file.text();
        let prefix = format!("process={process} pid={} ", std::process::id());
        let lines: Vec<&str> = file.lines().collect();
        assert_eq!(lines.len(), 2, "{file}");
        assert!(lines.iter().all(|line| line.starts_with(&prefix)), "{file}");
    }

    #[rstest]
    fn multi_line_message_is_one_file_line() {
        let logged = Logged::new(LogMode::File);

        logged.emit(|| tracing::warn!(target: "atuin_client::sync", "first\nsecond"));

        let file = logged.file.text();
        assert_eq!(file.lines().count(), 1, "{file}");
    }

    #[rstest]
    fn default_filter_drops_third_party_info() {
        let logged = Logged::new(LogMode::File);

        logged.emit(|| {
            tracing::info!(target: "lsm_tree::tree", "noisy");
            tracing::warn!(target: "lsm_tree::tree", "worrying");
            tracing::info!(target: "atuin_daemon::daemon", "ours");
        });

        let file = logged.file.text();
        assert!(!file.contains("noisy"), "{file}");
        assert!(file.contains("worrying"), "{file}");
        assert!(file.contains("ours"), "{file}");
    }
}
