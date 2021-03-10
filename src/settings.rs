use std::path::PathBuf;

use config::{Config, File};
use directories::ProjectDirs;
use eyre::{eyre, Result};
use std::fs;

#[derive(Debug, Deserialize)]
pub struct LocalDatabase {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct Local {
    pub server_address: String,
    pub dialect: String,
    pub db: LocalDatabase,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub local: Local,
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

        s.set_default("local.server_address", "https://atuin.elliehuxtable.com")?;
        s.set_default("local.dialect", "us")?;
        s.set_default("local.db.path", db_path.to_str())?;

        if config_file.exists() {
            s.merge(File::with_name(config_file.to_str().unwrap()))?;
        }

        // all paths should be expanded
        let db_path = s.get_str("local.db.path")?;
        let db_path = shellexpand::full(db_path.as_str())?;
        s.set("local.db.path", db_path.to_string())?;

        s.try_into()
            .map_err(|e| eyre!("failed to deserialize: {}", e))
    }
}
