use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use atuin_common::logs::LogLevel;
use atuin_common::path::{AbsPathBuf, ExpandError, PathAnchor, PathExt, RelativeTo, ShellExpanded};
// `AsDuration`/`AsDisableableDuration` are deprecated.
#[allow(deprecated)]
use atuin_common::time::{
    AsDisableableDuration, AsDuration, Days, Minutes, NonZeroDuration, Seconds,
};
use atuin_domain::record::HostId;
use clap::ValueEnum;
use config::builder::DefaultState;
use config::{Config, ConfigBuilder, Environment, File as ConfigFile, FileFormat};
use eyre::{Context, Result, eyre};
use fs_err::{File, create_dir_all};
use regex::RegexSet;
use secrecy::SecretString;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::sync::OnceCell;
use tracing::instrument;
use url::Url;

static EXAMPLE_CONFIG: &str = include_str!("../config.toml");

pub mod daemon;
mod db_location;
pub mod disk_usage_limit;
mod kv;
pub mod meta;
pub mod output;
mod scripts;
pub mod security;
pub mod shells;
pub mod threads;
pub mod watcher;

pub use daemon::Daemon;
pub use db_location::DbLocation;
pub use disk_usage_limit::{DiskUsageLimit, DiskUsageLimitParseError};
use output::OutputCaptureConfig;
pub use output::{CaptureLimits, CommandFilter, OutputCapture};
pub use security::Security;
pub use shells::Shells;
pub use threads::{AiSessionFilterMode, AiSessionResume, Threads};

/// Default sync address for Atuin's hosted service, parsed once.
pub static DEFAULT_SYNC_URL: LazyLock<Url> =
    LazyLock::new(|| Url::parse("https://api.atuin.sh").expect("default sync address is valid"));

/// Default Hub web/API endpoint for Atuin's hosted service, parsed once.
pub static DEFAULT_HUB_URL: LazyLock<Url> =
    LazyLock::new(|| Url::parse("https://hub.atuin.sh").expect("default hub endpoint is valid"));

/// The directory holding `config.toml`, which relative paths in it resolve against.
struct ConfigDir;

impl PathAnchor for ConfigDir {
    fn dir() -> std::io::Result<AbsPathBuf> {
        let mut path = atuin_common::dirs::config_path("config.toml");
        path.pop();
        // A relative `ATUIN_CONFIG_DIR` is relative to the working directory, like the config.toml
        // read from it.
        AbsPathBuf::from_current_dir(path)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchMode {
    Prefix,
    FullText,
    Fuzzy,
    DaemonFuzzy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum RequestedSearchMode {
    Prefix,
    #[clap(alias("full-text"))]
    Fulltext,
    Fuzzy,
    DaemonFuzzy,
    /// Removed: falls back to [`Self::Fuzzy`] but shows a warning.
    #[clap(hide = true)]
    Skim,
}

impl RequestedSearchMode {
    /// Return the actual search mode that will be used.
    ///
    /// Not all requested modes are supported; this method returns the closest supported mode for
    /// the given requested mode.
    #[must_use]
    pub fn effective_mode(self) -> SearchMode {
        match self {
            Self::Prefix => SearchMode::Prefix,
            Self::Fulltext => SearchMode::FullText,
            Self::Fuzzy | Self::Skim => SearchMode::Fuzzy,
            Self::DaemonFuzzy => SearchMode::DaemonFuzzy,
        }
    }
}

impl From<RequestedSearchMode> for SearchMode {
    fn from(requested: RequestedSearchMode) -> Self {
        requested.effective_mode()
    }
}

impl SearchMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prefix => "PREFIX",
            Self::FullText => "FULLTXT",
            Self::Fuzzy => "FUZZY",
            Self::DaemonFuzzy => "DAEMON",
        }
    }

    #[must_use]
    pub fn next(self, settings: &Settings) -> Self {
        match self {
            Self::Prefix => Self::FullText,
            // if the user is using daemon-fuzzy, we go to daemon-fuzzy
            Self::FullText if settings.active_search_mode() == Self::DaemonFuzzy => {
                Self::DaemonFuzzy
            }
            // otherwise fuzzy.
            Self::FullText => Self::Fuzzy,
            Self::Fuzzy | Self::DaemonFuzzy => Self::Prefix,
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, PartialEq, Eq, ValueEnum, Serialize)]
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
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "GLOBAL",
            Self::Host => "HOST",
            Self::Session => "SESSION",
            Self::Directory => "DIRECTORY",
            Self::Workspace => "WORKSPACE",
            Self::SessionPreload => "SESSION+",
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
#[derive(Clone, Debug, Default, Deserialize, Copy, Serialize)]
pub enum Dialect {
    #[default]
    #[serde(rename = "us")]
    Us,

    #[serde(rename = "uk")]
    Uk,
}

impl From<Dialect> for interim::Dialect {
    fn from(d: Dialect) -> Self {
        match d {
            Dialect::Uk => Self::Uk,
            Dialect::Us => Self::Us,
        }
    }
}

use atuin_common::time::UtcOffsetSpec;

#[derive(Clone, Debug, Deserialize, Copy, Serialize)]
pub enum Style {
    #[serde(rename = "auto")]
    Auto,

    #[serde(rename = "full")]
    Full,

    #[serde(rename = "compact")]
    Compact,
}

#[derive(Clone, Debug, Deserialize, Copy, PartialEq, Eq, Serialize)]
pub enum UpdateChannel {
    /// Stable releases only.
    #[serde(rename = "stable")]
    Stable,

    /// Prerelease builds, plus stable releases once they overtake the latest
    /// prerelease.
    #[serde(rename = "nightly")]
    Nightly,
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
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Emacs => "EMACS",
            Self::VimNormal => "VIMNORMAL",
            Self::VimInsert => "VIMINSERT",
            Self::Auto => "AUTO",
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
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DefaultUserShape => "DEFAULT",
            Self::BlinkingBlock => "BLINKBLOCK",
            Self::SteadyBlock => "STEADYBLOCK",
            Self::BlinkingUnderScore => "BLINKUNDERLINE",
            Self::SteadyUnderScore => "STEADYUNDERLINE",
            Self::BlinkingBar => "BLINKBAR",
            Self::SteadyBar => "STEADYBAR",
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

/// Sync protocol type for authentication.
///
/// This setting is primarily for development/testing. When not explicitly set,
/// the protocol is inferred from the sync_address:
/// - Default sync address (api.atuin.sh) → Hub protocol
/// - Custom sync address → Legacy protocol
///
/// Set explicitly to "hub" to use Hub authentication with a custom sync_address
/// (useful for local development against a Hub instance).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncProtocol {
    /// Use Hub authentication (Bearer token from Hub OAuth flow)
    Hub,
    /// Use legacy CLI authentication (Token from CLI register/login)
    Legacy,
    /// Infer from sync_address (default behavior)
    #[default]
    Auto,
}

/// Resolved authentication state for sync operations.
///
/// Determined at runtime by examining which tokens are available and what
/// server the client is configured to talk to. Operations use this to pick
/// the right auth header and endpoint style.
#[cfg(feature = "sync")]
#[derive(Debug, Clone)]
pub enum SyncAuth {
    /// Self-hosted Rust server. Uses `Authorization: Token <session>` and
    /// legacy endpoints.
    Legacy {
        token: SecretString,
    },
    /// Hub with a valid Hub API token (`atapi_*`). Uses
    /// `Authorization: Bearer <token>` and v0 endpoints.
    Hub {
        token: SecretString,
    },
    /// Targeting Hub but only has a CLI session token. Uses
    /// `Authorization: Token <session>` against compat/record endpoints.
    /// Sync, password change, and account deletion still work, but the user
    /// should be nudged to run `atuin login` for full Hub auth.
    HubViaCli {
        token: SecretString,
    },
    /// Not authenticated at all. Contains an actionable user-facing message.
    NotLoggedIn {
        reason: String,
    },
}

#[cfg(feature = "sync")]
impl SyncAuth {
    /// Convert into the auth token type used by the API client.
    ///
    /// Returns an error with an actionable message for `NotLoggedIn`.
    pub fn into_auth_token(self) -> Result<crate::api_client::AuthToken> {
        use crate::api_client::AuthToken;
        match self {
            Self::Legacy { token } => Ok(AuthToken::Token(token)),
            Self::Hub { token } => Ok(AuthToken::Bearer(token)),
            Self::HubViaCli { token } => Ok(AuthToken::Token(token)),
            Self::NotLoggedIn { reason } => Err(eyre!(reason)),
        }
    }
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
    #[must_use]
    pub fn standard_defaults() -> Self {
        Self {
            scroll_exits: true,
            exit_past_line_start: true,
            accept_past_line_end: true,
            accept_past_line_start: false,
            accept_with_backspace: false,
            prefix: "a".to_string(),
        }
    }

