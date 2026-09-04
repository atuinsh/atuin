use atuin_common::size::{ByteSize, DiskUsageLimit, Percent};
use serde::{Deserialize, Serialize};

/// The `[output_capture]` section of `config.toml`: capturing and storing command output.
///
/// Nothing consumes these settings yet; they are parsed and validated so the config format is
/// settled before the capture pipeline reads them.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OutputCapture {
    /// Whether command output is captured at all.
    pub enabled: bool,

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

impl Default for OutputCapture {
    fn default() -> Self {
        Self {
            enabled: false,
            max_output_size: ByteSize::MIB,
            sync: false,
            max_disk_usage: DiskUsageLimit::Percent(
                Percent::new(10).expect("10 is a valid percentage"),
            ),
        }
    }
}
