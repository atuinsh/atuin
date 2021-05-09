use std::fs::{create_dir_all, File};
use std::io::prelude::*;
use std::path::{Path, PathBuf};

use chrono::prelude::*;
use chrono::Utc;
use config::{Config, Environment, File as ConfigFile};
use eyre::{eyre, Result};
use parse_duration::parse;

pub const HISTORY_PAGE_SIZE: i64 = 100;

#[derive(Clone, Debug, Deserialize, Copy)]
pub enum SearchMode {
    #[serde(rename = "prefix")]
    Prefix,

    #[serde(rename = "fulltext")]
    FullText,
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

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub dialect: Dialect,
    pub auto_sync: bool,
    pub sync_address: String,
    pub sync_frequency: String,
    pub db_path: String,
    pub key_path: String,
    pub session_path: String,
    pub search_mode: SearchMode,

    // This is automatically loaded when settings is created. Do not set in
    // config! Keep secrets and settings apart.
    pub session_token: String,
}

impl Settings {
    pub fn save_sync_time() -> Result<()> {
        let data_dir = atuin_common::utils::data_dir();
        let data_dir = data_dir.as_path();

        let sync_time_path = data_dir.join("last_sync_time");

        std::fs::write(sync_time_path, Utc::now().to_rfc3339())?;

        Ok(())
    }

    pub fn last_sync() -> Result<chrono::DateTime<Utc>> {
        let data_dir = atuin_common::utils::data_dir();
        let data_dir = data_dir.as_path();

        let sync_time_path = data_dir.join("last_sync_time");

        if !sync_time_path.exists() {
            return Ok(Utc.ymd(1970, 1, 1).and_hms(0, 0, 0));
        }

        let time = std::fs::read_to_string(sync_time_path)?;
        let time = chrono::DateTime::parse_from_rfc3339(time.as_str())?;

        Ok(time.with_timezone(&Utc))
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

    pub fn new() -> Result<Self> {
        let config_dir = atuin_common::utils::config_dir();

        let data_dir = atuin_common::utils::data_dir();

        create_dir_all(&config_dir)?;
        create_dir_all(&data_dir)?;

        let mut config_file = if let Ok(p) = std::env::var("ATUIN_CONFIG_DIR") {
            PathBuf::from(p)
        } else {
            let mut config_file = PathBuf::new();
            config_file.push(config_dir);
            config_file
        };

        config_file.push("config.toml");

        let mut s = Config::new();

        let db_path = data_dir.join("history.db");
        let key_path = data_dir.join("key");
        let session_path = data_dir.join("session");

        s.set_default("db_path", db_path.to_str())?;
        s.set_default("key_path", key_path.to_str())?;
        s.set_default("session_path", session_path.to_str())?;
        s.set_default("dialect", "us")?;
        s.set_default("auto_sync", true)?;
        s.set_default("sync_frequency", "1h")?;
        s.set_default("sync_address", "https://api.atuin.sh")?;
        s.set_default("search_mode", "prefix")?;

        if config_file.exists() {
            s.merge(ConfigFile::with_name(config_file.to_str().unwrap()))?;
        } else {
            let example_config = include_bytes!("../config.toml");
            let mut file = File::create(config_file)?;
            file.write_all(example_config)?;
        }

        s.merge(Environment::with_prefix("atuin").separator("_"))?;

        // all paths should be expanded
        let db_path = s.get_str("db_path")?;
        let db_path = shellexpand::full(db_path.as_str())?;
        s.set("db_path", db_path.to_string())?;

        let key_path = s.get_str("key_path")?;
        let key_path = shellexpand::full(key_path.as_str())?;
        s.set("key_path", key_path.to_string())?;

        let session_path = s.get_str("session_path")?;
        let session_path = shellexpand::full(session_path.as_str())?;
        s.set("session_path", session_path.to_string())?;

        // Finally, set the auth token
        if Path::new(session_path.to_string().as_str()).exists() {
            let token = std::fs::read_to_string(session_path.to_string())?;
            s.set("session_token", token.trim())?;
        } else {
            s.set("session_token", "not logged in")?;
        }

        s.try_into()
            .map_err(|e| eyre!("failed to deserialize: {}", e))
    }
}
