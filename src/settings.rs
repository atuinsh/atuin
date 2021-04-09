use std::path::PathBuf;

use config::{Config, File};
use directories::ProjectDirs;
use eyre::{eyre, Result};
use std::fs;

#[derive(Debug, Deserialize)]
pub struct Local {
    pub dialect: String,
    pub sync: bool,
    pub sync_address: String,
    pub sync_frequency: String,
    pub db_path: String,
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
            .ok_or_else(|| {
                eyre!("could not determine db file location\nspecify one using the --db flag")
            })?
            .data_dir()
            .join("history.db");

        s.set_default("local.db_path", db_path.to_str())?;
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
        s.set("local.db.path", db_path.to_string())?;

        s.try_into()
            .map_err(|e| eyre!("failed to deserialize: {}", e))
    }
}
