mod behaviour;
pub mod display;
mod input;
mod stats;
mod sync;
mod time;

use ::time as time_lib;

use std::{
    collections::HashMap,
    io::prelude::*,
    path::{Path, PathBuf},
    str::FromStr,
};

use atuin_common::record::HostId;
use config::{
    builder::DefaultState, Config, ConfigBuilder, Environment, File as ConfigFile, FileFormat,
};
use eyre::{eyre, Context, Result};
use fs_err::{create_dir_all, File};
use parse_duration::parse;
use regex::RegexSet;
use semver::Version;
use serde::Deserialize;
use time_lib::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};
use uuid::Uuid;

pub use self::{
    behaviour::{ExitMode, FilterMode, SearchMode},
    display::{Display, Styles},
    input::{CursorStyle, KeymapMode, Keys, WordJumpMode},
    stats::{Dialect, Stats},
    sync::Sync,
    time::Timezone,
};

pub const HISTORY_PAGE_SIZE: i64 = 100;
pub const LAST_SYNC_FILENAME: &str = "last_sync_time";
pub const LAST_VERSION_CHECK_FILENAME: &str = "last_version_check_time";
pub const LATEST_VERSION_FILENAME: &str = "latest_version";
pub const HOST_ID_FILENAME: &str = "host_id";
static EXAMPLE_CONFIG: &str = include_str!("../config.toml");

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    // Behaviour
    pub exit_mode: ExitMode,
    pub filter_mode: FilterMode,
    pub filter_mode_shell_up_key_binding: Option<FilterMode>,
    pub search_mode: SearchMode,
    pub search_mode_shell_up_key_binding: Option<SearchMode>,

    // Display
    #[serde(default, flatten)]
    pub display: display::Settings,

    // Filters
    #[serde(with = "serde_regex", default = "RegexSet::empty")]
    pub cwd_filter: RegexSet,
    #[serde(with = "serde_regex", default = "RegexSet::empty")]
    pub history_filter: RegexSet,
    pub secrets_filter: bool,
    pub workspaces: bool,

    // Input
    pub enter_accept: bool,
    pub keymap_cursor: HashMap<String, CursorStyle>,
    pub keymap_mode: KeymapMode,
    pub keymap_mode_shell: KeymapMode,
    #[serde(default)]
    pub keys: Keys,
    pub shell_up_key_binding: bool,
    pub word_jump_mode: WordJumpMode,

    // Paths
    pub db_path: String,
    pub key_path: String,
    pub record_store_path: String,
    pub session_path: String,

    // Stats
    pub dialect: Dialect,
    #[serde(default)]
    pub stats: Stats,

    // Sync
    pub auto_sync: bool,
    pub sync_address: String,
    pub sync_frequency: String,
    #[serde(default)]
    pub sync: Sync,

    // Time
    pub timezone: Timezone,

    // Timeout
    pub local_timeout: f64,
    pub network_connect_timeout: u64,
    pub network_timeout: u64,

    pub update_check: bool,
    pub word_chars: String,
    pub scroll_context_lines: usize,
    pub history_format: String,
    pub ctrl_n_shortcuts: bool,

    // This is automatically loaded when settings is created. Do not set in
    // config! Keep secrets and settings apart.
    #[serde(skip)]
    pub session_token: String,
}

impl Settings {
    pub fn utc() -> Self {
        Self::builder()
            .expect("Could not build default")
            .set_override("timezone", "0")
            .expect("failed to override timezone with UTC")
            .build()
            .expect("Could not build config")
            .try_deserialize()
            .expect("Could not deserialize config")
    }

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
        Settings::save_to_data_dir(
            filename,
            OffsetDateTime::now_utc().format(&Rfc3339)?.as_str(),
        )?;

