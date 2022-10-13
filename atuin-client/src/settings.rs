use std::{
    io::prelude::*,
    path::{Path, PathBuf},
};

use chrono::{prelude::*, Utc};
use config::{Config, Environment, File as ConfigFile, FileFormat};
use eyre::{eyre, Context, Result};
use fs_err::{create_dir_all, File};
use parse_duration::parse;
use semver::Version;
use serde::Deserialize;

pub const HISTORY_PAGE_SIZE: i64 = 100;
pub const LAST_SYNC_FILENAME: &str = "last_sync_time";
pub const LAST_VERSION_CHECK_FILENAME: &str = "last_version_check_time";
pub const LATEST_VERSION_FILENAME: &str = "latest_version";

#[derive(Clone, Debug, Deserialize, Copy)]
pub enum SearchMode {
    #[serde(rename = "prefix")]
    Prefix,

    #[serde(rename = "fulltext")]
    FullText,

    #[serde(rename = "fuzzy")]
    Fuzzy,
}

#[derive(Clone, Debug, Deserialize, Copy, PartialEq, Eq)]
pub enum FilterMode {
    #[serde(rename = "global")]
    Global = 0,

    #[serde(rename = "host")]
    Host = 1,

    #[serde(rename = "session")]
    Session = 2,

    #[serde(rename = "directory")]
    Directory = 3,
}

impl FilterMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            FilterMode::Global => "GLOBAL",
            FilterMode::Host => "HOST",
            FilterMode::Session => "SESSION",
            FilterMode::Directory => "DIRECTORY",
        }
    }
}

// FIXME: Can use upstream Dialect enum if https://github.com/stevedonovan/chrono-english/pull/16 is merged
#[derive(Clone, Debug, Deserialize, Copy)]
pub enum Dialect {
    #[serde(rename = "us")]
    Us,

    #[serde(rename = "uk")]
    Uk,
}

impl From<Dialect> for chrono_english::Dialect {
    fn from(d: Dialect) -> chrono_english::Dialect {
        match d {
            Dialect::Uk => chrono_english::Dialect::Uk,
            Dialect::Us => chrono_english::Dialect::Us,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Copy)]
pub enum Style {
    #[serde(rename = "auto")]
    Auto,

    #[serde(rename = "full")]
    Full,

    #[serde(rename = "compact")]
    Compact,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub dialect: Dialect,
    pub style: Style,
    pub auto_sync: bool,
    pub update_check: bool,
    pub sync_address: String,
    pub sync_frequency: String,
    pub db_path: String,
    pub key_path: String,
    pub session_path: String,
    pub search_mode: SearchMode,
    pub filter_mode: FilterMode,
    // This is automatically loaded when settings is created. Do not set in
    // config! Keep secrets and settings apart.
    pub session_token: String,
}

impl Settings {
    fn save_to_data_dir(filename: &str, value: &str) -> Result<()> {
        let data_dir = atuin_common::utils::data_dir();
        let data_dir = data_dir.as_path();

        let path = data_dir.join(filename);

        fs_err::write(path, value)?;

        Ok(())
    }

    fn read_from_data_dir(filename: &str) -> Option<String> {
        let data_dir = atuin_common::utils::data_dir();
        let data_dir = data_dir.as_path();

        let path = data_dir.join(filename);

        if !path.exists() {
            return None;
        }

        let value = fs_err::read_to_string(path);

        value.ok()
    }

    fn save_current_time(filename: &str) -> Result<()> {
        Settings::save_to_data_dir(filename, Utc::now().to_rfc3339().as_str())?;

        Ok(())
    }

    fn load_time_from_file(filename: &str) -> Result<chrono::DateTime<Utc>> {
        let value = Settings::read_from_data_dir(filename);

        match value {
            Some(v) => {
                let time = chrono::DateTime::parse_from_rfc3339(v.as_str())?;

                Ok(time.with_timezone(&Utc))
            }
            None => Ok(Utc.ymd(1970, 1, 1).and_hms(0, 0, 0)),
        }
    }

    pub fn save_sync_time() -> Result<()> {
        Settings::save_current_time(LAST_SYNC_FILENAME)
    }

    pub fn save_version_check_time() -> Result<()> {
        Settings::save_current_time(LAST_VERSION_CHECK_FILENAME)
    }

    pub fn last_sync() -> Result<chrono::DateTime<Utc>> {
        Settings::load_time_from_file(LAST_SYNC_FILENAME)
    }

    pub fn last_version_check() -> Result<chrono::DateTime<Utc>> {
        Settings::load_time_from_file(LAST_VERSION_CHECK_FILENAME)
    }

