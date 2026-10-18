use std::io::prelude::*;

use atuin_common::units::ByteSize;
use config::builder::DefaultState;
use config::{Config, ConfigBuilder, Environment, File as ConfigFile, FileFormat, Source};
use eyre::{Result, eyre};
use fs_err::{File, create_dir_all};
use serde::{Deserialize, Serialize};

use crate::db::DbSettings;

static EXAMPLE_CONFIG: &str = include_str!("../server.toml");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Metrics {
    #[serde(alias = "enabled")]
    pub enable: bool,
    pub host: String,
    pub port: u16,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            enable: false,
            host: String::from("127.0.0.1"),
            port: 9001,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub open_registration: bool,
    pub max_record_size: ByteSize,
    pub register_webhook_url: Option<url::Url>,
    pub register_webhook_username: String,
    pub metrics: Metrics,

    /// Advertise a version that is not what we are _actually_ running
    /// Many clients compare their version with api.atuin.sh, and if they differ, notify the user
    /// that an update is available.
    /// Now that we take beta releases, we should be able to advertise a different version to avoid
    /// notifying users when the server runs something that is not a stable release.
    pub fake_version: Option<String>,

    #[serde(flatten)]
    pub db_settings: DbSettings,
}

impl Settings {
    pub fn new() -> Result<Self> {
        let config_file = atuin_common::dirs::config_path("server.toml");

        // create the config file if it does not exist
        let file = if config_file.exists() {
            Some(ConfigFile::new(config_file.to_str().unwrap(), FileFormat::Toml))
        } else {
            create_dir_all(config_file.parent().unwrap())?;
            let mut file = File::create(config_file)?;
            file.write_all(EXAMPLE_CONFIG.as_bytes())?;

            None
        };

        let config = Self::builder(file, Self::environment())?.build()?;

        config.try_deserialize().map_err(|e| eyre!("failed to deserialize: {}", e))
    }

    /// The `ATUIN_*` environment variables, as a config source.
    fn environment() -> Environment {
        Environment::with_prefix("atuin").prefix_separator("_").separator("__")
    }

    /// Layers the defaults, then the config file, if any, then `env`, so that environment
    /// variables override the file.
    fn builder(
        file: Option<impl Source + Send + Sync + 'static>,
        env: Environment,
    ) -> Result<ConfigBuilder<DefaultState>> {
        let mut builder = Config::builder()
            .set_default("host", "127.0.0.1")?
            .set_default("port", 8888)?
            .set_default("open_registration", false)?
            .set_default("max_record_size", "1GiB")?
            .set_default("path", "")?
            .set_default("register_webhook_username", "")?
            .set_default("metrics.enable", false)?
            .set_default("metrics.host", "127.0.0.1")?
            .set_default("metrics.port", 9001)?;

        if let Some(file) = file {
            builder = builder.add_source(file);
        }

        Ok(builder.add_source(env))
    }
}

#[must_use]
pub fn example_config() -> &'static str {
    EXAMPLE_CONFIG
}

#[cfg(test)]
mod tests {
    use atuin_common::units::ByteSize;
    use config::{Config, File as ConfigFile, FileFormat};
    use rstest::rstest;

    use super::Settings;

    fn settings_with_max_record_size(value: &str) -> Settings {
        let toml = format!(
            "host = \"\"\nport = 8888\npath = \"\"\nopen_registration = \
             false\nregister_webhook_username = \"\"\ndb_uri = \
             \"sqlite::memory:\"\nmax_record_size = {value}\n[metrics]\nenable = false\nhost = \
             \"127.0.0.1\"\nport = 9001\n"
        );
        Config::builder()
            .add_source(ConfigFile::from_str(&toml, FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap()
    }

    #[rstest]
    fn max_record_size_accepts_a_human_string() {
        let settings = settings_with_max_record_size("\"500MB\"");
        assert_eq!(settings.max_record_size, ByteSize::mb(500));
    }

    #[rstest]
    fn max_record_size_accepts_a_bare_integer() {
        let settings = settings_with_max_record_size("1048576");
        assert_eq!(settings.max_record_size, ByteSize::mib(1));
    }

    #[rstest]
    #[case::env_overrides_the_file(Some("9000"), 9000)]
    #[case::file_without_env(None, 8000)]
    fn environment_overrides_the_config_file(#[case] env: Option<&str>, #[case] expected: u16) {
        let env = Settings::environment().source(Some(
            env.map(|value| ("ATUIN_PORT".to_string(), value.to_string())).into_iter().collect(),
        ));
        let file =
            ConfigFile::from_str("port = 8000\ndb_uri = \"sqlite::memory:\"\n", FileFormat::Toml);
        let settings: Settings =
            Settings::builder(Some(file), env).unwrap().build().unwrap().try_deserialize().unwrap();
        assert_eq!(settings.port, expected);
    }
}
