use config::{builder::DefaultState, ConfigBuilder};
use eyre::Result;
use serde::Deserialize;

// Settings

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub auto_sync: bool,
    pub sync_address: String,
    pub sync_frequency: String,
    #[serde(default)]
    pub sync: Sync,
}

// Defaults

pub(crate) fn defaults(
    builder: ConfigBuilder<DefaultState>,
) -> Result<ConfigBuilder<DefaultState>> {
    Ok(builder
        .set_default("auto_sync", true)?
        .set_default("sync_address", "https://api.atuin.sh")?
        .set_default("sync_frequency", "10m")?
        .set_default("sync.records", false)?)
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct Sync {
    pub records: bool,
}
