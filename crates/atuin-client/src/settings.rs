use std::{
    collections::HashMap, convert::TryFrom, fmt, io::prelude::*, path::PathBuf, str::FromStr,
};

use atuin_common::record::HostId;
use clap::ValueEnum;
use config::{
    builder::DefaultState, Config, ConfigBuilder, Environment, File as ConfigFile, FileFormat,
};
use eyre::{bail, eyre, Context, Error, Result};
use fs_err::{create_dir_all, File};
use humantime::parse_duration;
use regex::RegexSet;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_with::DeserializeFromStr;
use time::{
    format_description::{well_known::Rfc3339, FormatItem},
    macros::format_description,
    OffsetDateTime, UtcOffset,
};
use uuid::Uuid;

pub const HISTORY_PAGE_SIZE: i64 = 100;
pub const LAST_SYNC_FILENAME: &str = "last_sync_time";
pub const LAST_VERSION_CHECK_FILENAME: &str = "last_version_check_time";
pub const LATEST_VERSION_FILENAME: &str = "latest_version";
pub const HOST_ID_FILENAME: &str = "host_id";
static EXAMPLE_CONFIG: &str = include_str!("../config.toml");

mod dotfiles;

#[derive(Clone, Debug, Deserialize, Copy, ValueEnum, PartialEq, Serialize)]
pub enum SearchMode {
    #[serde(rename = "prefix")]
    Prefix,

    #[serde(rename = "fulltext")]
    #[clap(aliases = &["fulltext"])]
    FullText,

    #[serde(rename = "fuzzy")]
    Fuzzy,

    #[serde(rename = "skim")]
    Skim,
}

impl SearchMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchMode::Prefix => "PREFIX",
            SearchMode::FullText => "FULLTXT",
            SearchMode::Fuzzy => "FUZZY",
            SearchMode::Skim => "SKIM",
        }
    }
    pub fn next(&self, settings: &Settings) -> Self {
        match self {
            SearchMode::Prefix => SearchMode::FullText,
            // if the user is using skim, we go to skim
            SearchMode::FullText if settings.search_mode == SearchMode::Skim => SearchMode::Skim,
            // otherwise fuzzy.
            SearchMode::FullText => SearchMode::Fuzzy,
            SearchMode::Fuzzy | SearchMode::Skim => SearchMode::Prefix,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Copy, PartialEq, Eq, ValueEnum, Serialize)]
pub enum FilterMode {
    #[serde(rename = "global")]
    Global = 0,

    #[serde(rename = "host")]
    Host = 1,

    #[serde(rename = "session")]
    Session = 2,

    #[serde(rename = "directory")]
    Directory = 3,

    #[serde(rename = "workspace")]
    Workspace = 4,
}

impl FilterMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            FilterMode::Global => "GLOBAL",
            FilterMode::Host => "HOST",
            FilterMode::Session => "SESSION",
            FilterMode::Directory => "DIRECTORY",
            FilterMode::Workspace => "WORKSPACE",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Copy, Serialize)]
pub enum ExitMode {
    #[serde(rename = "return-original")]
    ReturnOriginal,

    #[serde(rename = "return-query")]
    ReturnQuery,
}

// FIXME: Can use upstream Dialect enum if https://github.com/stevedonovan/chrono-english/pull/16 is merged
// FIXME: Above PR was merged, but dependency was changed to interim (fork of chrono-english) in the ... interim
#[derive(Clone, Debug, Deserialize, Copy, Serialize)]
pub enum Dialect {
    #[serde(rename = "us")]
    Us,

    #[serde(rename = "uk")]
    Uk,
}

impl From<Dialect> for interim::Dialect {
    fn from(d: Dialect) -> interim::Dialect {
        match d {
            Dialect::Uk => interim::Dialect::Uk,
            Dialect::Us => interim::Dialect::Us,
        }
    }
}

