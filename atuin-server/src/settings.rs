use std::fs::{create_dir_all, File};
use std::io::prelude::*;
use std::path::PathBuf;

use config::{Config, Environment, File as ConfigFile};
use eyre::{Context, Result};

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

        let mut s = Config::builder();

        if config_file.exists() {
            s = s.add_source(ConfigFile::with_name(config_file.to_str().unwrap()));
        } else {
            let example_config = include_bytes!("../server.toml");
            let mut file = File::create(config_file)?;
            file.write_all(example_config)?;
        }

        let s = s
            .set_default("host", "127.0.0.1")?
            .set_default("port", 8888)?
            .set_default("open_registration", false)?
            .set_default("db_uri", "default_uri")?
            .add_source(Environment::with_prefix("atuin").separator("_"));

        s.build()
            .wrap_err("failed to create config parser")?
            .try_into()
            .wrap_err("failed to deserialize")
    }
}
