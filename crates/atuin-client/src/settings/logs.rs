//! Settings for Atuin's log files.

// `JsonSchema`'s derive serializes each field's default through its `AsDuration`, in code that the
// `#[allow(deprecated)]` on a struct does not reach.
#![allow(deprecated)]

use std::path::PathBuf;
use std::time::Duration;

use atuin_common::logs::LogLevel;
use atuin_common::path::{AbsPathBuf, RelativeTo, ShellExpanded};
use atuin_common::time::{AsDuration, Days, DurationSchema};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::ConfigDir;

/// Configuration for one kind of log, such as the daemon's.
#[serde_as]
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct LogConfig {
    /// The log file. A relative path is relative to `logs.dir`.
    #[serde_as(as = "ShellExpanded")]
    pub file: PathBuf,

    /// Write this log. If unset, `logs.enabled`.
    pub enabled: Option<bool>,

    /// The least severe level this log records. If unset, `logs.level`.
    pub level: Option<LogLevel>,

    /// How long to keep this log, such as `4d`. If unset, `logs.retention`.
    #[serde_as(as = "Option<AsDuration<Days>>")]
    #[schemars(with = "Option<DurationSchema<Days>>")]
    pub retention: Option<Duration>,
}

impl LogConfig {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            ..Default::default()
        }
    }
}

#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct Logs {
    /// Write log files.
    #[serde(default = "Logs::default_enabled")]
    pub enabled: bool,

    /// The directory that log files go in.
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub dir: AbsPathBuf,

    /// The least severe level that logs record. The `ATUIN_LOG` environment variable overrides it.
    #[serde(default)]
    pub level: LogLevel,

    /// How long to keep each log file, such as `4d`. A bare number counts days. `0` keeps none.
    #[serde(default = "Logs::default_retention")]
    #[serde_as(as = "AsDuration<Days>")]
    #[schemars(with = "DurationSchema<Days>")]
    pub retention: Duration,

    /// The log of interactive search.
    #[serde(default)]
    pub search: LogConfig,

    /// The daemon's log.
    #[serde(default)]
    pub daemon: LogConfig,

    /// Atuin AI's log.
    #[serde(default)]
    pub ai: LogConfig,
}

impl Logs {
    fn default_enabled() -> bool {
        true
    }

    fn default_retention() -> Duration {
        Duration::from_secs(4 * 86_400)
    }
}