        Ok(())
    }

    fn load_time_from_file(filename: &str) -> Result<OffsetDateTime> {
        let value = Settings::read_from_data_dir(filename);

        match value {
            Some(v) => Ok(OffsetDateTime::parse(v.as_str(), &Rfc3339)?),
            None => Ok(OffsetDateTime::UNIX_EPOCH),
        }
    }

    pub fn save_sync_time() -> Result<()> {
        Settings::save_current_time(LAST_SYNC_FILENAME)
    }

    pub fn save_version_check_time() -> Result<()> {
        Settings::save_current_time(LAST_VERSION_CHECK_FILENAME)
    }

    pub fn last_sync() -> Result<OffsetDateTime> {
        Settings::load_time_from_file(LAST_SYNC_FILENAME)
    }

    pub fn last_version_check() -> Result<OffsetDateTime> {
        Settings::load_time_from_file(LAST_VERSION_CHECK_FILENAME)
    }

    pub fn host_id() -> Option<HostId> {
        let id = Settings::read_from_data_dir(HOST_ID_FILENAME);

        if let Some(id) = id {
            let parsed =
                Uuid::from_str(id.as_str()).expect("failed to parse host ID from local directory");
            return Some(HostId(parsed));
        }

        let uuid = atuin_common::utils::uuid_v7();

        Settings::save_to_data_dir(HOST_ID_FILENAME, uuid.as_simple().to_string().as_ref())
            .expect("Could not write host ID to data dir");

        Some(HostId(uuid))
    }

    pub fn should_sync(&self) -> Result<bool> {
        if !self.auto_sync || !PathBuf::from(self.session_path.as_str()).exists() {
            return Ok(false);
        }

        match parse(self.sync_frequency.as_str()) {
            Ok(d) => {
                let d = Duration::try_from(d).unwrap();
                Ok(OffsetDateTime::now_utc() - Settings::last_sync()? >= d)
            }
            Err(e) => Err(eyre!("failed to check sync: {}", e)),
        }
    }

    #[cfg(feature = "check-update")]
    fn needs_update_check(&self) -> Result<bool> {
        let last_check = Settings::last_version_check()?;
        let diff = OffsetDateTime::now_utc() - last_check;

        // Check a max of once per hour
        Ok(diff.whole_hours() >= 1)
    }

    #[cfg(feature = "check-update")]
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
    #[cfg(feature = "check-update")]
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

    #[cfg(not(feature = "check-update"))]
    pub async fn needs_update(&self) -> Option<Version> {
        None
    }

    pub fn builder() -> Result<ConfigBuilder<DefaultState>> {
        let data_dir = atuin_common::utils::data_dir();
        let db_path = data_dir.join("history.db");
        let record_store_path = data_dir.join("records.db");

        let key_path = data_dir.join("key");
        let session_path = data_dir.join("session");

        let builder = Config::builder();
        let builder = display::defaults(builder)?;

        Ok(builder
            .set_default("history_format", "{time}\t{command}\t{duration}")?
            .set_default("db_path", db_path.to_str())?
            .set_default("record_store_path", record_store_path.to_str())?
            .set_default("key_path", key_path.to_str())?
            .set_default("session_path", session_path.to_str())?
            .set_default("dialect", "us")?
            .set_default("timezone", "local")?
            .set_default("auto_sync", true)?
            .set_default("update_check", cfg!(feature = "check-update"))?
            .set_default("sync_address", "https://api.atuin.sh")?
            .set_default("sync_frequency", "10m")?
            .set_default("search_mode", "fuzzy")?
            .set_default("filter_mode", "global")?
            .set_default("exit_mode", "return-original")?
            .set_default("word_jump_mode", "emacs")?
            .set_default(
                "word_chars",
                "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            )?
            .set_default("scroll_context_lines", 1)?
            .set_default("shell_up_key_binding", false)?
            .set_default("session_token", "")?
            .set_default("workspaces", false)?
            .set_default("ctrl_n_shortcuts", false)?
            .set_default("secrets_filter", true)?
            .set_default("network_connect_timeout", 5)?
            .set_default("network_timeout", 30)?
            .set_default("local_timeout", 2.0)?
            // enter_accept defaults to false here, but true in the default config file. The dissonance is
            // intentional!
            // Existing users will get the default "False", so we don't mess with any potential
            // muscle memory.
            // New users will get the new default, that is more similar to what they are used to.
            .set_default("enter_accept", false)?
            .set_default("sync.records", false)?
            .set_default("keys.scroll_exits", true)?
            .set_default("keymap_mode", "emacs")?
            .set_default("keymap_mode_shell", "auto")?
            .set_default("keymap_cursor", HashMap::<String, String>::new())?
            .add_source(
                Environment::with_prefix("atuin")
                    .prefix_separator("_")
                    .separator("__"),
            ))
    }

    pub fn new() -> Result<Self> {
        let config_dir = atuin_common::utils::config_dir();
        let data_dir = atuin_common::utils::data_dir();

        create_dir_all(&config_dir)
            .wrap_err_with(|| format!("could not create dir {config_dir:?}"))?;

        create_dir_all(&data_dir).wrap_err_with(|| format!("could not create dir {data_dir:?}"))?;

        let mut config_file = if let Ok(p) = std::env::var("ATUIN_CONFIG_DIR") {
            PathBuf::from(p)
        } else {
            let mut config_file = PathBuf::new();
            config_file.push(config_dir);
            config_file
        };

        config_file.push("config.toml");

        let mut config_builder = Self::builder()?;

        config_builder = if config_file.exists() {
            config_builder.add_source(ConfigFile::new(
                config_file.to_str().unwrap(),
                FileFormat::Toml,
            ))
        } else {
            let mut file = File::create(config_file).wrap_err("could not create config file")?;
            file.write_all(EXAMPLE_CONFIG.as_bytes())
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

    pub fn example_config() -> &'static str {
        EXAMPLE_CONFIG
    }
}

impl Default for Settings {
    fn default() -> Self {
        // if this panics something is very wrong, as the default config
        // does not build or deserialize into the settings struct
        Self::builder()
            .expect("Could not build default")
            .build()
            .expect("Could not build config")
            .try_deserialize()
            .expect("Could not deserialize config")
    }
}
