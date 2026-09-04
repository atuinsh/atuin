use atuin_common::size::{ByteSize, DiskUsageLimit, Percent};
use serde::{Deserialize, Serialize};

/// The `[output_capture]` section of `config.toml`: capturing and storing command output.
///
/// In the file this is a flat table switched by `enabled`; the remaining keys only mean
/// something when capture is on, so here they are only reachable through [`Self::Enabled`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
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
}

/// How much captured output is kept, and where it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureLimits {
    /// The most output kept for a single command.
    ///
    /// Longer output is middle-truncated, so `a super very long output` is stored as
    /// `a super...long output`: both the start and the end survive.
    pub max_output_size: ByteSize,

    /// Whether captured output is synced.
    ///
    /// Sync of command output is not supported yet and is coming in the next release; until
    /// then this has no effect.
    pub sync: bool,

    /// The most disk space captured output may use: an absolute size (`10GB`), a share of the
    /// disk holding the data directory (`10%`), or `unlimited`. Once the limit is reached the
    /// oldest output is forgotten.
    pub max_disk_usage: DiskUsageLimit,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_output_size: ByteSize::MIB,
            sync: false,
            max_disk_usage: DiskUsageLimit::Percent(
                Percent::new(10).expect("10 is a valid percentage"),
            ),
        }
    }
}

/// The `[output_capture]` table as written in `config.toml`.
///
/// This is the serde representation of [`OutputCapture`]. Keeping it flat keeps the keys
/// independent (so `atuin config get output_capture.max_output_size` and
/// `ATUIN_OUTPUT_CAPTURE__MAX_OUTPUT_SIZE` work) and keeps validation errors pointing at the
/// offending key. The builder registers its defaults against these fields.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OutputCaptureConfig {
    pub(crate) enabled: bool,
    pub(crate) max_output_size: ByteSize,
    pub(crate) sync: bool,
    pub(crate) max_disk_usage: DiskUsageLimit,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn custom_limits() -> CaptureLimits {
        CaptureLimits {
            max_output_size: ByteSize::from_bytes(512 << 10),
            sync: true,
            max_disk_usage: DiskUsageLimit::Unlimited,
        }
    }

    #[rstest]
    #[case::disabled(OutputCapture::Disabled)]
    #[case::enabled_with_defaults(OutputCapture::Enabled(CaptureLimits::default()))]
    #[case::enabled_with_custom_limits(OutputCapture::Enabled(custom_limits()))]
    fn the_config_form_round_trips(#[case] capture: OutputCapture) {
        let config = OutputCaptureConfig::from(capture.clone());
        assert_eq!(OutputCapture::from(config), capture);
    }

    /// Limits written under `enabled = false` are dropped rather than carried around.
    #[rstest]
    fn disabled_forgets_its_limits() {
        let config = OutputCaptureConfig {
            enabled: false,
            ..OutputCaptureConfig::from(OutputCapture::Enabled(custom_limits()))
        };
        assert_eq!(OutputCapture::from(config), OutputCapture::Disabled);
    }

    #[rstest]
    fn serde_uses_the_flat_form() {
        let json = serde_json::to_string(&OutputCapture::Enabled(custom_limits())).unwrap();
        assert_eq!(
            json,
            r#"{"enabled":true,"max_output_size":"512KB","sync":true,"max_disk_usage":"unlimited"}"#
        );
        assert_eq!(
            serde_json::from_str::<OutputCapture>(&json).unwrap(),
            OutputCapture::Enabled(custom_limits())
        );
    }
}