    pub fn should_sync(&self) -> Result<bool> {
        let session_path = atuin_common::utils::data_dir().join("session");

        if !self.auto_sync || !session_path.exists() {
            return Ok(false);
        }

        match parse(self.sync_frequency.as_str()) {
            Ok(d) => {
                let d = chrono::Duration::from_std(d).unwrap();
                Ok(Utc::now() - Settings::last_sync()? >= d)
            }
            Err(e) => Err(eyre!("failed to check sync: {}", e)),
        }
    }

    fn needs_update_check(&self) -> Result<bool> {
        let last_check = Settings::last_version_check()?;
        let diff = Utc::now() - last_check;

        // Check a max of once per hour
        Ok(diff.num_hours() >= 1)
    }

    async fn latest_version(&self) -> Result<Version> {
        // Default to the current version, and if that doesn't parse, a version so high it's unlikely to ever
        // suggest upgrading.
        let current =
            Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(100000, 0, 0));

        if !self.needs_update_check()? {
            // Worst case, we don't want Atuin to fail to start because something funky is going on with
            // version checking.
            let version = match Settings::read_from_data_dir(LATEST_VERSION_FILENAME) {
                Some(v) => Version::parse(&v).unwrap_or(current),
                None => current,
            };

            return Ok(version);
        }

        #[cfg(feature = "sync")]
        let latest = crate::api_client::latest_version().await.unwrap_or(current);

        #[cfg(not(feature = "sync"))]
        let latest = current;

        Settings::save_version_check_time()?;
        Settings::save_to_data_dir(LATEST_VERSION_FILENAME, latest.to_string().as_str())?;

        Ok(latest)
    }

    // Return Some(latest version) if an update is needed. Otherwise, none.
    pub async fn needs_update(&self) -> Option<Version> {
        if !self.update_check {
            return None;
        }

        let current =
            Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(100000, 0, 0));

        let latest = self.latest_version().await;

        if latest.is_err() {
            return None;
        }

        let latest = latest.unwrap();

        if latest > current {
            return Some(latest);
        }

        None
    }

    pub fn new() -> Result<Self> {
        let config_dir = atuin_common::utils::config_dir();

        let data_dir = atuin_common::utils::data_dir();

        create_dir_all(&config_dir)
            .wrap_err_with(|| format!("could not create dir {:?}", config_dir))?;
        create_dir_all(&data_dir)
            .wrap_err_with(|| format!("could not create dir {:?}", data_dir))?;

        let mut config_file = if let Ok(p) = std::env::var("ATUIN_CONFIG_DIR") {
            PathBuf::from(p)
        } else {
            let mut config_file = PathBuf::new();
            config_file.push(config_dir);
            config_file
        };

        config_file.push("config.toml");

        let db_path = data_dir.join("history.db");
        let key_path = data_dir.join("key");
        let session_path = data_dir.join("session");

        let mut config_builder = Config::builder()
            .set_default("db_path", db_path.to_str())?
            .set_default("key_path", key_path.to_str())?
            .set_default("session_path", session_path.to_str())?
            .set_default("dialect", "us")?
            .set_default("auto_sync", true)?
            .set_default("update_check", true)?
            .set_default("sync_frequency", "1h")?
            .set_default("sync_address", "https://api.atuin.sh")?
            .set_default("search_mode", "prefix")?
            .set_default("filter_mode", "global")?
            .set_default("session_token", "")?
            .set_default("style", "auto")?
            .add_source(
                Environment::with_prefix("atuin")
                    .prefix_separator("_")
                    .separator("__"),
            );

        config_builder = if config_file.exists() {
            config_builder.add_source(ConfigFile::new(
                config_file.to_str().unwrap(),
                FileFormat::Toml,
            ))
        } else {
            let example_config = include_bytes!("../config.toml");
            let mut file = File::create(config_file).wrap_err("could not create config file")?;
            file.write_all(example_config)
                .wrap_err("could not write default config file")?;

            config_builder
        };

        let config = config_builder.build()?;
        let mut settings: Settings = config
            .try_deserialize()
            .map_err(|e| eyre!("failed to deserialize: {}", e))?;

        // all paths should be expanded
        let db_path = settings.db_path;
        let db_path = shellexpand::full(&db_path)?;
        settings.db_path = db_path.to_string();

        let key_path = settings.key_path;
        let key_path = shellexpand::full(&key_path)?;
        settings.key_path = key_path.to_string();

        let session_path = settings.session_path;
        let session_path = shellexpand::full(&session_path)?;
        settings.session_path = session_path.to_string();

        // Finally, set the auth token
        if Path::new(session_path.to_string().as_str()).exists() {
            let token = fs_err::read_to_string(session_path.to_string())?;
            settings.session_token = token.trim().to_string();
        } else {
            settings.session_token = String::from("not logged in");
        }

        Ok(settings)
    }
}