    /// Returns true if any value differs from the standard defaults.
    #[must_use]
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
    #[must_use]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PtyProxy {
    /// If enabled, `atuin init` emits shell code that re-execs the shell
    /// inside `atuin pty-proxy`, so no separate `atuin pty-proxy init` line
    /// is needed in shell config. Supported for bash, zsh, fish and nu on
    /// unix platforms.
    #[serde(alias = "enable")]
    pub enabled: bool,
}

#[cfg(feature = "octavo")]
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Octavo {
    pub enabled: bool,
    /// Whether captured command output goes to Octavo too, while `enabled` and output capture are
    /// both on.
    pub upload_output: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Search {
    /// The list of enabled filter modes, in order of priority.
    pub filters: Vec<FilterMode>,

    /// The recency score multiplier for the search index (default: 1.0).
    /// Values < 1.0 reduce weight, > 1.0 increase weight, 0.0 disables.
    pub recency_score_multiplier: f64,

    /// The frequency score multiplier for the search index (default: 1.0).
    /// Values < 1.0 reduce weight, > 1.0 increase weight, 0.0 disables.
    pub frequency_score_multiplier: f64,

    /// The overall frecency score multiplier for the search index (default: 1.0).
    /// Applied after combining recency and frequency scores.
    pub frecency_score_multiplier: f64,

    /// Controls which shells' commands are shown in interactive search.
    ///
    /// One of: `"all"`, `"auto"`, or an array of strings.
    pub shells: Shells,
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

/// Configuration for a specific log type (search or daemon).
#[allow(deprecated)]
#[serde_as]
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LogConfig {
    /// Log file name (relative to dir) or absolute path.
    #[serde_as(as = "ShellExpanded")]
    pub file: PathBuf,

    /// Override global enabled setting for this log type.
    pub enabled: Option<bool>,

    /// Override global level setting for this log type.
    pub level: Option<LogLevel>,

    /// Override the global retention for this log type (a bare number is days). `0` keeps nothing.
    #[serde_as(as = "Option<AsDuration<Days>>")]
    pub retention: Option<Duration>,
}

impl LogConfig {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            ..Default::default()
        }
    }
}

#[allow(deprecated)]
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Logs {
    /// Enable file logging globally. Defaults to true.
    #[serde(default = "Logs::default_enabled")]
    pub enabled: bool,

    /// Directory for log files. Defaults to ~/.atuin/logs
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub dir: AbsPathBuf,

    /// Default log level for file logging. Defaults to "info".
    /// Note: ATUIN_LOG environment variable overrides this.
    #[serde(default)]
    pub level: LogLevel,

    /// Default retention for log files (a bare number is days). Defaults to 4 days. `0` keeps
    /// nothing (prunes every rotated log).
    #[serde(default = "Logs::default_retention")]
    #[serde_as(as = "AsDuration<Days>")]
    pub retention: Duration,

    /// Search log settings; only used with `--interactive`
    #[serde(default)]
    pub search: LogConfig,

    /// Daemon log settings
    #[serde(default)]
    pub daemon: LogConfig,

    /// AI log settings
    #[serde(default)]
    pub ai: LogConfig,
}

/// Endpoint protocol for Atuin AI.
///
/// When set to "auto" (default), the protocol is inferred from `ai.endpoint`:
/// an unset or official Atuin address is treated as Hub, anything else as an
/// OSS server. Set explicitly to "hub" to keep Hub behavior with a custom
/// endpoint (useful for local development against a Hub instance).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AiEndpointProtocol {
    /// Atuin Hub: browser-based login flow, stored Hub session, usage reporting.
    Hub,
    /// A standalone AI server (e.g. atuin-ai-server): requests go straight to
    /// the endpoint, authenticated with `ai.api_token` if set.
    Oss,
    /// Infer from ai.endpoint (default behavior)
    #[default]
    Auto,
}

