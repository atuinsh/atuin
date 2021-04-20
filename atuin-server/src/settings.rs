use std::fs::{create_dir_all, File};
use std::io::prelude::*;
use std::path::PathBuf;

use config::{Config, Environment, File as ConfigFile};
use directories::ProjectDirs;
use eyre::{eyre, Result};

pub const HISTORY_PAGE_SIZE: i64 = 100;

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub db_uri: String,
    pub open_registration: bool,
}

impl Settings {
    pub fn new() -> Result<Self> {
        let config_dir = ProjectDirs::from("com", "elliehuxtable", "atuin").unwrap();
        let config_dir = config_dir.config_dir();

        create_dir_all(config_dir)?;

        let config_file = if let Ok(p) = std::env::var("ATUIN_CONFIG") {
            PathBuf::from(p)
        } else {
            let mut config_file = PathBuf::new();
            config_file.push(config_dir);
            config_file.push("server.toml");
            config_file
        };

        // create the config file if it does not exist

        let mut s = Config::new();

        if config_file.exists() {
            s.merge(ConfigFile::with_name(config_file.to_str().unwrap()))?;
        } else {
            let example_config = include_bytes!("../server.toml");
            let mut file = File::create(config_file)?;
            file.write_all(example_config)?;
        }

        s.set_default("host", "127.0.0.1")?;
        s.set_default("port", 8888)?;
        s.set_default("open_registration", false)?;
        s.set_default("db_uri", "default_uri")?;

        s.merge(Environment::with_prefix("atuin").separator("_"))?;

        s.try_into()
            .map_err(|e| eyre!("failed to deserialize: {}", e))
    }
}
