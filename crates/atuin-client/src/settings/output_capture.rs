use atuin_common::size::{ByteSize, DiskUsageLimit, Percent};
use serde::{Deserialize, Serialize};

/// The `[output_capture]` section of `config.toml`: capturing and storing command output.
///
/// Nothing consumes these settings yet; they are parsed and validated so the config format is
/// settled before the capture pipeline reads them.
#[derive(Clone, Debug, Serialize)]
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

/// The raw shape of `[output_capture]` as it comes off the wire, before the `enable`/`enabled`
/// alias is resolved.
///
/// `enabled` isn't declared with `#[serde(alias = "enable")]` (the way `[daemon]` and
/// `[pty_proxy]` do it) because `builder_with_data_dir` also registers a `config`-level default
/// for `output_capture.enabled`, so `atuin config get --resolved output_capture` can show it.
/// When a config file sets only `enable`, `config`'s source merging keeps `enabled` (the
/// default) and `enable` (the override) as two separate map entries; serde's generated alias
/// handling then sees two keys resolving to the same field and rejects the input as a duplicate
/// field, rather than letting the override win. Reading both keys explicitly here and preferring
/// `enable` when both are present avoids that collision.
#[derive(Deserialize)]
struct OutputCaptureWire {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    enable: Option<bool>,
    max_output_size: ByteSize,
    sync: bool,
    max_disk_usage: DiskUsageLimit,
}

impl<'de> Deserialize<'de> for OutputCapture {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = OutputCaptureWire::deserialize(deserializer)?;
        Ok(Self {
            enabled: wire.enable.or(wire.enabled).unwrap_or(false),
            max_output_size: wire.max_output_size,
            sync: wire.sync,
            max_disk_usage: wire.max_disk_usage,
        })
    }
}
