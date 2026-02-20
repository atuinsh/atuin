use std::{collections::HashMap, fmt, io::prelude::*, path::PathBuf, str::FromStr, sync::OnceLock};
use tokio::sync::OnceCell;

use atuin_common::record::HostId;
use atuin_common::utils;
use clap::ValueEnum;
use config::{
    Config, ConfigBuilder, Environment, File as ConfigFile, FileFormat, builder::DefaultState,
};
use eyre::{Context, Error, Result, bail, eyre};
use fs_err::{File, create_dir_all};
use humantime::parse_duration;
use regex::RegexSet;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_with::DeserializeFromStr;
use time::{OffsetDateTime, UtcOffset, format_description::FormatItem, macros::format_description};

pub const HISTORY_PAGE_SIZE: i64 = 100;
static EXAMPLE_CONFIG: &str = include_str!("../config.toml");

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static META_CONFIG: OnceLock<(String, f64)> = OnceLock::new();
static META_STORE: OnceCell<crate::meta::MetaStore> = OnceCell::const_new();

mod dotfiles;
mod kv;
pub(crate) mod meta;
mod scripts;

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

    #[serde(rename = "session-preload")]
    SessionPreload = 5,
}

impl FilterMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            FilterMode::Global => "GLOBAL",
            FilterMode::Host => "HOST",
            FilterMode::Session => "SESSION",
            FilterMode::Directory => "DIRECTORY",
            FilterMode::Workspace => "WORKSPACE",
            FilterMode::SessionPreload => "SESSION+",
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
/// See: <https://github.com/atuinsh/atuin/pull/1517#discussion_r1447516426>
#[derive(Clone, Copy, Debug, Eq, PartialEq, DeserializeFromStr, Serialize)]
pub struct Timezone(pub UtcOffset);
impl fmt::Display for Timezone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
/// format: <+|-><hour>[:<minute>[:<second>]]
static OFFSET_FMT: &[FormatItem<'_>] = format_description!(
    "[offset_hour sign:mandatory padding:none][optional [:[offset_minute padding:none][optional [:[offset_second padding:none]]]]]"
);
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
            "dotnet",
            "git",
            "go",
            "ip",
            "jj",
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
    pub exit_past_line_start: bool,
    pub accept_past_line_end: bool,
    pub accept_past_line_start: bool,
    pub accept_with_backspace: bool,
    pub prefix: String,
}

impl Keys {
    /// The standard default values for all `[keys]` options.
    /// These match the config defaults set in `builder_with_data_dir()`.
    pub fn standard_defaults() -> Self {
        Keys {
            scroll_exits: true,
            exit_past_line_start: true,
            accept_past_line_end: true,
            accept_past_line_start: false,
            accept_with_backspace: false,
            prefix: "a".to_string(),
        }
    }

    /// Returns true if any value differs from the standard defaults.
    pub fn has_non_default_values(&self) -> bool {
        let d = Self::standard_defaults();
        self.scroll_exits != d.scroll_exits
            || self.exit_past_line_start != d.exit_past_line_start
            || self.accept_past_line_end != d.accept_past_line_end
            || self.accept_past_line_start != d.accept_past_line_start
            || self.accept_with_backspace != d.accept_with_backspace
            || self.prefix != d.prefix
    }
}

/// A single rule within a conditional keybinding config.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KeyRuleConfig {
    /// Optional condition expression (e.g. "cursor-at-start", "input-empty && no-results").
    /// If absent, the rule always matches.
    #[serde(default)]
    pub when: Option<String>,
    /// The action to perform (e.g. "exit", "cursor-left", "accept").
    pub action: String,
}

/// A keybinding config value: either a simple action string or an ordered list of conditional rules.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum KeyBindingConfig {
    /// Simple unconditional binding: `"ctrl-c" = "return-original"`
    Simple(String),
    /// Conditional binding: `"left" = [{ when = "cursor-at-start", action = "exit" }, { action = "cursor-left" }]`
    Rules(Vec<KeyRuleConfig>),
}

