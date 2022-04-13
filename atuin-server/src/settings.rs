use fs_err::{create_dir_all, File};
use std::io::prelude::*;
use std::path::PathBuf;

use config::{Config, Environment, File as ConfigFile, FileFormat};
use eyre::{eyre, Result};

pub const HISTORY_PAGE_SIZE: i64 = 100;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub db_uri: String,
    pub open_registration: bool,
}

impl Settings {
    pub fn new() -> Result<Self> {
        let config_dir = atuin_common::utils::config_dir();
        let config_dir = config_dir.as_path();

        create_dir_all(config_dir)?;

        let mut config_file = if let Ok(p) = std::env::var("ATUIN_CONFIG_DIR") {
            PathBuf::from(p)
        } else {
            let mut config_file = PathBuf::new();
            config_file.push(config_dir);
            config_file
        };

        config_file.push("server.toml");

        // create the config file if it does not exist
        let mut config_builder = Config::builder()
            .set_default("host", "127.0.0.1")?
            .set_default("port", 8888)?
            .set_default("open_registration", false)?
            .set_default("db_uri", "default_uri")?
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
            let example_config = include_bytes!("../server.toml");
            let mut file = File::create(config_file)?;
            file.write_all(example_config)?;

            config_builder
        };

        let config = config_builder.build()?;

        config
            .try_deserialize()
            .map_err(|e| eyre!("failed to deserialize: {}", e))
    }
}
