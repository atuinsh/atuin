use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use config::{Config, File};
use directories::ProjectDirs;
use eyre::{eyre, Result};
use parse_duration::parse;

#[derive(Debug, Deserialize)]
pub struct Local {
    pub dialect: String,
    pub sync: bool,
    pub sync_address: String,
    pub sync_frequency: String,
    pub db_path: String,
    pub key_path: String,
    pub session_path: String,

    // This is automatically loaded when settings is created. Do not set in
    // config! Keep secrets and settings apart.
    pub session_token: String,
}

impl Local {
    pub fn save_sync_time(&self) -> Result<()> {
        let sync_time_path = ProjectDirs::from("com", "elliehuxtable", "atuin")
            .ok_or_else(|| eyre!("could not determine key file location"))?;
        let sync_time_path = sync_time_path.data_dir().join("last_sync_time");

        std::fs::write(sync_time_path, Utc::now().to_rfc3339())?;

        Ok(())
    }

    pub fn should_sync(&self) -> bool {
        // TODO: Make the sync time file path configurable
        let sync_time_path = ProjectDirs::from("com", "elliehuxtable", "atuin");

        if sync_time_path.is_none() {
            debug!("failed to load projectdirs, not syncing");
            return false;
        }

        let sync_time_path = sync_time_path.unwrap();
        let sync_time_path = sync_time_path.data_dir().join("last_sync_time");

        if !sync_time_path.exists() {
            debug!("no prior sync time file found, syncing");
            return true;
        }

        let time = std::fs::read_to_string(sync_time_path);

        if time.is_err() {
            debug!("failed to read last sync time, not syncing");
            return false;
        }

        let time = chrono::DateTime::parse_from_rfc3339(time.unwrap().as_str());

        if time.is_err() {
            debug!("failed to parse last sync time, not syncing");
            return false;
        }

        let time = time.unwrap().with_timezone(&Utc);

        match parse(self.sync_frequency.as_str()) {
            Ok(d) => {
                let d = chrono::Duration::from_std(d).unwrap();
                Utc::now() - time >= d
            }
            Err(_) => false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Remote {
    pub host: String,
    pub port: u16,
    pub db_uri: String,
    pub open_registration: bool,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub local: Local,
    pub remote: Remote,
}

impl Settings {
    pub fn new() -> Result<Self> {
        let config_dir = ProjectDirs::from("com", "elliehuxtable", "atuin").unwrap();
        let config_dir = config_dir.config_dir();

        fs::create_dir_all(config_dir)?;

        let mut config_file = PathBuf::new();
        config_file.push(config_dir);
        config_file.push("config.toml");
        let config_file = config_file.as_path();

        // create the config file if it does not exist

        let mut s = Config::new();

        let db_path = ProjectDirs::from("com", "elliehuxtable", "atuin")
            .ok_or_else(|| eyre!("could not determine db file location"))?
            .data_dir()
            .join("history.db");

        let key_path = ProjectDirs::from("com", "elliehuxtable", "atuin")
            .ok_or_else(|| eyre!("could not determine key file location"))?
            .data_dir()
            .join("key");

        let session_path = ProjectDirs::from("com", "elliehuxtable", "atuin")
            .ok_or_else(|| eyre!("could not determine session file location"))?
            .data_dir()
            .join("session");

        s.set_default("local.db_path", db_path.to_str())?;
        s.set_default("local.key_path", key_path.to_str())?;
        s.set_default("local.session_path", session_path.to_str())?;
        s.set_default("local.dialect", "us")?;
        s.set_default("local.sync", false)?;
        s.set_default("local.sync_frequency", "5m")?;
        s.set_default("local.sync_address", "https://atuin.ellie.wtf")?;

        s.set_default("remote.host", "127.0.0.1")?;
        s.set_default("remote.port", 8888)?;
        s.set_default("remote.open_registration", false)?;
        s.set_default("remote.db_uri", "please set a postgres url")?;

        if config_file.exists() {
            s.merge(File::with_name(config_file.to_str().unwrap()))?;
        }

        // all paths should be expanded
        let db_path = s.get_str("local.db_path")?;
        let db_path = shellexpand::full(db_path.as_str())?;
        s.set("local.db_path", db_path.to_string())?;

        let key_path = s.get_str("local.key_path")?;
        let key_path = shellexpand::full(key_path.as_str())?;
        s.set("local.key_path", key_path.to_string())?;

        let session_path = s.get_str("local.session_path")?;
        let session_path = shellexpand::full(session_path.as_str())?;
        s.set("local.session_path", session_path.to_string())?;

        // Finally, set the auth token
        if Path::new(session_path.to_string().as_str()).exists() {
            let token = std::fs::read_to_string(session_path.to_string())?;
            s.set("local.session_token", token)?;
        } else {
            s.set("local.session_token", "not logged in")?;
        }

        s.try_into()
            .map_err(|e| eyre!("failed to deserialize: {}", e))
    }
}