/// User-facing keymap configuration. Each mode maps key strings to bindings.
/// Keys present here override the defaults for that key; unmentioned keys keep defaults.
#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct KeymapConfig {
    #[serde(default)]
    pub emacs: HashMap<String, KeyBindingConfig>,
    #[serde(default, rename = "vim-normal")]
    pub vim_normal: HashMap<String, KeyBindingConfig>,
    #[serde(default, rename = "vim-insert")]
    pub vim_insert: HashMap<String, KeyBindingConfig>,
    #[serde(default)]
    pub inspector: HashMap<String, KeyBindingConfig>,
    #[serde(default)]
    pub prefix: HashMap<String, KeyBindingConfig>,
}

impl KeymapConfig {
    /// Returns true if no keybinding overrides are configured in any mode.
    pub fn is_empty(&self) -> bool {
        self.emacs.is_empty()
            && self.vim_normal.is_empty()
            && self.vim_insert.is_empty()
            && self.inspector.is_empty()
            && self.prefix.is_empty()
    }
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
    /// If enabled, history hooks are routed through the daemon.
    #[serde(alias = "enable")]
    pub enabled: bool,

    /// Automatically start and manage a local daemon when needed.
    pub autostart: bool,

    /// The daemon will handle sync on an interval. How often to sync, in seconds.
    pub sync_frequency: u64,

    /// The path to the unix socket used by the daemon
    pub socket_path: String,

    /// Path to the daemon pidfile used for process coordination.
    pub pidfile_path: String,

    /// Use a socket passed via systemd's socket activation protocol, instead of the path
    pub systemd_socket: bool,

    /// The port that should be used for TCP on non unix systems
    pub tcp_port: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Search {
    /// The list of enabled filter modes, in order of priority.
    pub filters: Vec<FilterMode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Tmux {
    /// Enable using atuin with tmux popup (tmux >= 3.2)
    pub enabled: bool,

    /// Width of the tmux popup (percentage)
    pub width: String,

    /// Height of the tmux popup (percentage)
    pub height: String,
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
pub struct Ai {
    /// The address of the Atuin AI endpoint. Used for AI features like command generation.
    /// Only necessary for custom AI endpoints.
    pub ai_endpoint: Option<String>,

    /// The API token for the Atuin AI endpoint. Used for AI features like command generation.
    /// Only necessary for custom AI endpoints.
    pub ai_api_token: Option<String>,

    /// Whether or not to send the current working directory to the AI endpoint.
    pub send_cwd: bool,
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
            autostart: false,
            sync_frequency: 300,
            socket_path: "".to_string(),
            pidfile_path: "".to_string(),
            systemd_socket: false,
            tcp_port: 8889,
        }
    }
}

impl Default for Search {
    fn default() -> Self {
        Self {
            filters: vec![
                FilterMode::Global,
                FilterMode::Host,
                FilterMode::Session,
                FilterMode::SessionPreload,
                FilterMode::Workspace,
                FilterMode::Directory,
            ],
        }
    }
}

impl Default for Tmux {
    fn default() -> Self {
        Self {
            enabled: false,
            width: "80%".to_string(),
            height: "60%".to_string(),
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

/// Column types available for the interactive search UI.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UiColumnType {
    /// Command execution duration (e.g., "123ms")
    Duration,
    /// Relative time since execution (e.g., "59s ago")
    Time,
    /// Absolute timestamp (e.g., "2025-01-22 14:35")
    Datetime,
    /// Working directory
    Directory,
    /// Hostname
    Host,
    /// Username
    User,
    /// Exit code
    Exit,
    /// The command itself (should be last, expands to fill)
    Command,
}

impl UiColumnType {
    /// Returns the default width for this column type (in characters).
    /// The Command column returns 0 as it expands to fill remaining space.
    pub fn default_width(&self) -> u16 {
        match self {
            UiColumnType::Duration => 5,  // "814ms"
            UiColumnType::Time => 9,      // "459ms ago"
            UiColumnType::Datetime => 16, // "2025-01-22 14:35"
            UiColumnType::Directory => 20,
            UiColumnType::Host => 15,
            UiColumnType::User => 10,
            UiColumnType::Exit => {
                if cfg!(windows) {
                    11 // 32-bit integer on Windows: "-1978335212"
                } else {
                    3 // Usually a byte on Unix
                }
            }
            UiColumnType::Command => 0, // Expands to fill
        }
    }
}

/// A column configuration with type and optional custom width.
/// Can be specified as just a string (uses default width) or as an object with type and width.
#[derive(Clone, Debug, Serialize)]
pub struct UiColumn {
    pub column_type: UiColumnType,
    pub width: u16,
    /// If true, this column expands to fill remaining space. Only one column should expand.
    pub expand: bool,
}

impl UiColumn {
    pub fn new(column_type: UiColumnType) -> Self {
        Self {
            width: column_type.default_width(),
            expand: column_type == UiColumnType::Command,
            column_type,
        }
    }

    pub fn with_width(column_type: UiColumnType, width: u16) -> Self {
        Self {
            column_type,
            width,
            expand: column_type == UiColumnType::Command,
        }
    }
}

// Custom deserialize to handle both string and object formats:
// "duration" or { type = "duration", width = 8, expand = true }
impl<'de> serde::Deserialize<'de> for UiColumn {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};

        struct UiColumnVisitor;

        impl<'de> Visitor<'de> for UiColumnVisitor {
            type Value = UiColumn;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a column type string or an object with 'type' and optional 'width'/'expand'",
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<UiColumn, E>
            where
                E: de::Error,
            {
                let column_type: UiColumnType =
                    serde::Deserialize::deserialize(serde::de::value::StrDeserializer::new(value))?;
                Ok(UiColumn::new(column_type))
            }

            fn visit_map<M>(self, mut map: M) -> Result<UiColumn, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut column_type: Option<UiColumnType> = None;
                let mut width: Option<u16> = None;
                let mut expand: Option<bool> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "type" => {
                            column_type = Some(map.next_value()?);
                        }
                        "width" => {
                            width = Some(map.next_value()?);
                        }
                        "expand" => {
                            expand = Some(map.next_value()?);
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let column_type = column_type.ok_or_else(|| de::Error::missing_field("type"))?;
                let width = width.unwrap_or_else(|| column_type.default_width());
                let expand = expand.unwrap_or(column_type == UiColumnType::Command);
                Ok(UiColumn {
                    column_type,
                    width,
                    expand,
                })
            }
        }

        deserializer.deserialize_any(UiColumnVisitor)
    }
}

/// UI-specific settings for the interactive search.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Ui {
    /// Columns to display in interactive search, from left to right.
    /// The indicator column (" > ") is always shown first implicitly.
    /// The "command" column should be last as it expands to fill remaining space.
    /// Can be simple strings or objects with type and width.
    #[serde(default = "Ui::default_columns")]
    pub columns: Vec<UiColumn>,
}

