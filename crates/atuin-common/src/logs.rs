use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

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
    pub fn to_tracing(&self) -> tracing::Level {
        use tracing::Level;
        match self {
            LogLevel::Trace => Level::TRACE,
            LogLevel::Debug => Level::DEBUG,
            LogLevel::Info => Level::INFO,
            LogLevel::Warn => Level::WARN,
            LogLevel::Error => Level::ERROR,
        }
    }
}

#[derive(Debug)]
pub struct FileConfig {
    pub path: PathBuf,
    pub level: LogLevel,
    pub retention_days: u64,
}

impl FileConfig {
    pub fn directory(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new(""))
    }

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
    pub fn file_only(file: FileConfig) -> Self {
        Self {
            file: Some(file),
            stderr: None,
        }
    }

    pub fn stderr_only() -> Self {
        Self {
            file: None,
            stderr: Some(StderrConfig::default()),
        }
    }
}