#[allow(deprecated)]
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Ai {
    /// Whether or not the AI features are enabled.
    pub enabled: Option<bool>,

    /// The address of the Atuin AI endpoint. Used for AI features like command generation.
    /// Only necessary for custom AI endpoints.
    pub endpoint: Option<Url>,

    /// How to talk to `endpoint`. See [`AiEndpointProtocol`].
    #[serde(default)]
    pub endpoint_protocol: AiEndpointProtocol,

    /// The API token for the Atuin AI endpoint. Used for AI features like command generation.
    /// Only necessary for custom AI endpoints.
    #[serde(skip_serializing)]
    pub api_token: Option<SecretString>,

    /// Path to the AI sessions database.
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub db_path: DbLocation,

    /// The maximum time an AI session can be automatically resumed (a bare number is minutes).
    /// `0` disables auto-resume.
    #[serde_as(as = "AsDisableableDuration<Minutes>")]
    pub session_continue_minutes: Option<NonZeroDuration>,

    /// The AI model to use for AI chats, based on the Atuin AI model alias.
    pub model: Option<String>,

    /// Whether to enable YOLO mode (skips all permission checks)
    #[serde(default)]
    pub yolo: bool,

    /// Deprecated: use opening.send_cwd instead. Kept for backwards compatibility.
    #[serde(default)]
    pub send_cwd: Option<bool>,

    /// Configuration for what context is sent in the opening AI request.
    #[serde(default)]
    pub opening: AiOpening,

    /// Tool capability flags.
    #[serde(default)]
    pub capabilities: AiCapabilities,

    /// Whether the AI TUI surfaces feature tips. `None` = enabled.
    pub tips: Option<bool>,
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
pub struct AiCapabilities {
    /// Whether the AI can request to search Atuin history. `None` = unset (defaults to enabled, and the ai will ask for permission).
    pub enable_history_search: Option<bool>,
    /// Whether the AI can request to view the stored output, if any, for Atuin history entries. `None` = unset (defaults to enabled, and the ai will ask for permission).
    pub enable_history_output: Option<bool>,
    /// Whether the AI can request to read and write files. `None` = unset (defaults to enabled, and the ai will ask for permission).
    pub enable_file_tools: Option<bool>,
    /// Whether the AI can request to execute bash commands. `None` = unset (defaults to enabled, and the ai will ask for permission).
    pub enable_command_execution: Option<bool>,
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
pub struct AiOpening {
    /// Whether or not to send the current working directory to the AI endpoint.
    pub send_cwd: Option<bool>,

    /// Whether or not to send the last command as context in the opening AI request.
    pub send_last_command: Option<bool>,
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

impl Logs {
    fn default_enabled() -> bool {
        true
    }

    fn default_retention() -> Duration {
        Duration::from_secs(4 * 86_400)
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

            recency_score_multiplier: 1.0,
            frequency_score_multiplier: 1.0,
            frecency_score_multiplier: 1.0,
            shells: Default::default(),
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
    #[must_use]
    pub fn default_width(&self) -> u16 {
        match self {
            Self::Duration => 5,  // "814ms"
            Self::Time => 9,      // "459ms ago"
            Self::Datetime => 16, // "2025-01-22 14:35"
            Self::Directory => 20,
            Self::Host => 15,
            Self::User => 10,
            Self::Exit => {
                if cfg!(windows) {
                    11 // 32-bit integer on Windows: "-1978335212"
                } else {
                    3 // Usually a byte on Unix
                }
            }
            Self::Command => 0, // Expands to fill
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
    #[must_use]
    pub fn new(column_type: UiColumnType) -> Self {
        Self {
            width: column_type.default_width(),
            expand: column_type == UiColumnType::Command,
            column_type,
        }
    }

    #[must_use]
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

    /// Syntax highlight commands in the interactive search results.
    #[serde(default = "Ui::default_syntax_highlight")]
    pub syntax_highlight: bool,
}

impl Ui {
    fn default_columns() -> Vec<UiColumn> {
        vec![
            UiColumn::new(UiColumnType::Duration),
            UiColumn::new(UiColumnType::Time),
            UiColumn::new(UiColumnType::Command),
        ]
    }

    fn default_syntax_highlight() -> bool {
        true
    }

    /// Validate the UI configuration.
    /// Returns an error if more than one column has expand = true.
    pub fn validate(&self) -> Result<(), UiValidationError> {
        let expand_count = self.columns.iter().filter(|c| c.expand).count();
        if expand_count > 1 {
            return Err(UiValidationError::MultipleExpandingColumns(expand_count));
        }
        Ok(())
    }
}

/// A [`Ui`] configuration that cannot be used as written.
#[derive(Debug, Error)]
pub enum UiValidationError {
    #[error("Only one column can have expand = true, but {0} columns are set to expand")]
    MultipleExpandingColumns(usize),
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            columns: Self::default_columns(),
            syntax_highlight: Self::default_syntax_highlight(),
        }
    }
}

#[allow(deprecated)]
#[serde_as]
#[derive(Debug, Deserialize, Serialize)]
pub struct Settings {
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub data_dir: AbsPathBuf,
    pub dialect: Dialect,
    pub timezone: UtcOffsetSpec,
    pub style: Style,
    pub auto_sync: bool,
    pub update_check: bool,
    pub update_channel: UpdateChannel,

    /// The sync address for atuin.
    pub sync_address: Url,

    /// Sync protocol for authentication. When set to "auto" (default), the protocol
    /// is inferred from sync_address. Set to "hub" to force Hub auth with a custom
    /// sync_address (useful for local development).
    #[serde(default)]
    pub sync_protocol: SyncProtocol,

    #[serde_as(as = "AsDuration<Seconds>")]
    pub sync_frequency: Duration,
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub db_path: DbLocation,
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub record_store_path: DbLocation,
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub key_path: AbsPathBuf,
    #[serde(rename = "search_mode")]
    pub requested_search_mode: RequestedSearchMode,
    pub filter_mode: Option<FilterMode>,
    pub filter_mode_shell_up_key_binding: Option<FilterMode>,
    #[serde(rename = "search_mode_shell_up_key_binding")]
    pub requested_search_mode_shell_up_key_binding: Option<RequestedSearchMode>,

    /// This is not a real setting. Instead, `atuin search` manually sets this field to true if
    /// the hidden `--shell-up-key-binding` option was passed.
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
    pub strip_trailing_whitespace: bool,
    pub prefers_reduced_motion: bool,
    pub store_failed: bool,
    pub no_mouse: bool,

    #[serde(with = "serde_regex", default = "RegexSet::empty", skip_serializing)]
    pub history_filter: RegexSet,

    #[serde(with = "serde_regex", default = "RegexSet::empty", skip_serializing)]
    pub cwd_filter: RegexSet,

    pub secrets_filter: bool,
    pub workspaces: bool,
    pub ctrl_n_shortcuts: bool,

    #[serde_as(as = "AsDuration<Seconds>")]
    pub network_connect_timeout: Duration,
    #[serde_as(as = "AsDuration<Seconds>")]
    pub network_timeout: Duration,
    #[serde_as(as = "AsDuration<Seconds>")]
    pub local_timeout: Duration,

    /// Extra HTTP headers to send on every request to the sync server, e.g.
    /// for services like Cloudflare Access that sit in front of a self-hosted
    /// server. Headers that Atuin sets itself (e.g. Authorization) win over
    /// values configured here.
    #[serde(default, skip_serializing)]
    pub extra_headers: HashMap<String, SecretString>,

    pub enter_accept: bool,
    pub smart_sort: bool,
    pub command_chaining: bool,

    #[serde(default)]
    pub stats: Stats,

    #[serde(default)]
    pub keys: Keys,

    #[serde(default)]
    pub keymap: KeymapConfig,

    #[serde(default)]
    pub preview: Preview,

    pub daemon: Daemon,

    #[serde(default)]
    pub pty_proxy: PtyProxy,

    #[serde(default)]
    pub output: OutputCapture,

    #[cfg(feature = "octavo")]
    #[serde(default)]
    pub octavo: Octavo,

    #[serde(default)]
    pub search: Search,

    #[serde(default)]
    pub theme: Theme,

    #[serde(default)]
    pub ui: Ui,

    pub scripts: scripts::Settings,

    pub kv: kv::Settings,

    #[serde(default)]
    pub tmux: Tmux,

    pub logs: Logs,

    pub meta: meta::Settings,

    pub ai: Ai,

    #[serde(default)]
    pub threads: Threads,

    #[serde(default)]
    pub security: Security,
}

impl Settings {
    #[must_use]
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

    #[must_use]
    pub fn search_mode(&self) -> SearchMode {
        self.requested_search_mode.into()
    }

    pub fn search_mode_shell_up_key_binding(&self) -> Option<SearchMode> {
        self.requested_search_mode_shell_up_key_binding.map(Into::into)
    }

    /// Return the active search mode depending on whether Atuin was invoked from the "up"
    /// keybinding.
    ///
    /// If Atuin was invoked from the "up" keybinding, this returns
    /// [`Self::search_mode_shell_up_key_binding`], falling back to [`Self::search_mode`] if that
    /// binding isn't defined. Otherwise, [`Self::search_mode`] is returned.
    #[must_use]
    pub fn active_search_mode(&self) -> SearchMode {
        self.shell_up_key_binding
            .then(|| self.search_mode_shell_up_key_binding())
            .flatten()
            .unwrap_or_else(|| self.search_mode())
    }

    /// Directory of the durable command-output capture store, under the [data dir](Self::data_dir).
    /// Unlike the sqlite stores this is a fjall database directory, not a single file.
    ///
    /// The user should not mess with this directory. Bad things can happen.
    #[must_use]
    pub fn command_capture_dir(&self) -> PathBuf {
        self.data_dir.join("output-capture")
    }

    /// The threads sidecar database, under the [data dir](Self::data_dir). The daemon owns and
    /// writes it; other readers open it read-only (`AiSessionDatabase::open_read_only`).
    #[must_use]
    pub fn threads_db_path(&self) -> PathBuf {
        self.data_dir.join("threads.db")
    }

    #[cfg(feature = "octavo")]
    #[must_use]
    pub fn octavo_queue_path(&self) -> PathBuf {
        self.data_dir.join("octavo.db")
    }

    // -- Meta store: lazily initialized on first access --

    /// Load the meta store.
    ///
    /// Note: there is one meta store per process. The first time this method is called, the meta
    /// store will be initialized from the parameters in that [`Settings`] instance (most notably
    /// [`meta.db_path`](meta::Settings::db_path)). All future calls to this method will return the
    /// same [`MetaStore`](crate::meta::MetaStore), even if called on a different [`Settings`]
    /// instance. This matches existing behavior.
    pub async fn meta_store(&self) -> Result<&'static crate::meta::MetaStore> {
        static META_STORE: OnceCell<crate::meta::MetaStore> = OnceCell::const_new();

        META_STORE
            .get_or_try_init(|| async {
                crate::meta::MetaStore::new(&self.meta.db_path, &self.data_dir, self.local_timeout)
                    .await
            })
            .await
    }

    pub async fn host_id(&self) -> Result<HostId> {
        self.meta_store().await?.host_id().await
    }

    pub async fn last_sync(&self) -> Result<OffsetDateTime> {
        self.meta_store().await?.last_sync().await
    }

    pub async fn save_sync_time(&self) -> Result<()> {
        self.meta_store().await?.save_sync_time().await
    }

    pub async fn last_version_check(&self) -> Result<OffsetDateTime> {
        self.meta_store().await?.last_version_check().await
    }

    pub async fn save_version_check_time(&self) -> Result<()> {
        self.meta_store().await?.save_version_check_time().await
    }

    pub async fn should_sync(&self) -> Result<bool> {
        if !self.auto_sync || !self.meta_store().await?.logged_in().await? {
            return Ok(false);
        }

        Ok(OffsetDateTime::now_utc() - self.last_sync().await?
            >= time::Duration::try_from(self.sync_frequency)?)
    }

    pub async fn logged_in(&self) -> Result<bool> {
        self.meta_store().await?.logged_in().await
    }

    pub async fn session_token(&self) -> Result<SecretString> {
        match self.meta_store().await?.session_token().await? {
            Some(token) => Ok(token),
            None => Err(eyre!("Tried to load session; not logged in")),
        }
    }

    pub async fn hub_session_token(&self) -> Result<SecretString> {
        match self.meta_store().await?.hub_session_token().await? {
            Some(token) => Ok(token),
            None => Err(eyre!("Tried to load hub session; not logged in")),
        }
    }

    /// Check if a URL matches one of Atuin's official hosted addresses.
    fn is_official_address(url: &Url) -> bool {
        let origin = url.origin();
        origin == DEFAULT_SYNC_URL.origin() || origin == DEFAULT_HUB_URL.origin()
    }

    /// Returns whether this configuration uses Hub-style sync.
    ///
    /// Hub sync uses Bearer token authentication and is the default for
    /// Atuin's hosted service. This returns true when:
    /// - `sync_protocol` is explicitly set to `Hub`, OR
    /// - `sync_protocol` is `Auto` and `sync_address` is an official Atuin address
    #[must_use]
    pub fn is_hub_sync(&self) -> bool {
        match self.sync_protocol {
            SyncProtocol::Hub => true,
            SyncProtocol::Legacy => false,
            SyncProtocol::Auto => Self::is_official_address(&self.sync_address),
        }
    }

    /// Returns whether the resolved AI endpoint should be treated as an Atuin
    /// Hub instance (browser login flow, Hub session management, usage
    /// reporting) rather than a standalone OSS server.
    ///
    /// `endpoint` is the resolved AI endpoint — the `--api-endpoint` flag or
    /// `ai.endpoint` setting, after defaults are applied — which is why it's a
    /// parameter rather than read from `self.ai.endpoint`.
    #[must_use]
    pub fn is_hub_ai_endpoint(&self, endpoint: &Url) -> bool {
        match self.ai.endpoint_protocol {
            AiEndpointProtocol::Hub => true,
            AiEndpointProtocol::Oss => false,
            AiEndpointProtocol::Auto => Self::is_official_address(endpoint),
        }
    }

    /// The base URL for the Hub endpoint.
    ///
    /// For Atuin's official hosted service this is always `https://hub.atuin.sh`,
    /// regardless of whether `sync_address` is `api.atuin.sh` or `hub.atuin.sh`.
    /// For a self-hosted Hub, it is the configured `sync_address`.
    pub fn hub_endpoint(&self) -> Url {
        if self.is_hub_sync() && !Self::is_official_address(&self.sync_address) {
            self.sync_address.clone()
        } else {
            DEFAULT_HUB_URL.clone()
        }
    }

    /// Examines the configured sync target and available tokens to determine
    /// the correct auth strategy. Also performs cleanup of mis-stored tokens
    /// (e.g. a CLI token incorrectly saved in the Hub session slot).
    #[cfg(feature = "sync")]
    #[must_use]
    pub async fn resolve_sync_auth(&self) -> SyncAuth {
        let meta = match self.meta_store().await {
            Ok(m) => m,
            Err(e) => {
                // Unlike not being logged in, a failure: callers tracing the result at debug
                // would hide it.
                tracing::warn!("failed to open the meta store for sync auth: {e}");
                return SyncAuth::NotLoggedIn {
                    reason: format!("Failed to open meta store: {e}"),
                };
            }
        };

        if !self.is_hub_sync() {
            // Self-hosted / legacy server
            return match meta.session_token().await {
                Ok(Some(token)) => SyncAuth::Legacy { token },
                _ => SyncAuth::NotLoggedIn {
                    reason: "Not logged in. Run 'atuin login' to authenticate with your sync \
                             server."
                        .into(),
                },
            };
        }

        // Targeting Hub — check for a valid Hub API token first
        if let Ok(Some(hub_token)) = meta.hub_session_token().await {
            if crate::meta::is_hub_token(&hub_token) {
                return SyncAuth::Hub { token: hub_token };
            }

            // A non-atapi_ token in the hub_session slot is a mis-stored CLI
            // token (from the migration-fallback bug). Move it to the CLI
            // session slot if that slot is empty, then clear hub_session
            // only if the move succeeded.
            if let Ok(None) = meta.session_token().await {
                if meta.save_session(&hub_token).await.is_ok() {
                    let _ = meta.delete_hub_session().await;
                }
            } else {
                // CLI slot already has a token; just clear the bad hub_session
                let _ = meta.delete_hub_session().await;
            }
            // Fall through to check CLI token below
        }

        // No valid Hub token — check for a CLI session token
        match meta.session_token().await {
            Ok(Some(token)) => SyncAuth::HubViaCli { token },
            _ => SyncAuth::NotLoggedIn {
                reason: "Not logged in. Run 'atuin login' or 'atuin register' to authenticate."
                    .into(),
            },
        }
    }

    /// Returns the appropriate auth token for sync operations.
    ///
    /// Delegates to [`Self::resolve_sync_auth`] and converts the result to an
    /// `AuthToken`. Callers that need to distinguish between auth states
    /// (e.g. to show different UI) should call `resolve_sync_auth` directly.
    ///
    /// Not being logged in is an expected state (an offline daemon asks on every start), so the
    /// error is traced at debug, and callers that fail on it report it themselves.
    #[cfg(feature = "sync")]
    #[instrument(level = "trace", skip_all, err(level = "debug"))]
    pub async fn sync_auth_token(&self) -> Result<crate::api_client::AuthToken> {
        self.resolve_sync_auth().await.into_auth_token()
    }

    #[cfg(feature = "check-update")]
    async fn needs_update_check(&self) -> Result<bool> {
        let last_check = self.last_version_check().await?;
        let diff = OffsetDateTime::now_utc() - last_check;

        // Check a max of once per hour
        Ok(diff.whole_hours() >= 1)
    }

    #[cfg(feature = "check-update")]
    async fn latest_version(&self) -> Result<Version> {
        // Default to the current version, and if that doesn't parse, a version so high it's unlikely to ever
        // suggest upgrading.
        let current =
            Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(100_000, 0, 0));

        if !self.needs_update_check().await? {
            let meta = self.meta_store().await?;
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

        let meta = self.meta_store().await?;
        self.save_version_check_time().await?;
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
            Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(100_000, 0, 0));

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

    #[must_use]
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
    #[allow(
        clippy::unused_async,
        reason = "needs to match the `check-update` version of this method"
    )]
    pub async fn needs_update(&self) -> Option<Version> {
        None
    }

    pub fn builder() -> Result<ConfigBuilder<DefaultState>> {
        Ok(Self::builder_with_data_dir(&atuin_common::dirs::unconfigured_data_dir())?)
    }

    /// The default of each path setting, for `data_dir`, [escaped](ShellExpanded::escape) as
    /// config.toml would write it. An empty value of a path setting falls back to its default
    /// here, so every path setting needs one.
    fn path_defaults(data_dir: &Path) -> [(&'static str, Option<String>); 14] {
        let path = |path: &Path| path.to_str().map(ShellExpanded::escape);
        [
            ("data_dir", path(data_dir)),
            ("db_path", path(&data_dir.join("history.db"))),
            ("record_store_path", path(&data_dir.join("records.db"))),
            ("key_path", path(&data_dir.join("key"))),
            ("daemon.socket_path", None),
            ("daemon.pidfile_path", path(&data_dir.join("atuin-daemon.pid"))),
            ("logs.dir", path(&atuin_common::dirs::logs_dir())),
            ("logs.search.file", Some("search.log".to_owned())),
            ("logs.daemon.file", Some("daemon.log".to_owned())),
            ("logs.ai.file", Some("ai.log".to_owned())),
            ("kv.db_path", path(&data_dir.join("kv.db"))),
            ("scripts.db_path", path(&data_dir.join("scripts.db"))),
            ("meta.db_path", path(&data_dir.join("meta.db"))),
            ("ai.db_path", path(&data_dir.join("ai_sessions.db"))),
        ]
    }

    /// `config`, with each path setting in `defaults` that is empty set to its default instead.
    /// Warns about those, and about each that names a variable that isn't set, which loads as
    /// written.
    fn with_lenient_paths(
        config: Config,
        defaults: impl IntoIterator<Item = (&'static str, Option<String>)>,
    ) -> Result<Config, config::ConfigError> {
        let mut emptied = Vec::new();
        for (key, default) in defaults {
            let Ok(raw) = config.get_string(key) else {
                continue;
            };
            match ShellExpanded::expand(&raw) {
                Ok(_) => {}
                Err(ExpandError::Empty) => {
                    tracing::warn!("{key} is empty, so Atuin uses its default");
                    emptied.push((key, default));
                }
                Err(err @ ExpandError::Variable { .. }) => tracing::warn!(
                    "{key}: failed to expand {raw:?} ({err}), so Atuin uses it as written; write \
                     $$ for a literal $"
                ),
            }
        }
        if emptied.is_empty() {
            return Ok(config);
        }

        emptied
            .into_iter()
            .try_fold(Config::builder().add_source(config), |builder, (key, default)| {
                builder.set_override(key, default)
            })?
            .build()
    }

    fn builder_with_data_dir(
        data_dir: &std::path::Path,
    ) -> Result<ConfigBuilder<DefaultState>, config::ConfigError> {
        let output = OutputCaptureConfig::default();

        let builder = Self::path_defaults(data_dir)
            .into_iter()
            .try_fold(Config::builder(), |builder, (key, default)| {
                builder.set_default(key, default)
            })?
            .set_default("history_format", "{time}\t{command}\t{duration}")?
            .set_default("dialect", "us")?
            .set_default("timezone", "local")?
            .set_default("auto_sync", true)?
            .set_default("update_check", cfg!(feature = "check-update"))?
            .set_default("update_channel", "stable")?
            .set_default("sync_address", DEFAULT_SYNC_URL.as_str())?
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
            .set_default("workspaces", true)?
            .set_default("ctrl_n_shortcuts", false)?
            .set_default("secrets_filter", true)?
            .set_default("strip_trailing_whitespace", true)?
            .set_default("network_connect_timeout", 5)?
            .set_default("network_timeout", 30)?
            .set_default("extra_headers", HashMap::<String, String>::new())?
            .set_default("local_timeout", 2.0)?
            // enter_accept defaults to false here, but true in the default config file. The dissonance is
            // intentional!
            // Existing users will get the default "False", so we don't mess with any potential
            // muscle memory.
            // New users will get the new default, that is more similar to what they are used to.
            .set_default("enter_accept", false)?
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
            .set_default("daemon.systemd_socket", false)?
            .set_default("daemon.tcp_port", 8889)?
            .set_default("output.enabled", output.enabled)?
            .set_default("output.max_output_size", output.max_output_size.to_string())?
            .set_default("output.sync", output.sync)?
            .set_default("output.max_disk_usage", output.max_disk_usage.to_string())?
            .set_default("logs.enabled", true)?
            .set_default("logs.level", "info")?
            .set_default("search.recency_score_multiplier", 1.0)?
            .set_default("search.frequency_score_multiplier", 1.0)?
            .set_default("search.frecency_score_multiplier", 1.0)?
            .set_default("search.shells", "auto")?
            .set_default("ai.session_continue_minutes", 60)?
            .set_default("ai.send_cwd", false)?
            .set_default("ai.opening.send_cwd", false)?
            .set_default("ai.opening.send_last_command", false)?
            .set_default("ui.syntax_highlight", true)?
            .set_default("search.filters", vec![
                "global",
                "host",
                "session",
                "workspace",
                "directory",
                "session-preload",
            ])?
            .set_default("theme.name", "default")?
            .set_default("theme.debug", None::<bool>)?
            .set_default("tmux.enabled", false)?
            .set_default("tmux.width", "80%")?
            .set_default("tmux.height", "60%")?
            .set_default(
                "prefers_reduced_motion",
                atuin_common::env::var_os("NO_MOTION").is_some(),
            )?
            .set_default("no_mouse", false)?
            .set_default("pty_proxy.enabled", false)?;
        #[cfg(feature = "octavo")]
        let builder = builder
            .set_default("octavo.enabled", false)?
            .set_default("octavo.upload_output", false)?;

        Ok(builder.add_source(Self::environment()))
    }

    /// The `ATUIN_*` environment variables, as a config source.
    fn environment() -> Environment {
        Environment::with_prefix("atuin").prefix_separator("_").separator("__")
    }

    /// Layers a config file over `builder`, with `env` on top of it again so that environment
    /// variables override the file.
    fn add_config_file(
        builder: ConfigBuilder<DefaultState>,
        file: impl config::Source + Send + Sync + 'static,
        env: Environment,
    ) -> ConfigBuilder<DefaultState> {
        builder.add_source(file).add_source(env)
    }

    pub fn get_config_path() -> Result<PathBuf> {
        let config_file = atuin_common::dirs::config_path("config.toml");

        if let Some(config_dir) = config_file.parent() {
            create_dir_all(config_dir)
                .wrap_err_with(|| format!("could not create dir {config_dir:?}"))?;
        }

        Ok(config_file)
    }

    /// Build a merged `Config` from defaults, config file, and environment.
    ///
    /// This resolves `data_dir`, initializes the data directory on disk,
    /// and layers defaults → config file → env overrides. Both `new()` and
    /// `get_config_value()` use this so the resolution logic lives in one place.
    fn build_config() -> Result<Config> {
        let config_file = Self::get_config_path()?;

        // extract data_dir first so we can use it as the base for other path defaults
        let effective_data_dir = if config_file.exists() {
            #[serde_as]
            #[derive(Deserialize)]
            struct DataDirOnly {
                #[serde_as(as = "Option<RelativeTo<ConfigDir>>")]
                data_dir: Option<AbsPathBuf>,
            }

            let config_file_str =
                config_file.to_str().ok_or_else(|| eyre!("config file path is not valid UTF-8"))?;

            // A config that doesn't build fails again, with a better error, below.
            let custom_data_dir = match Config::builder()
                .add_source(ConfigFile::new(config_file_str, FileFormat::Toml))
                .add_source(Self::environment())
                .build()
            {
                Ok(partial) => {
                    Self::with_lenient_paths(partial, [("data_dir", None)])?
                        .try_deserialize::<DataDirOnly>()?
                        .data_dir
                }
                Err(_) => None,
            };

            custom_data_dir.map_or_else(atuin_common::dirs::unconfigured_data_dir, PathBuf::from)
        } else {
            atuin_common::dirs::unconfigured_data_dir()
        };

        create_dir_all(&effective_data_dir)
            .wrap_err_with(|| format!("could not create dir {effective_data_dir:?}"))?;

        let config_builder = Self::builder_with_data_dir(&effective_data_dir)?;

        let config_builder = if config_file.exists() {
            let config_file_str =
                config_file.to_str().ok_or_else(|| eyre!("config file path is not valid UTF-8"))?;
            Self::add_config_file(
                config_builder,
                ConfigFile::new(config_file_str, FileFormat::Toml),
                Self::environment(),
            )
        } else {
            // Use `create_new` to avoid overwriting the file if it just got created in between our
            // existence check and now.
            match File::create_new(&config_file) {
                Ok(mut file) => file
                    .write_all(EXAMPLE_CONFIG.as_bytes())
                    .wrap_err("could not write default config file")?,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err).wrap_err("could not create config file"),
            }

            config_builder
        };

        // Explicitly set `data_dir` here to ensure it matches the `effective_data_dir` we just
        // calculated.
        let config_builder = config_builder
            .set_override("data_dir", effective_data_dir.to_str().map(ShellExpanded::escape))?;

        Ok(Self::with_lenient_paths(
            Self::with_legacy_threads(config_builder.build()?)?,
            Self::path_defaults(&effective_data_dir),
        )?)
    }

    /// `[threads]` was `ai.capture_sessions`, `ai.capture_tools` and `[ai.sessions]`: read them
    /// as `threads.capture`, `threads.capture_tools` and `threads.*`, for any key `[threads]`
    /// doesn't set itself. The defaults are set here, under them, so they never hide a legacy
    /// value.
    fn with_legacy_threads(config: Config) -> Result<Config, config::ConfigError> {
        fn leaves(prefix: String, value: config::Value, out: &mut Vec<(String, config::Value)>) {
            match value.kind {
                config::ValueKind::Table(table) => {
                    for (key, value) in table {
                        leaves(format!("{prefix}.{key}"), value, out);
                    }
                }
                _ => out.push((prefix, value)),
            }
        }

        let mut legacy = Vec::new();
        for (old, new) in [
            ("ai.capture_sessions", "threads.capture"),
            ("ai.capture_tools", "threads.capture_tools"),
            ("ai.sessions", "threads"),
        ] {
            if let Ok(value) = config.get::<config::Value>(old) {
                leaves(new.to_string(), value, &mut legacy);
            }
        }
        legacy.retain(|(key, _)| config.get::<config::Value>(key).is_err());

        legacy
            .into_iter()
            .try_fold(
                Config::builder()
                    .set_default("threads.capture", false)?
                    .set_default("threads.capture_tools", true)?
                    .add_source(config),
                |builder, (key, value)| builder.set_override(key, value),
            )?
            .build()
    }

    /// Look up a single config value by dotted key (e.g. `"daemon.sync_frequency"`).
    ///
    /// Returns the effective value after merging defaults, config file, and environment, with the
    /// path settings as `self` resolved them.
    pub fn get_config_value(&self, key: &str) -> Result<String> {
        use config::Value;

        // When unset, `daemon.socket_path` is calculated dynamically by [`Daemon::socket_path`] and
        // wouldn't show up in `atuin config get --resolved daemon.socket_path`. However, it may be
        // useful for users to see the path when debugging socket issues, so it's shown here.
        #[cfg(unix)]
        let socket_path = (key == "daemon" || key == "daemon.socket_path")
            .then(|| self.daemon.socket_path().as_path().to_path_buf());
        #[cfg(not(unix))]
        let socket_path = self.daemon.socket_path.clone().map(AbsPathBuf::into_path_buf);

        // Loading `self` warned about its paths already.
        let config = tracing::subscriber::with_default(
            tracing::subscriber::NoSubscriber::default(),
            Self::build_config,
        )?;
        let config = self
            .resolved_paths()
            .into_iter()
            .chain(socket_path.as_deref().map(|path| ("daemon.socket_path", path.as_os_str())))
            .try_fold(Config::builder().add_source(config), |builder, (key, path)| {
                builder.set_override(key, path.to_string_lossy().into_owned())
            })?
            .build()?;

        let value: Value =
            config.get(key).map_err(|e| eyre!("failed to get config value '{}': {}", key, e))?;

        Ok(Self::format_resolved_value(&value, key))
    }

    /// The path settings by key, as deserialization resolved them; the merged [`Config`] holds
    /// them as written, with Atuin's own defaults [escaped](ShellExpanded::escape). A new path
    /// setting belongs here too, or `atuin config get --resolved` shows it as written.
    fn resolved_paths(&self) -> [(&'static str, &OsStr); 13] {
        [
            ("data_dir", self.data_dir.as_os_str()),
            ("db_path", self.db_path.as_os_str()),
            ("record_store_path", self.record_store_path.as_os_str()),
            ("key_path", self.key_path.as_os_str()),
            ("daemon.pidfile_path", self.daemon.pidfile_path.as_os_str()),
            ("logs.dir", self.logs.dir.as_os_str()),
            ("logs.search.file", self.logs.search.file.as_os_str()),
            ("logs.daemon.file", self.logs.daemon.file.as_os_str()),
            ("logs.ai.file", self.logs.ai.file.as_os_str()),
            ("kv.db_path", self.kv.db_path.as_os_str()),
            ("scripts.db_path", self.scripts.db_path.as_os_str()),
            ("meta.db_path", self.meta.db_path.as_os_str()),
            ("ai.db_path", self.ai.db_path.as_os_str()),
        ]
    }

    fn format_resolved_value(value: &config::Value, prefix: &str) -> String {
        use config::ValueKind;

        match &value.kind {
            ValueKind::Nil => String::new(),
            ValueKind::Boolean(b) => b.to_string(),
            ValueKind::I64(i) => i.to_string(),
            ValueKind::I128(i) => i.to_string(),
            ValueKind::U64(u) => u.to_string(),
            ValueKind::U128(u) => u.to_string(),
            ValueKind::Float(f) => f.to_string(),
            ValueKind::String(s) => s.clone(),
            ValueKind::Array(arr) => {
                let items: Vec<String> =
                    arr.iter().map(|v| Self::format_resolved_value(v, "")).collect();
                format!("[{}]", items.join(", "))
            }
            ValueKind::Table(map) => {
                let mut lines = Vec::new();
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();

                for k in keys {
                    let v = &map[k];
                    let full_key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };

                    match &v.kind {
                        ValueKind::Table(_) => {
                            lines.push(Self::format_resolved_value(v, &full_key));
                        }
                        _ => {
                            lines.push(format!(
                                "{} = {}",
                                full_key,
                                Self::format_resolved_value(v, "")
                            ));
                        }
                    }
                }

                lines.join("\n")
            }
        }
    }

    pub fn new() -> Result<Self> {
        let config = Self::build_config()?;
        let settings: Self =
            config.try_deserialize().map_err(|e| eyre!("failed to deserialize: {}", e))?;

        // Validate UI settings
        settings.ui.validate()?;

        Ok(settings)
    }

    #[must_use]
    pub fn example_config() -> &'static str {
        EXAMPLE_CONFIG
    }

    #[must_use]
    pub fn paths_ok(&self) -> bool {
        [
            self.db_path.file(),
            self.record_store_path.file(),
            Some(self.key_path.as_abs_path()),
            self.meta.db_path.file(),
        ]
        .into_iter()
        .flatten()
        .all(|path| !path.is_dangling_symlink())
    }

    /// Check that a TOML string can be successfully deserialized into a [`Settings`] object.
    pub fn validate_str(toml: &str) -> Result<(), ValidationError> {
        let data_dir = atuin_common::dirs::unconfigured_data_dir();
        let config = Self::with_lenient_paths(
            Self::with_legacy_threads(
                Self::builder_with_data_dir(&data_dir)?
                    .add_source(ConfigFile::from_str(toml, FileFormat::Toml))
                    .build()?,
            )?,
            Self::path_defaults(&data_dir),
        )?;

        let settings: Self = config.try_deserialize()?;
        settings.ui.validate()?;
        Ok(())
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

/// Error returned by [`Settings::validate_str`] when validation fails.
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error(transparent)]
    Config(#[from] config::ConfigError),

    #[error(transparent)]
    Ui(#[from] UiValidationError),
}

#[cfg(test)]
pub(crate) fn test_local_timeout() -> Duration {
    let secs = atuin_common::env::var("ATUIN_TEST_LOCAL_TIMEOUT")
        .ok()
        .and_then(|x| x.parse::<f64>().ok())
        // this hardcoded value should be replaced by a simple way to get the
        // default local_timeout of Settings if possible
        .unwrap_or(2.0);
    Duration::try_from_secs_f64(secs).unwrap_or_else(|_| Duration::from_secs(2))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::str::FromStr;

    use atuin_common::db::SqliteDbUrl;
    use atuin_common::env::MockEnv;
    use atuin_common::path::{AbsPathBuf, PathBufExt as _};
    use config::{Config, ValueKind};
    use eyre::Result;
    use rstest::{fixture, rstest};
    use url::Url;

    use super::{
        AiEndpointProtocol, ConfigFile, DbLocation, FileFormat, FilterMode, RequestedSearchMode,
        SearchMode, Settings, UtcOffsetSpec,
    };

    #[rstest]
    #[case::plus_two_digit_hours("+02", (2, 0, 0))]
    #[case::minus_two_digit_hours("-04", (-4, 0, 0))]
    #[case::plus_hours_minutes("+05:30", (5, 30, 0))]
    #[case::minus_hours_minutes("-09:30", (-9, -30, 0))]
    // single digit hours are allowed
    #[case::plus_single_digit_hour("+2", (2, 0, 0))]
    #[case::minus_single_digit_hour("-4", (-4, 0, 0))]
    #[case::plus_single_digit_hour_minutes("+5:30", (5, 30, 0))]
    #[case::minus_single_digit_hour_minutes("-9:30", (-9, -30, 0))]
    // fully qualified form
    #[case::plus_fully_qualified("+09:30:00", (9, 30, 0))]
    #[case::minus_fully_qualified("-09:30:00", (-9, -30, 0))]
    // these offsets don't really exist but are supported anyway
    #[case::plus_zero_hour_minutes("+0:5", (0, 5, 0))]
    #[case::minus_zero_hour_minutes("-0:5", (0, -5, 0))]
    #[case::plus_with_seconds("+01:23:45", (1, 23, 45))]
    #[case::minus_with_seconds("-01:23:45", (-1, -23, -45))]
    fn can_parse_offset_timezone_spec(
        #[case] input: &str,
        #[case] expected: (i8, i8, i8),
    ) -> Result<()> {
        assert_eq!(UtcOffsetSpec::from_str(input)?.0.as_hms(), expected);
        Ok(())
    }

    /// A leading sign is required, for clarity.
    #[rstest]
    #[case::bare_hour("5")]
    #[case::bare_hour_minutes("10:30")]
    fn rejects_timezone_spec_without_leading_sign(#[case] input: &str) {
        assert!(UtcOffsetSpec::from_str(input).is_err());
    }

    #[rstest]
    // Auto: official addresses are Hub, anything else is OSS
    #[case::auto_hub_address(AiEndpointProtocol::Auto, "https://hub.atuin.sh", true)]
    #[case::auto_api_address(AiEndpointProtocol::Auto, "https://api.atuin.sh", true)]
    #[case::auto_third_party_address(AiEndpointProtocol::Auto, "https://ai.example.com", false)]
    #[case::auto_localhost(AiEndpointProtocol::Auto, "http://localhost:4000", false)]
    // An explicit protocol overrides the address check
    #[case::explicit_hub_overrides_address(AiEndpointProtocol::Hub, "http://localhost:4000", true)]
    #[case::explicit_oss_overrides_address(AiEndpointProtocol::Oss, "https://hub.atuin.sh", false)]
    fn ai_endpoint_protocol_resolution(
        #[case] protocol: AiEndpointProtocol,
        #[case] endpoint: &str,
        #[case] expected: bool,
    ) {
        let mut settings = Settings::default();
        settings.ai.endpoint_protocol = protocol;

        assert_eq!(settings.is_hub_ai_endpoint(&Url::parse(endpoint).unwrap()), expected,);
    }

    /// Forces both `LazyLock`s, so a typo in either constant fails here rather
    /// than panicking at runtime.
    #[rstest]
    fn default_addresses_parse() {
        assert_eq!(super::DEFAULT_SYNC_URL.host_str(), Some("api.atuin.sh"));
        assert_eq!(super::DEFAULT_HUB_URL.host_str(), Some("hub.atuin.sh"));
    }

    #[rstest]
    #[case::in_git_context(true, true, FilterMode::Workspace)]
    #[case::not_in_git_context(true, false, FilterMode::Host)]
    #[case::workspaces_disabled(false, true, FilterMode::Host)]
    fn workspace_filter_selection(
        #[case] workspaces: bool,
        #[case] git_root: bool,
        #[case] expected: FilterMode,
    ) {
        let mut settings = super::Settings::default();
        settings.search.filters = vec![
            super::FilterMode::Workspace,
            super::FilterMode::Host,
            super::FilterMode::Directory,
            super::FilterMode::Session,
            super::FilterMode::Global,
        ];
        settings.workspaces = workspaces;

        assert_eq!(settings.default_filter_mode(git_root), expected);
    }

    /// The example config writes out the headers of tables whose keys are all commented out, so
    /// that `atuin config set` can uncomment their keys beneath them. Those empty tables must
    /// resolve the same as leaving them out.
    #[rstest]
    fn example_config_empty_tables_change_nothing() -> Result<()> {
        let resolve = |toml: &str| -> Result<serde_json::Value> {
            let settings: Settings =
                Settings::builder_with_data_dir(&atuin_common::dirs::unconfigured_data_dir())?
                    .add_source(ConfigFile::from_str(toml, FileFormat::Toml))
                    .build()?
                    .try_deserialize()?;
            Ok(serde_json::to_value(settings)?)
        };

        let example = Settings::example_config();
        let lines: Vec<&str> = example.lines().collect();
        let is_header = |line: &str| line.starts_with('[');
        let mut emptied = 0;
        let without_empty_tables: String = lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let empty = is_header(line)
                    && lines[i + 1..]
                        .iter()
                        .take_while(|line| !is_header(line))
                        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'));
                if empty {
                    emptied += 1;
                    format!("# {line}\n")
                } else {
                    format!("{line}\n")
                }
            })
            .collect();

        assert!(emptied >= 3, "expected the example config to have empty tables");
        assert_eq!(resolve(example)?, resolve(&without_empty_tables)?);
        Ok(())
    }

    #[rstest]
    fn builder_with_data_dir_uses_custom_paths() -> Result<()> {
        use std::path::PathBuf;

        let custom_dir = PathBuf::from("/custom/data/dir");
        let builder = Settings::builder_with_data_dir(&custom_dir)?;
        let config = builder.build()?;

        let data_dir: String = config.get("data_dir")?;
        let db_path: String = config.get("db_path")?;
        let key_path: String = config.get("key_path")?;
        let record_store_path: String = config.get("record_store_path")?;
        let kv_db_path: String = config.get("kv.db_path")?;
        let scripts_db_path: String = config.get("scripts.db_path")?;
        let meta_db_path: String = config.get("meta.db_path")?;
        let daemon_socket_path: Option<String> = config.get("daemon.socket_path")?;
        let daemon_pidfile_path: String = config.get("daemon.pidfile_path")?;
        let daemon_autostart: bool = config.get("daemon.autostart")?;

        assert_eq!(data_dir, custom_dir.to_str().unwrap());
        assert_eq!(db_path, custom_dir.join("history.db").to_str().unwrap());
        assert_eq!(key_path, custom_dir.join("key").to_str().unwrap());
        assert_eq!(record_store_path, custom_dir.join("records.db").to_str().unwrap());
        assert_eq!(kv_db_path, custom_dir.join("kv.db").to_str().unwrap());
        assert_eq!(scripts_db_path, custom_dir.join("scripts.db").to_str().unwrap());
        assert_eq!(meta_db_path, custom_dir.join("meta.db").to_str().unwrap());
        assert_eq!(daemon_socket_path, None);
        assert_eq!(daemon_pidfile_path, custom_dir.join("atuin-daemon.pid").to_str().unwrap());
        assert!(!daemon_autostart);

        Ok(())
    }

    #[rstest]
    #[case::valid_config("search_mode = \"fuzzy\"\n")]
    #[case::empty_config("")]
    #[case::plain_data_dir("data_dir = \"/tmp/atuin-test\"\n")]
    #[case::path_with_an_unset_variable("data_dir = \"${DEFINITELY_UNSET_VAR_XYZ}/atuin\"\n")]
    #[case::empty_path("db_path = \"\"\n")]
    #[case::sqlite_url("db_path = \"sqlite:///tmp/atuin-test.db\"\n")]
    fn validate_accepts(#[case] toml: &str) {
        assert!(Settings::validate_str(toml).is_ok());
    }

    /// The error should always name the offending key.
    #[rstest]
    #[case::invalid_enum_variant("search_mode = \"invalid\"\n", "search_mode")]
    #[case::value_of_the_wrong_type("auto_sync = \"banana\"\n", "auto_sync")]
    #[case::invalid_nested_value("[search]\nfilters = [\"nope\"]\n", "search.filters")]
    #[case::more_than_one_expanding_column(
        "[ui]\ncolumns = [{ type = \"duration\", expand = true }, { type = \"command\", expand = \
         true }]\n",
        "expand"
    )]
    fn validate_rejects(#[case] toml: &str, #[case] expected_err: &str) {
        let err = Settings::validate_str(toml).expect_err("config should not validate").to_string();

        assert!(err.contains(expected_err), "error should mention `{expected_err}`, got: {err}");
    }

    #[rstest]
    #[case::configured(Some("custom"), "custom")]
    #[case::unset(None, "default")]
    fn data_dir_prefers_the_configured_one(
        #[case] configured: Option<&str>,
        #[case] expected: &str,
    ) {
        // Absolute on every platform, unlike `/data` on Windows, which would resolve beside
        // config.toml.
        let root = std::env::current_dir().unwrap();
        let toml =
            configured.map_or_else(String::new, |dir| format!("data_dir = {:?}\n", root.join(dir)));
        let settings: Settings = Settings::builder_with_data_dir(&root.join("default"))
            .unwrap()
            .add_source(ConfigFile::from_str(&toml, FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        assert_eq!(settings.data_dir.as_path(), root.joined(expected));
    }

    /// A relative path in config.toml resolves beside config.toml, whatever directory Atuin runs
    /// in.
    #[rstest]
    #[case::data_dir("data_dir", |s: &Settings| s.data_dir.to_path_buf())]
    #[case::db_path("db_path", |s: &Settings| s.db_path.file().unwrap().to_path_buf())]
    #[case::record_store_path("record_store_path", |s: &Settings| {
        s.record_store_path.file().unwrap().to_path_buf()
    })]
    #[case::key_path("key_path", |s: &Settings| s.key_path.to_path_buf())]
    #[case::socket_path("daemon.socket_path", |s: &Settings| {
        s.daemon.socket_path.as_deref().unwrap().to_path_buf()
    })]
    #[case::pidfile_path("daemon.pidfile_path", |s: &Settings| s.daemon.pidfile_path.to_path_buf())]
    #[case::logs_dir("logs.dir", |s: &Settings| s.logs.dir.to_path_buf())]
    #[case::kv_db_path("kv.db_path", |s: &Settings| s.kv.db_path.file().unwrap().to_path_buf())]
    #[case::scripts_db_path("scripts.db_path", |s: &Settings| {
        s.scripts.db_path.file().unwrap().to_path_buf()
    })]
    #[case::meta_db_path("meta.db_path", |s: &Settings| s.meta.db_path.file().unwrap().to_path_buf())]
    #[case::ai_db_path("ai.db_path", |s: &Settings| s.ai.db_path.file().unwrap().to_path_buf())]
    fn relative_paths_resolve_next_to_config_toml(
        #[case] key: &str,
        #[case] path: fn(&Settings) -> PathBuf,
        #[values("/config", "config")] config_dir: &str,
    ) {
        let env = MockEnv::install();
        env.set("HOME", "/home/user");
        env.set("ATUIN_CONFIG_DIR", config_dir);

        let settings: Settings = Settings::builder_with_data_dir(Path::new("/data"))
            .unwrap()
            .set_override(key, "relative/path")
            .unwrap()
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        // A relative `ATUIN_CONFIG_DIR`, like `/config` on Windows, is relative to the working
        // directory.
        let expected = std::env::current_dir().unwrap().joined(config_dir).joined("relative/path");
        assert_eq!(path(&settings), expected);
    }

    /// A database setting takes SQLite's `:memory:` and URIs as written, rather than resolving
    /// them beside config.toml into odd file names.
    #[rstest]
    #[case::db_path("db_path", |s: &Settings| s.db_path.clone())]
    #[case::record_store_path("record_store_path", |s: &Settings| s.record_store_path.clone())]
    #[case::kv_db_path("kv.db_path", |s: &Settings| s.kv.db_path.clone())]
    #[case::scripts_db_path("scripts.db_path", |s: &Settings| s.scripts.db_path.clone())]
    #[case::meta_db_path("meta.db_path", |s: &Settings| s.meta.db_path.clone())]
    #[case::ai_db_path("ai.db_path", |s: &Settings| s.ai.db_path.clone())]
    fn database_paths_take_sqlite_urls_as_written(
        #[case] key: &str,
        #[case] location: fn(&Settings) -> DbLocation,
        #[values(":memory:", "sqlite:///tmp/history.db", "file:history.db?mode=memory")]
        value: &str,
    ) {
        let settings: Settings = Settings::builder_with_data_dir(Path::new("/data"))
            .unwrap()
            .set_override(key, value)
            .unwrap()
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        assert_eq!(location(&settings), DbLocation::Url(SqliteDbUrl(value.to_owned())));
    }

    /// A mock environment with config.toml in `config` under the working directory.
    #[fixture]
    fn config_env() -> MockEnv {
        let env = MockEnv::install();
        env.set("HOME", std::env::current_dir().unwrap().joined("home"));
        env.set("ATUIN_CONFIG_DIR", std::env::current_dir().unwrap().joined("config"));
        env
    }

    /// Settings with `key` set to `value`, if given, loaded as `Settings::new` loads them, in the
    /// mock environment `_env`.
    fn load(_env: &MockEnv, key: &str, value: Option<&str>) -> Settings {
        let data_dir = std::env::current_dir().unwrap().joined("data");
        let config = Settings::builder_with_data_dir(&data_dir)
            .unwrap()
            .set_override_option(key, value)
            .unwrap()
            .build()
            .unwrap();
        Settings::with_lenient_paths(config, Settings::path_defaults(&data_dir))
            .unwrap()
            .try_deserialize()
            .unwrap()
    }

    /// An empty path setting is its default, as one left unset is.
    #[rstest]
    fn an_empty_path_setting_is_its_default(
        config_env: MockEnv,
        #[values(
            "data_dir",
            "db_path",
            "record_store_path",
            "key_path",
            "daemon.pidfile_path",
            "logs.dir",
            "logs.search.file",
            "logs.daemon.file",
            "logs.ai.file",
            "kv.db_path",
            "scripts.db_path",
            "meta.db_path",
            "ai.db_path"
        )]
        key: &str,
    ) {
        assert_eq!(
            load(&config_env, key, Some("")).resolved_paths(),
            load(&config_env, key, None).resolved_paths()
        );
    }

    /// A path setting naming a variable that isn't set loads as written, resolved as any other.
    #[rstest]
    #[case::data_dir("data_dir", true)]
    #[case::db_path("db_path", true)]
    #[case::record_store_path("record_store_path", true)]
    #[case::key_path("key_path", true)]
    #[case::pidfile_path("daemon.pidfile_path", true)]
    #[case::logs_dir("logs.dir", true)]
    #[case::search_log_file("logs.search.file", false)]
    #[case::daemon_log_file("logs.daemon.file", false)]
    #[case::ai_log_file("logs.ai.file", false)]
    #[case::kv_db_path("kv.db_path", true)]
    #[case::scripts_db_path("scripts.db_path", true)]
    #[case::meta_db_path("meta.db_path", true)]
    #[case::ai_db_path("ai.db_path", true)]
    fn a_path_setting_with_an_unset_variable_loads_as_written(
        config_env: MockEnv,
        #[case] key: &str,
        #[case] beside_config_toml: bool,
    ) {
        let written = Path::new("$UNSET/${ALSO_UNSET}");
        let expected = if beside_config_toml {
            std::env::current_dir().unwrap().joined("config").joined(written)
        } else {
            written.to_path_buf()
        };

        let settings = load(&config_env, key, written.to_str());
        let (_, resolved) =
            settings.resolved_paths().into_iter().find(|(resolved, _)| *resolved == key).unwrap();
        assert_eq!(resolved, expected.as_os_str());
    }

    /// `daemon.socket_path` is optional, so empty it is unset rather than a default path.
    #[rstest]
    #[case::empty("", None)]
    #[case::unset_variable("$UNSET/atuin.sock", Some("$UNSET/atuin.sock"))]
    fn socket_path_loads_leniently(
        config_env: MockEnv,
        #[case] value: &str,
        #[case] expected: Option<&str>,
    ) {
        let expected =
            expected.map(|path| std::env::current_dir().unwrap().joined("config").joined(path));
        let socket_path = load(&config_env, "daemon.socket_path", Some(value)).daemon.socket_path;
        assert_eq!(socket_path.map(AbsPathBuf::into_path_buf), expected);
    }

    /// Every path setting has a default in `path_defaults`, which an empty value falls back to:
    /// without one, an empty value would stop Atuin from loading its settings at all.
    #[rstest]
    fn every_path_setting_has_a_default() {
        fn leaves(prefix: &str, value: config::Value) -> Vec<String> {
            match value.kind {
                ValueKind::Table(table) => table
                    .into_iter()
                    .flat_map(|(key, value)| leaves(&format!("{prefix}{key}."), value))
                    .collect(),
                _ => vec![prefix.trim_end_matches('.').to_owned()],
            }
        }

        let data_dir = std::env::current_dir().unwrap().joined("data");
        let defaults = Settings::builder_with_data_dir(&data_dir).unwrap().build().unwrap();
        let without_default: Vec<String> = leaves("", defaults.cache.clone())
            .into_iter()
            .filter(|key| {
                let emptied = Config::builder()
                    .add_source(defaults.clone())
                    .set_override(key.as_str(), "")
                    .unwrap()
                    .build()
                    .unwrap();
                Settings::with_lenient_paths(emptied, Settings::path_defaults(&data_dir))
                    .unwrap()
                    .try_deserialize::<Settings>()
                    .is_err_and(|err| err.to_string().contains("path is empty"))
            })
            .collect();
        assert!(
            without_default.is_empty(),
            "path settings missing from `path_defaults`: {without_default:?}"
        );
    }

    /// A path Atuin works out itself, here from a data dir with a `$` in its name, isn't expanded a
    /// second time.
    #[rstest]
    fn a_data_dir_with_a_dollar_in_its_name_loads() {
        let env = MockEnv::install();
        env.set("HOME", "/home/user");
        let data_dir = std::env::current_dir().unwrap().joined("price$usd");

        let settings: Settings = Settings::builder_with_data_dir(&data_dir)
            .unwrap()
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        assert_eq!(settings.data_dir, data_dir);
        assert_eq!(
            settings.scripts.db_path,
            DbLocation::File(AbsPathBuf::new(data_dir.join("scripts.db")).unwrap())
        );
    }

    #[rstest]
    #[case::env_overrides_the_file(Some("true"), true)]
    #[case::file_without_env(None, false)]
    fn environment_overrides_the_config_file(#[case] env: Option<&str>, #[case] expected: bool) {
        let env = Settings::environment().source(Some(
            env.map(|value| ("ATUIN_ENTER_ACCEPT".to_string(), value.to_string()))
                .into_iter()
                .collect(),
        ));
        let builder = Settings::builder_with_data_dir(Path::new("/default/data")).unwrap();
        let settings: Settings = Settings::add_config_file(
            builder,
            ConfigFile::from_str("enter_accept = false\n", FileFormat::Toml),
            env,
        )
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap();
        assert_eq!(settings.enter_accept, expected);
    }

    #[rstest]
    fn keymap_config_deserializes_simple_binding() {
        let json = r#"{"emacs": {"ctrl-c": "exit"}}"#;
        let config: super::KeymapConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.emacs.len(), 1);
        if let super::KeyBindingConfig::Simple(s) = &config.emacs["ctrl-c"] {
            assert_eq!(s, "exit");
        }
    }

    #[rstest]
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
        if let super::KeyBindingConfig::Rules(rules) = &config.emacs["left"] {
            assert_eq!(rules.len(), 2);
            assert_eq!(rules[0].when.as_deref(), Some("cursor-at-start"));
            assert_eq!(rules[0].action, "exit");
            assert!(rules[1].when.is_none());
            assert_eq!(rules[1].action, "cursor-left");
        } else {
            panic!("expected Rules variant");
        }
    }

    #[rstest]
    fn keymap_config_deserializes_vim_normal() {
        let json = r#"{"vim-normal": {"j": "select-next", "k": "select-previous"}}"#;
        let config: super::KeymapConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.vim_normal.len(), 2);
        assert!(config.emacs.is_empty());
    }

    #[rstest]
    fn keymap_config_is_empty_when_default() {
        let config = super::KeymapConfig::default();
        assert!(config.is_empty());
    }

    #[rstest]
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

    /// Deserialize a TOML string into a [`Settings`] object.
    fn parse_settings(toml: &str) -> Settings {
        let config = Settings::builder_with_data_dir(&atuin_common::dirs::unconfigured_data_dir())
            .expect("could not build settings builder")
            .add_source(ConfigFile::from_str(toml, FileFormat::Toml))
            .build()
            .expect("could not build config");
        Settings::with_legacy_threads(config)
            .expect("could not read legacy threads settings")
            .try_deserialize()
            .expect("could not deserialize config")
    }

    #[rstest]
    fn threads_default_off_with_tools() {
        let settings = parse_settings("");
        assert!(!settings.threads.capture);
        assert!(settings.threads.capture_tools);
        assert!(settings.threads.resume_chooser);
    }

    #[rstest]
    fn legacy_ai_session_settings_are_read_as_threads() {
        let settings = parse_settings(
            "[ai]\ncapture_sessions = true\ncapture_tools = \
             false\n\n[ai.sessions]\nresume_chooser = false\n\n[ai.sessions.resume]\npi = \"pi \
             --session {id}\"\n",
        );
        assert!(settings.threads.capture);
        assert!(!settings.threads.capture_tools);
        assert!(!settings.threads.resume_chooser);
        assert_eq!(settings.threads.resume.pi.as_deref(), Some("pi --session {id}"));
    }

    #[rstest]
    fn threads_settings_win_over_legacy_ones() {
        let settings = parse_settings(
            "[ai]\ncapture_sessions = true\n\n[ai.sessions]\ninline_height = 10\nmouse = \
             false\n\n[ai.sessions.resume]\nclaude = \"old\"\npi = \"old\"\n\n[threads]\ncapture \
             = false\ninline_height = 20\n\n[threads.resume]\npi = \"new\"\n",
        );
        assert!(!settings.threads.capture);
        assert_eq!(settings.threads.inline_height, Some(20));
        assert_eq!(settings.threads.mouse, Some(false));
        assert_eq!(settings.threads.resume.claude.as_deref(), Some("old"));
        assert_eq!(settings.threads.resume.pi.as_deref(), Some("new"));
    }

    /// `atuin config get --resolved` shows the defaults, as it did for `ai.capture_sessions`.
    #[rstest]
    fn threads_defaults_resolve() {
        let config = Settings::with_legacy_threads(config::Config::default()).unwrap();
        assert!(!config.get_bool("threads.capture").unwrap());
        assert!(config.get_bool("threads.capture_tools").unwrap());
    }

    #[rstest]
    fn zero_log_retention_is_accepted() {
        // `retention = 0` = keep for zero seconds (prune everything); it must load, not error.
        Settings::validate_str("[logs]\nretention = 0\n").expect("retention 0 must load");

        let settings = parse_settings("[logs]\nretention = 0\n");
        assert_eq!(settings.logs.retention, std::time::Duration::ZERO);
        // an unset per-type override inherits the global; an explicit 0 override is accepted too
        assert_eq!(settings.logs.search.retention, None);
        let overridden = parse_settings("[logs.search]\nretention = 0\n");
        assert_eq!(overridden.logs.search.retention, Some(std::time::Duration::ZERO));
    }

    #[rstest]
    fn negative_session_continue_minutes_disables_resume() {
        // Legacy clamped a negative value to 0 (disabled); it must load, not error.
        Settings::validate_str("[ai]\nsession_continue_minutes = -5\n")
            .expect("negative session_continue_minutes must load");

        let settings = parse_settings("[ai]\nsession_continue_minutes = -5\n");
        assert_eq!(settings.ai.session_continue_minutes, None);
    }

    #[rstest]
    #[case::anchored(Some(r#"["^cat "]"#), "cat .env", true)]
    #[case::anchor_holds(Some(r#"["^cat "]"#), "echo cat .env", false)]
    #[case::unanchored(Some(r#"["token"]"#), "gh auth token", true)]
    #[case::any_of_several(Some(r#"["^cat ", "token"]"#), "gh auth token", true)]
    #[case::omitted(None, "cat .env", false)]
    fn output_command_filter_matches_the_command_line(
        #[case] patterns: Option<&str>,
        #[case] command: &str,
        #[case] expected: bool,
    ) {
        let filter = patterns.map(|p| format!("command_filter = {p}\n")).unwrap_or_default();
        let settings = parse_settings(&format!("[output]\nenabled = true\n{filter}"));

        let limits = settings.output.limits().expect("output capture is enabled");
        assert_eq!(limits.command_filter.is_match(command), expected);
    }

    #[rstest]
    fn output_command_filter_rejects_an_invalid_expression() {
        assert!(Settings::validate_str("[output]\ncommand_filter = [\"(\"]\n").is_err());
    }

    /// AI session capture applies `command_filter` to the commands agents run, whether or not
    /// output capture is on.
    #[rstest]
    #[case::enabled("enabled = true\n")]
    #[case::disabled("enabled = false\n")]
    fn output_command_filter_is_kept_with_capture_off(#[case] enabled: &str) {
        let settings =
            parse_settings(&format!("[output]\n{enabled}command_filter = [\"^cat \"]\n"));
        assert!(settings.output.command_filter().is_match("cat .env"));
    }

    #[rstest]
    fn security_settings_load() {
        let settings = parse_settings(concat!(
            "[security]\n",
            "redact_patterns = [\"ACME-[0-9]{6}\"]\n",
            "sensitive_files = [\"*.secret\"]\n",
        ));
        assert_eq!(settings.security.redactor(true).redact("id ACME-123456"), "id ****");
        let files = settings.security.sensitive_files(&settings.key_path);
        assert!(files.matches("notes.secret", None));
        assert!(files.matches(&settings.key_path.to_string_lossy(), None));
        assert!(files.matches(".env", None), "the built-in files stay");
    }

    #[rstest]
    fn security_settings_default_to_the_built_in_rules() {
        let settings = parse_settings("");
        assert!(settings.security.redact_patterns.is_empty());
        assert!(settings.security.sensitive_files(&settings.key_path).matches(".env", None));
    }

    #[rstest]
    fn redact_patterns_reject_an_invalid_expression() {
        assert!(Settings::validate_str("[security]\nredact_patterns = [\"(\"]\n").is_err());
    }

    #[rstest]
    fn sync_frequency_accepts_humantime_and_bare_seconds() {
        let secs = std::time::Duration::from_secs;
        let zero = std::time::Duration::ZERO;
        // humantime strings
        assert_eq!(parse_settings("sync_frequency = \"5m\"\n").sync_frequency, secs(300));
        // a bare number (int or float) is seconds — the historical unit
        assert_eq!(parse_settings("sync_frequency = 300\n").sync_frequency, secs(300));
        assert_eq!(parse_settings("sync_frequency = 30.0\n").sync_frequency, secs(30));
        assert_eq!(parse_settings("sync_frequency = \"30\"\n").sync_frequency, secs(30));
        // `0` in any form is the documented always-sync sentinel
        assert_eq!(parse_settings("sync_frequency = 0\n").sync_frequency, zero);
        assert_eq!(parse_settings("sync_frequency = 0.0\n").sync_frequency, zero);
        assert_eq!(parse_settings("sync_frequency = \"0\"\n").sync_frequency, zero);
        // negatives remain rejected
        assert!(Settings::validate_str("sync_frequency = -5\n").is_err());
    }

    #[rstest]
    fn skim_is_requested_but_resolves_to_fuzzy() {
        let settings = parse_settings("search_mode = \"skim\"\n");

        assert_eq!(settings.requested_search_mode, RequestedSearchMode::Skim);
        assert_eq!(settings.search_mode(), SearchMode::Fuzzy);
    }

    #[rstest]
    fn skim_shell_up_key_binding_resolves_to_fuzzy() {
        let settings = parse_settings("search_mode_shell_up_key_binding = \"skim\"\n");

        assert_eq!(
            settings.requested_search_mode_shell_up_key_binding,
            Some(RequestedSearchMode::Skim)
        );
        assert_eq!(settings.search_mode_shell_up_key_binding(), Some(SearchMode::Fuzzy));
    }

    #[rstest]
    #[case("prefix", RequestedSearchMode::Prefix)]
    #[case("fulltext", RequestedSearchMode::Fulltext)]
    #[case("fuzzy", RequestedSearchMode::Fuzzy)]
    #[case("daemon-fuzzy", RequestedSearchMode::DaemonFuzzy)]
    #[case("skim", RequestedSearchMode::Skim)]
    fn requested_search_mode_parses_correctly(
        #[case] value: &str,
        #[case] expected: RequestedSearchMode,
    ) {
        let settings = parse_settings(&format!("search_mode = \"{value}\"\n"));
        assert_eq!(settings.requested_search_mode, expected);
    }

    #[rstest]
    #[case(RequestedSearchMode::Prefix, SearchMode::Prefix)]
    #[case(RequestedSearchMode::Fulltext, SearchMode::FullText)]
    #[case(RequestedSearchMode::Fuzzy, SearchMode::Fuzzy)]
    #[case(RequestedSearchMode::DaemonFuzzy, SearchMode::DaemonFuzzy)]
    // "skim" was removed -- maps to "fuzzy"
    #[case(RequestedSearchMode::Skim, SearchMode::Fuzzy)]
    fn effective_search_mode_matches_requested(
        #[case] requested: RequestedSearchMode,
        #[case] expected: SearchMode,
    ) {
        assert_eq!(requested.effective_mode(), expected);
        assert_eq!(SearchMode::from(requested), expected);
    }
}
