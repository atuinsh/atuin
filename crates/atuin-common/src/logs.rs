use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Target for messages meant for the user, as opposed to diagnostics.
///
/// No module path can start with a hyphenated name, so target prefix matching never mixes it up
/// with a module's events.
pub const USER: &str = "atuin-user";

/// Log level for file logging. Maps to tracing's LevelFilter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LogLevel {
    #[must_use]
    pub fn to_tracing(&self) -> tracing::Level {
        use tracing::Level;
        match self {
            Self::Trace => Level::TRACE,
            Self::Debug => Level::DEBUG,
            Self::Info => Level::INFO,
            Self::Warn => Level::WARN,
            Self::Error => Level::ERROR,
        }
    }
}

#[derive(Debug)]
pub struct FileConfig {
    pub path: PathBuf,
    pub level: LogLevel,
    pub retention: std::time::Duration,
}

impl FileConfig {
    #[must_use]
    pub fn directory(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new(""))
    }

    #[must_use]
    pub fn name(&self) -> &OsStr {
        self.path.file_name().unwrap_or_else(|| OsStr::new(""))
    }
}

#[derive(Debug, Default)]
pub struct StderrConfig {
    pub show_time: bool,
    pub show_target: bool,
}

impl StderrConfig {
    #[must_use]
    pub fn verbose() -> Self {
        Self {
            show_time: true,
            show_target: true,
        }
    }
}

#[derive(Debug)]
pub struct LogConfig {
    pub file: Option<FileConfig>,
    pub stderr: Option<StderrConfig>,
}

impl LogConfig {
    #[must_use]
    pub fn file_only(file: FileConfig) -> Self {
        Self {
            file: Some(file),
            stderr: None,
        }
    }

    #[must_use]
    pub fn stderr_only() -> Self {
        Self {
            file: None,
            stderr: Some(StderrConfig::default()),
        }
    }
}
