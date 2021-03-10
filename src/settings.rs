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
        let path = ProjectDirs::from("com", "elliehuxtable", "atuin").unwrap();
        let path = path.config_dir();

        fs::create_dir_all(path)?;

        let path = path.to_str().unwrap();
        let path = format!("{}/config", path);

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

        s.merge(File::with_name(path.as_str()))?;

        // all paths should be expanded
        let db_path = s.get_str("local.db.path")?;
        let db_path = shellexpand::full(db_path.as_str())?;
        s.set("local.db.path", db_path.to_string())?;

        s.try_into()
            .map_err(|e| eyre!("failed to deserialize: {}", e))
    }
}
