use atuin_common::units::{ByteSize, Percent};
use derive_more::From;
use regex::RegexSet;
use serde::{Deserialize, Serialize};

use super::DiskUsageLimit;

/// The `[output]` section of `config.toml`: capturing and storing command output.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(from = "OutputCaptureConfig", into = "OutputCaptureConfig")]
pub enum OutputCapture {
    /// `enabled = false`: nothing is captured and the other keys are ignored.
    #[default]
    Disabled,
    /// `enabled = true`, with the limits that govern what is kept.
    Enabled(CaptureLimits),
}

impl OutputCapture {
    /// The limits in force, or `None` when capture is disabled.
    #[must_use]
    pub const fn limits(&self) -> Option<&CaptureLimits> {
        match self {
            Self::Disabled => None,
            Self::Enabled(limits) => Some(limits),
        }
    }

    /// The limits in force, treating a disabled config as the enabled defaults.
    ///
    /// Capture is always active in this crate; the disabled case is delegated to a
    /// separate engine backend rather than branched on here.
    #[must_use]
    pub fn effective_limits(&self) -> CaptureLimits {
        self.limits().cloned().unwrap_or_default()
    }
}

/// Whose captured output is kept, how much of it, and where it goes.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureLimits {
    /// The most output kept for a single command.
    pub max_output_size: ByteSize,

    /// Whether captured output is synced.
    ///
    /// Currently, sync is not implemented.
    pub sync: bool,

    /// The most disk space captured output may use: an absolute size (`10GB`), a share of the disk
    /// holding the data directory (`10%`), or `unlimited`. Once the limit is reached the oldest
    /// output is forgotten.
    pub max_disk_usage: DiskUsageLimit,

    /// Commands whose output is never stored, though they are still recorded in history.
    pub command_filter: CommandFilter,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_output_size: ByteSize::mb(1),
            sync: false,
            max_disk_usage: DiskUsageLimit::Percent(Percent::new(10.0)),
            command_filter: CommandFilter::default(),
        }
    }
}

/// Unanchored regular expressions matched against a command line (`^cat `), like
/// `history_filter`.
#[derive(Clone, Debug, Default, From, Deserialize, Serialize)]
#[serde(transparent)]
pub struct CommandFilter(#[serde(with = "serde_regex")] RegexSet);

impl CommandFilter {
    /// Whether any expression matches `command`.
    #[must_use]
    pub fn is_match(&self, command: &str) -> bool {
        self.0.is_match(command)
    }
}

impl PartialEq for CommandFilter {
    fn eq(&self, other: &Self) -> bool {
        self.0.patterns() == other.0.patterns()
    }
}

/// The `[output]` table as written in `config.toml`.
///
/// This is the serde representation of [`OutputCapture`]. [`OutputCapture`] is the predominant way
/// you should interact with the configuration. This type represents the value in the `config.toml`.
/// Unlike [`OutputCapture`], it is a flattened representation.
///
/// The reason I went with a flattened representation is to enable the `atuin config set
/// output.enabled false` convention.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OutputCaptureConfig {
    pub(crate) enabled: bool,
    pub(crate) max_output_size: ByteSize,
    pub(crate) sync: bool,
    pub(crate) max_disk_usage: DiskUsageLimit,
    #[serde(default)]
    pub(crate) command_filter: CommandFilter,
}

impl Default for OutputCaptureConfig {
    fn default() -> Self {
        Self::from(OutputCapture::default())
    }
}

impl From<OutputCaptureConfig> for OutputCapture {
    fn from(config: OutputCaptureConfig) -> Self {
        if !config.enabled {
            return Self::Disabled;
        }

        Self::Enabled(CaptureLimits {
            max_output_size: config.max_output_size,
            sync: config.sync,
            max_disk_usage: config.max_disk_usage,
            command_filter: config.command_filter,
        })
    }
}

impl From<OutputCapture> for OutputCaptureConfig {
    fn from(capture: OutputCapture) -> Self {
        let (enabled, limits) = match capture {
            OutputCapture::Disabled => (false, CaptureLimits::default()),
            OutputCapture::Enabled(limits) => (true, limits),
        };
        Self {
            enabled,
            max_output_size: limits.max_output_size,
            sync: limits.sync,
            max_disk_usage: limits.max_disk_usage,
            command_filter: limits.command_filter,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn effective_limits_uses_defaults_when_disabled() {
        assert_eq!(OutputCapture::Disabled.effective_limits(), CaptureLimits::default());
    }

    #[rstest]
    fn effective_limits_returns_the_configured_limits_when_enabled() {
        let limits = CaptureLimits {
            max_output_size: ByteSize::b(42),
            ..CaptureLimits::default()
        };
        assert_eq!(OutputCapture::Enabled(limits.clone()).effective_limits(), limits);
    }
}
