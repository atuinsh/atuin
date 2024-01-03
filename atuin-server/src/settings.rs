use std::{io::prelude::*, path::PathBuf};

use config::{Config, Environment, File as ConfigFile, FileFormat};
use eyre::{bail, eyre, Context, Result};
use fs_err::{create_dir_all, File};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

static EXAMPLE_CONFIG: &str = include_str!("../server.toml");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Metrics {
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
pub struct Settings<DbSettings> {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub open_registration: bool,
    pub max_history_length: usize,
    pub max_record_size: usize,
    pub page_size: i64,
    pub register_webhook_url: Option<String>,
    pub register_webhook_username: String,
    pub metrics: Metrics,
    pub tls: Tls,

    #[serde(flatten)]
    pub db_settings: DbSettings,
}

impl<DbSettings: DeserializeOwned> Settings<DbSettings> {
    pub fn new() -> Result<Self> {
        let mut config_file = if let Ok(p) = std::env::var("ATUIN_CONFIG_DIR") {
            PathBuf::from(p)
        } else {
            let mut config_file = PathBuf::new();
            let config_dir = atuin_common::utils::config_dir();
            config_file.push(config_dir);
            config_file
        };

        config_file.push("server.toml");

        // create the config file if it does not exist
        let mut config_builder = Config::builder()
            .set_default("host", "127.0.0.1")?
            .set_default("port", 8888)?
            .set_default("open_registration", false)?
            .set_default("max_history_length", 8192)?
            .set_default("max_record_size", 1024 * 1024 * 1024)? // pretty chonky
            .set_default("path", "")?
            .set_default("register_webhook_username", "")?
            .set_default("page_size", 1100)?
            .set_default("metrics.enable", false)?
            .set_default("metrics.host", "127.0.0.1")?
            .set_default("metrics.port", 9001)?
            .set_default("tls.enable", false)?
            .set_default("tls.cert_path", "")?
            .set_default("tls.pkey_path", "")?
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
            create_dir_all(config_file.parent().unwrap())?;
            let mut file = File::create(config_file)?;
            file.write_all(EXAMPLE_CONFIG.as_bytes())?;

            config_builder
        };

        let config = config_builder.build()?;

        config
            .try_deserialize()
            .map_err(|e| eyre!("failed to deserialize: {}", e))
    }
}

pub fn example_config() -> &'static str {
    EXAMPLE_CONFIG
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Tls {
    pub enable: bool,

    pub cert_path: PathBuf,
    pub pkey_path: PathBuf,
}

impl Tls {
    pub fn certificates(&self) -> Result<Vec<rustls::Certificate>> {
        let cert_file = std::fs::File::open(&self.cert_path)
            .with_context(|| format!("tls.cert_path {:?} is missing", self.cert_path))?;
        let mut reader = std::io::BufReader::new(cert_file);
        let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
            .with_context(|| format!("tls.cert_path {:?} is invalid", self.cert_path))?
            .into_iter()
            .map(rustls::Certificate)
            .collect();

        if certs.is_empty() {
            bail!(
                "tls.cert_path {:?} must have at least one certificate",
                self.cert_path
            );
        }

        Ok(certs)
    }

    pub fn private_key(&self) -> Result<rustls::PrivateKey> {
        let pkey_file = std::fs::File::open(&self.pkey_path)
            .with_context(|| format!("tls.pkey_path {:?} is missing", self.pkey_path))?;
        let mut reader = std::io::BufReader::new(pkey_file);
        let keys = rustls_pemfile::pkcs8_private_keys(&mut reader)
            .with_context(|| format!("tls.pkey_path {:?} is not PKCS8-encoded", self.pkey_path))?;

        if keys.is_empty() {
            bail!(
                "tls.pkey_path {:?} must have at least one private key",
                self.pkey_path
            );
        }

        let key = rustls::PrivateKey(keys[0].clone());

        Ok(key)
    }
}