/// Type wrapper around `time::UtcOffset` to support a wider variety of timezone formats.
///
/// Note that the parsing of this struct needs to be done before starting any
/// multithreaded runtime, otherwise it will fail on most Unix systems.
///
/// See: https://github.com/atuinsh/atuin/pull/1517#discussion_r1447516426
#[derive(Clone, Copy, Debug, Eq, PartialEq, DeserializeFromStr, Serialize)]
pub struct Timezone(pub UtcOffset);
impl fmt::Display for Timezone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
/// format: <+|-><hour>[:<minute>[:<second>]]
static OFFSET_FMT: &[FormatItem<'_>] =
    format_description!("[offset_hour sign:mandatory padding:none][optional [:[offset_minute padding:none][optional [:[offset_second padding:none]]]]]");
impl FromStr for Timezone {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        // local timezone
        if matches!(s.to_lowercase().as_str(), "l" | "local") {
            // There have been some timezone issues, related to errors fetching it on some
            // platforms
            // Rather than fail to start, fallback to UTC. The user should still be able to specify
            // their timezone manually in the config file.
            let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
            return Ok(Self(offset));
        }

        if matches!(s.to_lowercase().as_str(), "0" | "utc") {
            let offset = UtcOffset::UTC;
            return Ok(Self(offset));
        }

        // offset from UTC
        if let Ok(offset) = UtcOffset::parse(s, OFFSET_FMT) {
            return Ok(Self(offset));
        }

        // IDEA: Currently named timezones are not supported, because the well-known crate
        // for this is `chrono_tz`, which is not really interoperable with the datetime crate
        // that we currently use - `time`. If ever we migrate to using `chrono`, this would
        // be a good feature to add.

        bail!(r#""{s}" is not a valid timezone spec"#)
    }
}

#[derive(Clone, Debug, Deserialize, Copy, Serialize)]
pub enum Style {
    #[serde(rename = "auto")]
    Auto,

    #[serde(rename = "full")]
    Full,

    #[serde(rename = "compact")]
    Compact,
}

#[derive(Clone, Debug, Deserialize, Copy, Serialize)]
pub enum WordJumpMode {
    #[serde(rename = "emacs")]
    Emacs,

    #[serde(rename = "subl")]
    Subl,
}

#[derive(Clone, Debug, Deserialize, Copy, PartialEq, Eq, ValueEnum, Serialize)]
pub enum KeymapMode {
    #[serde(rename = "emacs")]
    Emacs,

    #[serde(rename = "vim-normal")]
    VimNormal,

    #[serde(rename = "vim-insert")]
    VimInsert,

    #[serde(rename = "auto")]
    Auto,
}

impl KeymapMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeymapMode::Emacs => "EMACS",
            KeymapMode::VimNormal => "VIMNORMAL",
            KeymapMode::VimInsert => "VIMINSERT",
            KeymapMode::Auto => "AUTO",
        }
    }
}

// We want to translate the config to crossterm::cursor::SetCursorStyle, but
// the original type does not implement trait serde::Deserialize unfortunately.
// It seems impossible to implement Deserialize for external types when it is
// used in HashMap (https://stackoverflow.com/questions/67142663).  We instead
// define an adapter type.
#[derive(Clone, Debug, Deserialize, Copy, PartialEq, Eq, ValueEnum, Serialize)]
pub enum CursorStyle {
    #[serde(rename = "default")]
    DefaultUserShape,

    #[serde(rename = "blink-block")]
    BlinkingBlock,

    #[serde(rename = "steady-block")]
    SteadyBlock,

    #[serde(rename = "blink-underline")]
    BlinkingUnderScore,

    #[serde(rename = "steady-underline")]
    SteadyUnderScore,

    #[serde(rename = "blink-bar")]
    BlinkingBar,

    #[serde(rename = "steady-bar")]
    SteadyBar,
}

