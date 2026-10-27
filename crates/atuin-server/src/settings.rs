use std::io::prelude::*;
use std::path::Path;

use atuin_common::units::ByteSize;
use config::{
    Config, ConfigError, Environment, File as ConfigFile, FileFormat, Map, Source, Value, ValueKind,
};
use eyre::{Result, eyre};
use fs_err::{File, create_dir_all};
use serde::{Deserialize, Serialize};
use url::Url;

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

        if !config_file.exists() {
            create_dir_all(config_file.parent().unwrap())?;
            let mut file = File::create(&config_file)?;
            file.write_all(EXAMPLE_CONFIG.as_bytes())?;
        }

        Self::load(&config_file, None)
    }

    /// Load `config_file`, overridden by the `ATUIN_*` variables in `env`, or in the process
    /// environment if `None`.
    fn load(config_file: &Path, env: Option<Map<String, String>>) -> Result<Self> {
        let env = Environment::with_prefix("atuin")
            .prefix_separator("_")
            .separator("__")
            .ignore_empty(true)
            .source(env);

        let config = Config::builder()
            .set_default("host", "127.0.0.1")?
            .set_default("port", 8888)?
            .set_default("open_registration", false)?
            .set_default("max_record_size", "1GiB")?
            .set_default("path", "")?
            .set_default("register_webhook_username", "")?
            .set_default("metrics.enable", false)?
            .set_default("metrics.host", "127.0.0.1")?
            .set_default("metrics.port", 9001)?
            .add_source(ConfigFile::from(config_file).format(FileFormat::Toml))
            .add_source(WithoutServiceLinks(env))
            .build()?;

        config.try_deserialize().map_err(|e| eyre!("failed to deserialize: {}", e))
    }
}

/// The environment, minus an `ATUIN_PORT` that a Kubernetes service link set.
///
/// A Service named `atuin` gives each pod in its namespace `ATUIN_PORT=tcp://<ip>:<port>` unless
/// the pod sets `enableServiceLinks: false`. The other variables links inject end in
/// `_SERVICE_HOST`, `_SERVICE_PORT[_<name>]`, `_PORT` or `_PORT_<n>_<PROTO>[_PROTO|_PORT|_ADDR]`,
/// also for a Service named `atuin-<x>`, and no other setting has a key of that shape.
#[derive(Clone, Debug)]
struct WithoutServiceLinks(Environment);

impl Source for WithoutServiceLinks {
    fn clone_into_box(&self) -> Box<dyn Source + Send + Sync> {
        Box::new(self.clone())
    }

    fn collect(&self) -> Result<Map<String, Value>, ConfigError> {
        let mut env = self.0.collect()?;
        let Some(ValueKind::String(port)) = env.get("port").map(|port| &port.kind) else {
            return Ok(env);
        };
        let is_service_link = Url::parse(port).is_ok_and(|url| {
            matches!(url.scheme(), "tcp" | "udp" | "sctp") && url.port().is_some()
        });
        if !is_service_link {
            return Ok(env);
        }

        tracing::warn!(
            "ignoring ATUIN_PORT={port}, a Kubernetes service link for a Service named `atuin`, \
             and using the port from server.toml or the default; set ATUIN_PORT explicitly in the \
             pod's env, or `enableServiceLinks: false` in its spec"
        );
        env.remove("port");
        Ok(env)
    }
}

#[must_use]
pub fn example_config() -> &'static str {
    EXAMPLE_CONFIG
}

#[cfg(test)]
mod tests {
    use atuin_common::units::ByteSize;
    use config::{Config, File as ConfigFile, FileFormat, Map};
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

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

    /// A config dir whose `server.toml` sets `port` and `db_uri`, removed on drop.
    #[fixture]
    fn config_dir() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(
            dir.path().join("server.toml"),
            "port = 8000\ndb_uri = \"sqlite://file.db\"\n",
        )
        .unwrap();
        dir
    }

    #[rstest]
    #[case::service_link_port_is_ignored(
        &[
            ("ATUIN_PORT", "tcp://10.96.0.12:8888"),
            ("ATUIN_SERVICE_HOST", "10.96.0.12"),
            ("ATUIN_SERVICE_PORT", "8888"),
            ("ATUIN_SERVICE_PORT_HTTP", "8888"),
            ("ATUIN_PORT_8888_TCP", "tcp://10.96.0.12:8888"),
            ("ATUIN_PORT_8888_TCP_PROTO", "tcp"),
            ("ATUIN_PORT_8888_TCP_PORT", "8888"),
            ("ATUIN_PORT_8888_TCP_ADDR", "10.96.0.12"),
            ("ATUIN_POSTGRES_PORT", "tcp://10.96.0.13:5432"),
            ("ATUIN_POSTGRES_SERVICE_HOST", "10.96.0.13"),
            ("ATUIN_POSTGRES_SERVICE_PORT", "5432"),
            ("ATUIN_POSTGRES_PORT_5432_TCP_PORT", "5432"),
        ],
        8000,
        "sqlite://file.db",
    )]
    #[case::ipv6_service_link_port_is_ignored(
        &[("ATUIN_PORT", "tcp://[fd00::12]:8888")],
        8000,
        "sqlite://file.db",
    )]
    #[case::numeric_port_beats_server_toml(&[("ATUIN_PORT", "9000")], 9000, "sqlite://file.db")]
    #[case::empty_port_is_unset(&[("ATUIN_PORT", "")], 8000, "sqlite://file.db")]
    #[case::db_uri_beats_server_toml(&[("ATUIN_DB_URI", "sqlite://env.db")], 8000, "sqlite://env.db")]
    #[case::empty_db_uri_is_unset(&[("ATUIN_DB_URI", "")], 8000, "sqlite://file.db")]
    fn settings_from_server_toml_and_environment(
        config_dir: TempDir,
        #[case] env: &[(&str, &str)],
        #[case] port: u16,
        #[case] db_uri: &str,
    ) {
        let env = env.iter().map(|&(key, value)| (key.to_owned(), value.to_owned())).collect();

        let settings = Settings::load(&config_dir.path().join("server.toml"), Some(env)).unwrap();

        assert_eq!((settings.port, settings.db_settings.db_uri.as_str()), (port, db_uri));
    }

    #[rstest]
    fn a_url_port_that_is_not_a_service_link_is_an_error(config_dir: TempDir) {
        let env = Map::from([("ATUIN_PORT".to_owned(), "http://10.96.0.12:8888".to_owned())]);

        let err = Settings::load(&config_dir.path().join("server.toml"), Some(env)).unwrap_err();

        assert!(err.to_string().contains("key `port`"), "{err}");
    }
}