impl Ui {
    fn default_columns() -> Vec<UiColumn> {
        vec![
            UiColumn::new(UiColumnType::Duration),
            UiColumn::new(UiColumnType::Time),
            UiColumn::new(UiColumnType::Command),
        ]
    }

    /// Validate the UI configuration.
    /// Returns an error if more than one column has expand = true.
    pub fn validate(&self) -> Result<()> {
        let expand_count = self.columns.iter().filter(|c| c.expand).count();
        if expand_count > 1 {
            bail!(
                "Only one column can have expand = true, but {} columns are set to expand",
                expand_count
            );
        }
        Ok(())
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            columns: Self::default_columns(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    pub data_dir: Option<String>,
    pub dialect: Dialect,
    pub timezone: Timezone,
    pub style: Style,
    pub auto_sync: bool,
    pub update_check: bool,

    /// The address of the Atuin Hub. Used for Hub-specific features like AI.
    pub hub_address: String,

    /// The sync address for atuin.
    pub sync_address: String,

    pub sync_frequency: String,
    pub db_path: String,
    pub record_store_path: String,
    pub key_path: String,
    pub search_mode: SearchMode,
    pub filter_mode: Option<FilterMode>,
    pub filter_mode_shell_up_key_binding: Option<FilterMode>,
    pub search_mode_shell_up_key_binding: Option<SearchMode>,
    pub shell_up_key_binding: bool,
    pub inline_height: u16,
    pub inline_height_shell_up_key_binding: Option<u16>,
    pub invert: bool,
    pub show_preview: bool,
    pub max_preview_height: u16,
    pub show_help: bool,
    pub show_tabs: bool,
    pub show_numeric_shortcuts: bool,
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
    pub command_chaining: bool,

    #[serde(default)]
    pub stats: Stats,

    #[serde(default)]
    pub sync: Sync,

    #[serde(default)]
    pub keys: Keys,

    #[serde(default)]
    pub keymap: KeymapConfig,

    #[serde(default)]
    pub preview: Preview,

    #[serde(default)]
    pub dotfiles: dotfiles::Settings,

    #[serde(default)]
    pub daemon: Daemon,

    #[serde(default)]
    pub search: Search,

    #[serde(default)]
    pub theme: Theme,

    #[serde(default)]
    pub ui: Ui,

    #[serde(default)]
    pub scripts: scripts::Settings,

    #[serde(default)]
    pub kv: kv::Settings,

    #[serde(default)]
    pub tmux: Tmux,

    #[serde(default)]
    pub meta: meta::Settings,

    #[serde(default)]
    pub ai: Ai,
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

    pub(crate) fn effective_data_dir() -> PathBuf {
        DATA_DIR
            .get()
            .cloned()
            .unwrap_or_else(atuin_common::utils::data_dir)
    }

    // -- Meta store: lazily initialized on first access --

    pub async fn meta_store() -> Result<&'static crate::meta::MetaStore> {
        META_STORE
            .get_or_try_init(|| async {
                let (db_path, timeout) = META_CONFIG.get().ok_or_else(|| {
                    eyre!("meta store config not set — Settings::new() has not been called")
                })?;
                crate::meta::MetaStore::new(db_path, *timeout).await
            })
            .await
    }

    pub async fn host_id() -> Result<HostId> {
        Self::meta_store().await?.host_id().await
    }

    pub async fn last_sync() -> Result<OffsetDateTime> {
        Self::meta_store().await?.last_sync().await
    }

    pub async fn save_sync_time() -> Result<()> {
        Self::meta_store().await?.save_sync_time().await
    }

    pub async fn last_version_check() -> Result<OffsetDateTime> {
        Self::meta_store().await?.last_version_check().await
    }

    pub async fn save_version_check_time() -> Result<()> {
        Self::meta_store().await?.save_version_check_time().await
    }

    pub async fn should_sync(&self) -> Result<bool> {
        if !self.auto_sync || !Self::meta_store().await?.logged_in().await? {
            return Ok(false);
        }

        if self.sync_frequency == "0" {
            return Ok(true);
        }

        match parse_duration(self.sync_frequency.as_str()) {
            Ok(d) => {
                let d = time::Duration::try_from(d)?;
                Ok(OffsetDateTime::now_utc() - Settings::last_sync().await? >= d)
            }
            Err(e) => Err(eyre!("failed to check sync: {}", e)),
        }
    }

    pub async fn logged_in(&self) -> Result<bool> {
        Self::meta_store().await?.logged_in().await
    }

    pub async fn session_token(&self) -> Result<String> {
        match Self::meta_store().await?.session_token().await? {
            Some(token) => Ok(token),
            None => Err(eyre!("Tried to load session; not logged in")),
        }
    }

    #[cfg(feature = "check-update")]
    async fn needs_update_check(&self) -> Result<bool> {
        let last_check = Settings::last_version_check().await?;
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

        if !self.needs_update_check().await? {
            let meta = Self::meta_store().await?;
            let version = match meta.latest_version().await? {
                Some(v) => Version::parse(&v).unwrap_or(current),
                None => current,
            };

            return Ok(version);
        }

        #[cfg(feature = "sync")]
        let latest = crate::api_client::latest_version().await.unwrap_or(current);

        #[cfg(not(feature = "sync"))]
        let latest = current;

        let meta = Self::meta_store().await?;
        Settings::save_version_check_time().await?;
        meta.save_latest_version(&latest.to_string()).await?;

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

    pub fn default_filter_mode(&self, git_root: bool) -> FilterMode {
        self.filter_mode
            .filter(|x| self.search.filters.contains(x))
            .or_else(|| {
                self.search
                    .filters
                    .iter()
                    .find(|x| match (x, git_root, self.workspaces) {
                        (FilterMode::Workspace, true, true) => true,
                        (FilterMode::Workspace, _, _) => false,
                        (_, _, _) => true,
                    })
                    .copied()
            })
            .unwrap_or(FilterMode::Global)
    }

    #[cfg(not(feature = "check-update"))]
    pub async fn needs_update(&self) -> Option<Version> {
        None
    }

    pub fn builder() -> Result<ConfigBuilder<DefaultState>> {
        Self::builder_with_data_dir(&atuin_common::utils::data_dir())
    }

    fn builder_with_data_dir(data_dir: &std::path::Path) -> Result<ConfigBuilder<DefaultState>> {
        let db_path = data_dir.join("history.db");
        let record_store_path = data_dir.join("records.db");
        let kv_path = data_dir.join("kv.db");
        let scripts_path = data_dir.join("scripts.db");
        let socket_path = atuin_common::utils::runtime_dir().join("atuin.sock");
        let pidfile_path = data_dir.join("atuin-daemon.pid");

        let key_path = data_dir.join("key");
        let meta_path = data_dir.join("meta.db");

        Ok(Config::builder()
            .set_default("history_format", "{time}\t{command}\t{duration}")?
            .set_default("db_path", db_path.to_str())?
            .set_default("record_store_path", record_store_path.to_str())?
            .set_default("key_path", key_path.to_str())?
            .set_default("dialect", "us")?
            .set_default("timezone", "local")?
            .set_default("auto_sync", true)?
            .set_default("update_check", cfg!(feature = "check-update"))?
            .set_default("hub_address", "https://hub.atuin.sh")?
            .set_default("sync_address", "https://api.atuin.sh")?
            .set_default("sync_frequency", "5m")?
            .set_default("search_mode", "fuzzy")?
            .set_default("filter_mode", None::<String>)?
            .set_default("style", "compact")?
            .set_default("inline_height", 40)?
            .set_default("show_preview", true)?
            .set_default("preview.strategy", "auto")?
            .set_default("max_preview_height", 4)?
            .set_default("show_help", true)?
            .set_default("show_tabs", true)?
            .set_default("show_numeric_shortcuts", true)?
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
            .set_default("keys.accept_past_line_end", true)?
            .set_default("keys.exit_past_line_start", true)?
            .set_default("keys.accept_past_line_start", false)?
            .set_default("keys.accept_with_backspace", false)?
            .set_default("keys.prefix", "a")?
            .set_default("keymap_mode", "emacs")?
            .set_default("keymap_mode_shell", "auto")?
            .set_default("keymap_cursor", HashMap::<String, String>::new())?
            .set_default("smart_sort", false)?
            .set_default("command_chaining", false)?
            .set_default("store_failed", true)?
            .set_default("daemon.sync_frequency", 300)?
            .set_default("daemon.enabled", false)?
            .set_default("daemon.autostart", false)?
            .set_default("daemon.socket_path", socket_path.to_str())?
            .set_default("daemon.pidfile_path", pidfile_path.to_str())?
            .set_default("daemon.systemd_socket", false)?
            .set_default("daemon.tcp_port", 8889)?
            .set_default("kv.db_path", kv_path.to_str())?
            .set_default("scripts.db_path", scripts_path.to_str())?
            .set_default("meta.db_path", meta_path.to_str())?
            .set_default(
                "search.filters",
                vec![
                    "global",
                    "host",
                    "session",
                    "workspace",
                    "directory",
                    "session-preload",
                ],
            )?
            .set_default("theme.name", "default")?
            .set_default("theme.debug", None::<bool>)?
            .set_default("tmux.enabled", false)?
            .set_default("tmux.width", "80%")?
            .set_default("tmux.height", "60%")?
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

        create_dir_all(&config_dir)
            .wrap_err_with(|| format!("could not create dir {config_dir:?}"))?;

        let mut config_file = if let Ok(p) = std::env::var("ATUIN_CONFIG_DIR") {
            PathBuf::from(p)
        } else {
            let mut config_file = PathBuf::new();
            config_file.push(config_dir);
            config_file
        };

        config_file.push("config.toml");

        // extract data_dir first so we can use it as the base for other path defaults
        let effective_data_dir = if config_file.exists() {
            #[derive(Deserialize, Default)]
            struct DataDirOnly {
                data_dir: Option<String>,
            }

            let config_file_str = config_file
                .to_str()
                .ok_or_else(|| eyre!("config file path is not valid UTF-8"))?;

            let partial_config = Config::builder()
                .add_source(ConfigFile::new(config_file_str, FileFormat::Toml))
                .add_source(
                    Environment::with_prefix("atuin")
                        .prefix_separator("_")
                        .separator("__"),
                )
                .build()
                .ok();

            let custom_data_dir = partial_config
                .and_then(|c| c.try_deserialize::<DataDirOnly>().ok())
                .and_then(|d| d.data_dir);

            match custom_data_dir {
                Some(dir) => {
                    let expanded = shellexpand::full(&dir)
                        .map_err(|e| eyre!("failed to expand data_dir path: {}", e))?;
                    PathBuf::from(expanded.as_ref())
                }
                None => atuin_common::utils::data_dir(),
            }
        } else {
            atuin_common::utils::data_dir()
        };

        DATA_DIR.set(effective_data_dir.clone()).ok();

        create_dir_all(&effective_data_dir)
            .wrap_err_with(|| format!("could not create dir {effective_data_dir:?}"))?;

        let mut config_builder = Self::builder_with_data_dir(&effective_data_dir)?;

        config_builder = if config_file.exists() {
            let config_file_str = config_file
                .to_str()
                .ok_or_else(|| eyre!("config file path is not valid UTF-8"))?;
            config_builder.add_source(ConfigFile::new(config_file_str, FileFormat::Toml))
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
        settings.db_path = Self::expand_path(settings.db_path)?;
        settings.record_store_path = Self::expand_path(settings.record_store_path)?;
        settings.key_path = Self::expand_path(settings.key_path)?;
        settings.daemon.socket_path = Self::expand_path(settings.daemon.socket_path)?;
        settings.daemon.pidfile_path = Self::expand_path(settings.daemon.pidfile_path)?;

        // Validate UI settings
        settings.ui.validate()?;

        // Register meta store config for lazy initialization on first access
        META_CONFIG
            .set((settings.meta.db_path.clone(), settings.local_timeout))
            .ok();

        Ok(settings)
    }

    fn expand_path(path: String) -> Result<String> {
        shellexpand::full(&path)
            .map(|p| p.to_string())
            .map_err(|e| eyre!("failed to expand path: {}", e))
    }

    pub fn example_config() -> &'static str {
        EXAMPLE_CONFIG
    }

    pub fn paths_ok(&self) -> bool {
        let paths = [
            &self.db_path,
            &self.record_store_path,
            &self.key_path,
            &self.meta.db_path,
        ];
        paths.iter().all(|p| !utils::broken_symlink(p))
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

    #[test]
    fn can_choose_workspace_filters_when_in_git_context() -> Result<()> {
        let mut settings = super::Settings::default();
        settings.search.filters = vec![
            super::FilterMode::Workspace,
            super::FilterMode::Host,
            super::FilterMode::Directory,
            super::FilterMode::Session,
            super::FilterMode::Global,
        ];
        settings.workspaces = true;

        assert_eq!(
            settings.default_filter_mode(true),
            super::FilterMode::Workspace,
        );

        Ok(())
    }

    #[test]
    fn wont_choose_workspace_filters_when_not_in_git_context() -> Result<()> {
        let mut settings = super::Settings::default();
        settings.search.filters = vec![
            super::FilterMode::Workspace,
            super::FilterMode::Host,
            super::FilterMode::Directory,
            super::FilterMode::Session,
            super::FilterMode::Global,
        ];
        settings.workspaces = true;

        assert_eq!(settings.default_filter_mode(false), super::FilterMode::Host,);

        Ok(())
    }

    #[test]
    fn wont_choose_workspace_filters_when_workspaces_disabled() -> Result<()> {
        let mut settings = super::Settings::default();
        settings.search.filters = vec![
            super::FilterMode::Workspace,
            super::FilterMode::Host,
            super::FilterMode::Directory,
            super::FilterMode::Session,
            super::FilterMode::Global,
        ];
        settings.workspaces = false;

        assert_eq!(settings.default_filter_mode(true), super::FilterMode::Host,);

        Ok(())
    }

    #[test]
    fn builder_with_data_dir_uses_custom_paths() -> Result<()> {
        use std::path::PathBuf;

        let custom_dir = PathBuf::from("/custom/data/dir");
        let builder = super::Settings::builder_with_data_dir(&custom_dir)?;
        let config = builder.build()?;

        let db_path: String = config.get("db_path")?;
        let key_path: String = config.get("key_path")?;
        let record_store_path: String = config.get("record_store_path")?;
        let kv_db_path: String = config.get("kv.db_path")?;
        let scripts_db_path: String = config.get("scripts.db_path")?;
        let meta_db_path: String = config.get("meta.db_path")?;
        let daemon_socket_path: String = config.get("daemon.socket_path")?;
        let daemon_pidfile_path: String = config.get("daemon.pidfile_path")?;
        let daemon_autostart: bool = config.get("daemon.autostart")?;

        assert_eq!(db_path, custom_dir.join("history.db").to_str().unwrap());
        assert_eq!(key_path, custom_dir.join("key").to_str().unwrap());
        assert_eq!(
            record_store_path,
            custom_dir.join("records.db").to_str().unwrap()
        );
        assert_eq!(kv_db_path, custom_dir.join("kv.db").to_str().unwrap());
        assert_eq!(
            scripts_db_path,
            custom_dir.join("scripts.db").to_str().unwrap()
        );
        assert_eq!(meta_db_path, custom_dir.join("meta.db").to_str().unwrap());
        assert_eq!(
            daemon_socket_path,
            atuin_common::utils::runtime_dir()
                .join("atuin.sock")
                .to_str()
                .unwrap()
        );
        assert_eq!(
            daemon_pidfile_path,
            custom_dir.join("atuin-daemon.pid").to_str().unwrap()
        );
        assert!(!daemon_autostart);

        Ok(())
    }

    #[test]
    fn effective_data_dir_returns_default_when_not_set() {
        let effective = super::Settings::effective_data_dir();
        let default = atuin_common::utils::data_dir();

        assert!(effective.to_str().is_some());
        assert!(effective.ends_with("atuin") || effective == default);
    }

    #[test]
    fn keymap_config_deserializes_simple_binding() {
        let json = r#"{"emacs": {"ctrl-c": "exit"}}"#;
        let config: super::KeymapConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.emacs.len(), 1);
        match &config.emacs["ctrl-c"] {
            super::KeyBindingConfig::Simple(s) => assert_eq!(s, "exit"),
            _ => panic!("expected Simple variant"),
        }
    }

    #[test]
    fn keymap_config_deserializes_conditional_binding() {
        let json = r#"{
            "emacs": {
                "left": [
                    {"when": "cursor-at-start", "action": "exit"},
                    {"action": "cursor-left"}
                ]
            }
        }"#;
        let config: super::KeymapConfig = serde_json::from_str(json).unwrap();
        match &config.emacs["left"] {
            super::KeyBindingConfig::Rules(rules) => {
                assert_eq!(rules.len(), 2);
                assert_eq!(rules[0].when.as_deref(), Some("cursor-at-start"));
                assert_eq!(rules[0].action, "exit");
                assert!(rules[1].when.is_none());
                assert_eq!(rules[1].action, "cursor-left");
            }
            _ => panic!("expected Rules variant"),
        }
    }

    #[test]
    fn keymap_config_deserializes_vim_normal() {
        let json = r#"{"vim-normal": {"j": "select-next", "k": "select-previous"}}"#;
        let config: super::KeymapConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.vim_normal.len(), 2);
        assert!(config.emacs.is_empty());
    }

    #[test]
    fn keymap_config_is_empty_when_default() {
        let config = super::KeymapConfig::default();
        assert!(config.is_empty());
    }

    #[test]
    fn keymap_config_mixed_modes() {
        let json = r#"{
            "emacs": {"ctrl-c": "exit"},
            "vim-normal": {"q": "exit"},
            "inspector": {"d": "delete"}
        }"#;
        let config: super::KeymapConfig = serde_json::from_str(json).unwrap();
        assert!(!config.is_empty());
        assert_eq!(config.emacs.len(), 1);
        assert_eq!(config.vim_normal.len(), 1);
        assert_eq!(config.inspector.len(), 1);
        assert!(config.vim_insert.is_empty());
        assert!(config.prefix.is_empty());
    }
}