impl CursorStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            CursorStyle::DefaultUserShape => "DEFAULT",
            CursorStyle::BlinkingBlock => "BLINKBLOCK",
            CursorStyle::SteadyBlock => "STEADYBLOCK",
            CursorStyle::BlinkingUnderScore => "BLINKUNDERLINE",
            CursorStyle::SteadyUnderScore => "STEADYUNDERLINE",
            CursorStyle::BlinkingBar => "BLINKBAR",
            CursorStyle::SteadyBar => "STEADYBAR",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Stats {
    #[serde(default = "Stats::common_prefix_default")]
    pub common_prefix: Vec<String>, // sudo, etc. commands we want to strip off
    #[serde(default = "Stats::common_subcommands_default")]
    pub common_subcommands: Vec<String>, // kubectl, commands we should consider subcommands for
    #[serde(default = "Stats::ignored_commands_default")]
    pub ignored_commands: Vec<String>, // cd, ls, etc. commands we want to completely hide from stats
}

impl Stats {
    fn common_prefix_default() -> Vec<String> {
        vec!["sudo", "doas"].into_iter().map(String::from).collect()
    }

    fn common_subcommands_default() -> Vec<String> {
        vec![
            "apt",
            "cargo",
            "composer",
            "dnf",
            "docker",
            "git",
            "go",
            "ip",
            "kubectl",
            "nix",
            "nmcli",
            "npm",
            "pecl",
            "pnpm",
            "podman",
            "port",
            "systemctl",
            "tmux",
            "yarn",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    fn ignored_commands_default() -> Vec<String> {
        vec![]
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            common_prefix: Self::common_prefix_default(),
            common_subcommands: Self::common_subcommands_default(),
            ignored_commands: Self::ignored_commands_default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Default, Serialize)]
pub struct Sync {
    pub records: bool,
}

#[derive(Clone, Debug, Deserialize, Default, Serialize)]
pub struct Keys {
    pub scroll_exits: bool,
    pub prefix: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Preview {
    pub strategy: PreviewStrategy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Theme {
    /// Name of desired theme ("default" for base)
    pub name: String,

    /// Whether any available additional theme debug should be shown
    pub debug: Option<bool>,

    /// How many levels of parenthood will be traversed if needed
    pub max_depth: Option<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Daemon {
    /// Use the daemon to sync
    /// If enabled, requires a running daemon with `atuin daemon`
    #[serde(alias = "enable")]
    pub enabled: bool,

    /// The daemon will handle sync on an interval. How often to sync, in seconds.
    pub sync_frequency: u64,

    /// The path to the unix socket used by the daemon
    pub socket_path: String,

    /// Use a socket passed via systemd's socket activation protocol, instead of the path
    pub systemd_socket: bool,

    /// The port that should be used for TCP on non unix systems
    pub tcp_port: u64,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            strategy: PreviewStrategy::Auto,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "".to_string(),
            debug: None::<bool>,
            max_depth: Some(10),
        }
    }
}

impl Default for Daemon {
    fn default() -> Self {
        Self {
            enabled: false,
            sync_frequency: 300,
            socket_path: "".to_string(),
            systemd_socket: false,
            tcp_port: 8889,
        }
    }
}

// The preview height strategy also takes max_preview_height into account.
#[derive(Clone, Debug, Deserialize, Copy, PartialEq, Eq, ValueEnum, Serialize)]
pub enum PreviewStrategy {
    // Preview height is calculated for the length of the selected command.
    #[serde(rename = "auto")]
    Auto,

    // Preview height is calculated for the length of the longest command stored in the history.
    #[serde(rename = "static")]
    Static,

    // max_preview_height is used as fixed height.
    #[serde(rename = "fixed")]
    Fixed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    pub dialect: Dialect,
    pub timezone: Timezone,
    pub style: Style,
    pub auto_sync: bool,
    pub update_check: bool,
    pub sync_address: String,
    pub sync_frequency: String,
    pub db_path: String,
    pub record_store_path: String,
    pub key_path: String,
    pub session_path: String,
    pub search_mode: SearchMode,
    pub filter_mode: FilterMode,
    pub filter_mode_shell_up_key_binding: Option<FilterMode>,
    pub search_mode_shell_up_key_binding: Option<SearchMode>,
    pub shell_up_key_binding: bool,
    pub inline_height: u16,
    pub invert: bool,
    pub show_preview: bool,
    pub max_preview_height: u16,
    pub show_help: bool,
    pub show_tabs: bool,
    pub auto_hide_height: u16,
    pub exit_mode: ExitMode,
    pub keymap_mode: KeymapMode,
    pub keymap_mode_shell: KeymapMode,
    pub keymap_cursor: HashMap<String, CursorStyle>,
    pub word_jump_mode: WordJumpMode,
    pub word_chars: String,
    pub scroll_context_lines: usize,
    pub history_format: String,
    pub prefers_reduced_motion: bool,
    pub store_failed: bool,

    #[serde(with = "serde_regex", default = "RegexSet::empty", skip_serializing)]
    pub history_filter: RegexSet,

    #[serde(with = "serde_regex", default = "RegexSet::empty", skip_serializing)]
    pub cwd_filter: RegexSet,

    pub secrets_filter: bool,
    pub workspaces: bool,
    pub ctrl_n_shortcuts: bool,

    pub network_connect_timeout: u64,
    pub network_timeout: u64,
    pub local_timeout: f64,
    pub enter_accept: bool,
    pub smart_sort: bool,

    pub exit_with_backspace: bool,
    pub exit_with_space: bool,
    pub exit_with_home: bool,
    pub exit_with_cursor_left: bool,
    pub exit_positions_cursor: bool,

    #[serde(default)]
    pub stats: Stats,

    #[serde(default)]
    pub sync: Sync,

    #[serde(default)]
    pub keys: Keys,

    #[serde(default)]
    pub preview: Preview,

    #[serde(default)]
    pub dotfiles: dotfiles::Settings,

    #[serde(default)]
    pub daemon: Daemon,

    #[serde(default)]
    pub theme: Theme,
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

        if self.sync_frequency == "0" {
            return Ok(true);
        }

        match parse_duration(self.sync_frequency.as_str()) {
            Ok(d) => {
                let d = time::Duration::try_from(d).unwrap();
                Ok(OffsetDateTime::now_utc() - Settings::last_sync()? >= d)
            }
            Err(e) => Err(eyre!("failed to check sync: {}", e)),
        }
    }

    pub fn logged_in(&self) -> bool {
        let session_path = self.session_path.as_str();

        PathBuf::from(session_path).exists()
    }

    pub fn session_token(&self) -> Result<String> {
        if !self.logged_in() {
            return Err(eyre!("Tried to load session; not logged in"));
        }

        let session_path = self.session_path.as_str();
        Ok(fs_err::read_to_string(session_path)?)
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
            let version = tokio::task::spawn_blocking(|| {
                Settings::read_from_data_dir(LATEST_VERSION_FILENAME)
            })
            .await
            .expect("file task panicked");

            let version = match version {
                Some(v) => Version::parse(&v).unwrap_or(current),
                None => current,
            };

            return Ok(version);
        }

        #[cfg(feature = "sync")]
        let latest = crate::api_client::latest_version().await.unwrap_or(current);

        #[cfg(not(feature = "sync"))]
        let latest = current;

        let latest_encoded = latest.to_string();
        tokio::task::spawn_blocking(move || {
            Settings::save_version_check_time()?;
            Settings::save_to_data_dir(LATEST_VERSION_FILENAME, &latest_encoded)?;
            Ok::<(), eyre::Report>(())
        })
        .await
        .expect("file task panicked")?;

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
        let socket_path = atuin_common::utils::runtime_dir().join("atuin.sock");

        let key_path = data_dir.join("key");
        let session_path = data_dir.join("session");

        Ok(Config::builder()
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
            .set_default("style", "compact")?
            .set_default("inline_height", 40)?
            .set_default("show_preview", true)?
            .set_default("preview.strategy", "auto")?
            .set_default("max_preview_height", 4)?
            .set_default("show_help", true)?
            .set_default("show_tabs", true)?
            .set_default("auto_hide_height", 8)?
            .set_default("invert", false)?
            .set_default("exit_mode", "return-original")?
            .set_default("word_jump_mode", "emacs")?
            .set_default(
                "word_chars",
                "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            )?
            .set_default("scroll_context_lines", 1)?
            .set_default("shell_up_key_binding", false)?
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
            .set_default("sync.records", true)?
            .set_default("keys.scroll_exits", true)?
            .set_default("keys.prefix", "a")?
            .set_default("keymap_mode", "emacs")?
            .set_default("keymap_mode_shell", "auto")?
            .set_default("keymap_cursor", HashMap::<String, String>::new())?
            .set_default("smart_sort", false)?
            .set_default("exit_with_backspace", false)?
            .set_default("exit_with_space", false)?
            .set_default("exit_with_home", false)?
            .set_default("exit_with_cursor_left", false)?
            .set_default("exit_positions_cursor", false)?
            .set_default("store_failed", true)?
            .set_default("daemon.sync_frequency", 300)?
            .set_default("daemon.enabled", false)?
            .set_default("daemon.socket_path", socket_path.to_str())?
            .set_default("daemon.systemd_socket", false)?
            .set_default("daemon.tcp_port", 8889)?
            .set_default("theme.name", "default")?
            .set_default("theme.debug", None::<bool>)?
            .set_default(
                "prefers_reduced_motion",
                std::env::var("NO_MOTION")
                    .ok()
                    .map(|_| config::Value::new(None, config::ValueKind::Boolean(true)))
                    .unwrap_or_else(|| config::Value::new(None, config::ValueKind::Boolean(false))),
            )?
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

#[cfg(test)]
pub(crate) fn test_local_timeout() -> f64 {
    std::env::var("ATUIN_TEST_LOCAL_TIMEOUT")
        .ok()
        .and_then(|x| x.parse().ok())
        // this hardcoded value should be replaced by a simple way to get the
        // default local_timeout of Settings if possible
        .unwrap_or(2.0)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use eyre::Result;

    use super::Timezone;

    #[test]
    fn can_parse_offset_timezone_spec() -> Result<()> {
        assert_eq!(Timezone::from_str("+02")?.0.as_hms(), (2, 0, 0));
        assert_eq!(Timezone::from_str("-04")?.0.as_hms(), (-4, 0, 0));
        assert_eq!(Timezone::from_str("+05:30")?.0.as_hms(), (5, 30, 0));
        assert_eq!(Timezone::from_str("-09:30")?.0.as_hms(), (-9, -30, 0));

        // single digit hours are allowed
        assert_eq!(Timezone::from_str("+2")?.0.as_hms(), (2, 0, 0));
        assert_eq!(Timezone::from_str("-4")?.0.as_hms(), (-4, 0, 0));
        assert_eq!(Timezone::from_str("+5:30")?.0.as_hms(), (5, 30, 0));
        assert_eq!(Timezone::from_str("-9:30")?.0.as_hms(), (-9, -30, 0));

        // fully qualified form
        assert_eq!(Timezone::from_str("+09:30:00")?.0.as_hms(), (9, 30, 0));
        assert_eq!(Timezone::from_str("-09:30:00")?.0.as_hms(), (-9, -30, 0));

        // these offsets don't really exist but are supported anyway
        assert_eq!(Timezone::from_str("+0:5")?.0.as_hms(), (0, 5, 0));
        assert_eq!(Timezone::from_str("-0:5")?.0.as_hms(), (0, -5, 0));
        assert_eq!(Timezone::from_str("+01:23:45")?.0.as_hms(), (1, 23, 45));
        assert_eq!(Timezone::from_str("-01:23:45")?.0.as_hms(), (-1, -23, -45));

        // require a leading sign for clarity
        assert!(Timezone::from_str("5").is_err());
        assert!(Timezone::from_str("10:30").is_err());

        Ok(())
    }
}
