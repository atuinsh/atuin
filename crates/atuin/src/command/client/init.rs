use atuin_client::settings::{SearchMode, Settings, Tmux};
use atuin_common::env::var_os;
use atuin_common::userio;
use clap::{Parser, ValueEnum};
use enumset::{EnumSet, EnumSetType};
use eyre::Result;
use tracing::instrument;

use crate::i18n::fl;

mod bash;
mod fish;
mod nu;
mod powershell;
mod xonsh;
mod zsh;

#[derive(Parser, Debug)]
pub struct Cmd {
    shell: Shell,

    #[command(flatten)]
    keys: KeyFlags,

    /// No longer does anything: `?` isn't bound any more. Kept so old init lines still work.
    #[clap(long = "disable-ai", hide = true)]
    _disable_ai: bool,
}

/// The keybinding flags of `atuin init`.
///
/// The flags exist so that old init lines keep working. The supported way to choose keys is the
/// `bind_ctrl_r`, `bind_up_arrow`, and `bind_ai_resume` settings.
#[derive(clap::Args, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyFlags {
    #[clap(long, hide = true)]
    pub disable_ctrl_r: bool,

    #[clap(long, hide = true)]
    pub disable_up_arrow: bool,

    #[clap(long, hide = true)]
    pub bind_ai_resume: bool,
}

/// Each of the keys that `atuin init` can bind, on or off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keybindings {
    pub ctrl_r: bool,
    pub up_arrow: bool,
    /// ctrl-t, which resumes an agent thread.
    pub ai_resume: bool,
}

/// A set of the keys that `atuin init` can bind, regardless of whether each key is on or off.
pub type KeySet = EnumSet<Key>;

/// One of the keys that `atuin init` can bind.
#[derive(Debug, EnumSetType)]
pub enum Key {
    CtrlR,
    UpArrow,
    /// ctrl-t, which resumes an agent thread.
    AiResume,
}

impl Key {
    /// The setting in the config that turns the key on or off.
    #[must_use]
    pub const fn setting(self) -> &'static str {
        match self {
            Self::CtrlR => "bind_ctrl_r",
            Self::UpArrow => "bind_up_arrow",
            Self::AiResume => "bind_ai_resume",
        }
    }

    /// The environment variable that overrides the key's setting.
    #[must_use]
    pub const fn env_var(self) -> &'static str {
        match self {
            Self::CtrlR => "ATUIN_BIND_CTRL_R",
            Self::UpArrow => "ATUIN_BIND_UP_ARROW",
            Self::AiResume => "ATUIN_BIND_AI_RESUME",
        }
    }
}

/// The state, beyond the keys that are chosen, that decides which keys `atuin init` binds.
#[derive(Clone, Copy, Debug)]
pub struct KeyContext {
    /// Whether the daemon captures agent threads, which ctrl-t needs.
    pub threads: bool,
    /// Whether Atuin loads into a shell that can bind ctrl-t. See [`Shell::binds_ai_resume`].
    pub ai_resume_shell: bool,
}

impl KeyContext {
    /// The context in which `atuin init` runs for `shell` with `settings`.
    #[must_use]
    pub const fn new(settings: &Settings, shell: Shell) -> Self {
        Self {
            threads: settings.threads.enabled,
            ai_resume_shell: shell.binds_ai_resume(),
        }
    }
}

impl Keybindings {
    /// The keybindings that `settings` chooses.
    #[must_use]
    pub const fn from_settings(settings: &Settings) -> Self {
        Self {
            ctrl_r: settings.bind_ctrl_r,
            up_arrow: settings.bind_up_arrow,
            ai_resume: settings.bind_ai_resume,
        }
    }

    /// Whether `key` is on.
    #[must_use]
    pub const fn get(self, key: Key) -> bool {
        match key {
            Key::CtrlR => self.ctrl_r,
            Key::UpArrow => self.up_arrow,
            Key::AiResume => self.ai_resume,
        }
    }

    /// The keys that `atuin init` binds in `context` when these keys are chosen.
    ///
    /// This can differ from `self` when, e.g., threads are disabled.
    #[must_use]
    pub const fn bound(self, context: KeyContext) -> Self {
        Self {
            ai_resume: self.ai_resume && context.threads && context.ai_resume_shell,
            ..self
        }
    }
}

/// Which keybindings are currently configured via an environment variable.
///
/// This does not depend on the variable's value, only whether it is set. For example,
/// `ATUIN_BIND_CTRL_R=true` and `ATUIN_BIND_CTRL_R=false` will both put ctrl-r in the set.
#[must_use]
pub fn keys_set_by_env() -> KeySet {
    KeySet::all().iter().filter(|key| var_os(key.env_var()).is_some()).collect()
}

impl KeyFlags {
    /// Combine two sets of flags, keeping flags that appear in at least one set.
    #[must_use]
    pub const fn or(self, other: Self) -> Self {
        Self {
            disable_ctrl_r: self.disable_ctrl_r || other.disable_ctrl_r,
            disable_up_arrow: self.disable_up_arrow || other.disable_up_arrow,
            bind_ai_resume: self.bind_ai_resume || other.bind_ai_resume,
        }
    }

    /// The keys that have been requested to be bound, according to the combination of these flags
    /// and the provided settings.
    ///
    /// This method only returns the bindings that have been *requested*. There may be reasons that
    /// some of the requested bindings are not executed. [`Self::bound`] returns the bindings that
    /// will actually take effect.
    pub const fn chosen(self, settings: &Settings) -> Keybindings {
        let in_settings = Keybindings::from_settings(settings);
        Keybindings {
            ctrl_r: in_settings.ctrl_r && !self.disable_ctrl_r,
            up_arrow: in_settings.up_arrow && !self.disable_up_arrow,
            ai_resume: in_settings.ai_resume || self.bind_ai_resume,
        }
    }

    /// The keys that `atuin init` binds in `shell`, given these flags and the provided settings.
    pub const fn bound(self, settings: &Settings, shell: Shell) -> Keybindings {
        self.chosen(settings).bound(KeyContext::new(settings, shell))
    }
}

#[derive(Clone, Copy, ValueEnum, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
#[allow(clippy::enum_variant_names, clippy::doc_markdown)]
pub enum Shell {
    #[value(help = fl!("value-init-shell-zsh"))]
    Zsh,
    #[value(help = fl!("value-init-shell-bash"))]
    Bash,
    #[value(help = fl!("value-init-shell-fish"))]
    Fish,
    #[value(help = fl!("value-init-shell-nu"))]
    Nu,
    #[value(help = fl!("value-init-shell-xonsh"))]
    Xonsh,
    #[value(help = fl!("value-init-shell-powershell"))]
    PowerShell,
}

impl Shell {
    /// The name `atuin init` takes it by.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::Fish => "fish",
            Self::Nu => "nu",
            Self::Xonsh => "xonsh",
            Self::PowerShell => "powershell",
        }
    }

    /// Whether `atuin init` can bind ctrl-t to `atuin resume` in this shell.
    ///
    /// Zsh, Bash and fish can, in a build with agent support.
    pub const fn binds_ai_resume(self) -> bool {
        cfg!(feature = "ai") && matches!(self, Self::Zsh | Self::Bash | Self::Fish)
    }
}

#[allow(clippy::struct_excessive_bools)]
struct StaticInitOptions<'a> {
    pub enable_up_arrow: bool,
    pub enable_ctrl_r: bool,
    /// Bind ctrl-t to `atuin threads resume`, which replaces the shell's own binding on that key.
    /// The widgets are defined either way.
    #[cfg_attr(not(feature = "ai"), allow(dead_code))]
    pub enable_ai_resume: bool,
    /// `atuin __internal prepare-search-index` does nothing unless search uses the daemon, so a
    /// shell skips starting that process otherwise.
    pub prepare_search_index: bool,
    pub tmux: &'a Tmux,
}

impl Cmd {
    fn static_init(&self, settings: &Settings) {
        let options = self.to_options(settings);

        match self.shell {
            Shell::Zsh => {
                zsh::init_static(&options);
            }
            Shell::Bash => {
                bash::init_static(&options);
            }
            Shell::Fish => {
                fish::init_static(&options);
            }
            Shell::Nu => {
                nu::init_static(&options);
            }
            Shell::Xonsh => {
                xonsh::init_static(&options);
            }
            Shell::PowerShell => {
                powershell::init_static(&options);
            }
        }
    }

    fn to_options<'a>(&self, settings: &'a Settings) -> StaticInitOptions<'a> {
        let keys = self.keys.bound(settings, self.shell);
        StaticInitOptions {
            enable_up_arrow: keys.up_arrow,
            enable_ctrl_r: keys.ctrl_r,
            enable_ai_resume: keys.ai_resume,
            prepare_search_index: settings.search_mode() == SearchMode::DaemonFuzzy,
            tmux: &settings.tmux,
        }
    }

    /// When `pty_proxy.enabled` is set, prepend the pty-proxy exec preamble to the init script so
    /// the shell re-execs itself inside `atuin pty-proxy` — no separate `atuin pty-proxy init` line
    /// needed in shell config. The preamble checks whether a PTY proxy has already been spawned and
    /// won't spawn another, so users who still have the standalone line keep working: whichever
    /// copy runs first wins and the other no-ops.
    #[cfg(all(feature = "pty-proxy", unix))]
    fn pty_proxy_init(&self, settings: &Settings) {
        if !settings.pty_proxy.enabled {
            return;
        }

        let shell = match self.shell {
            Shell::Zsh => atuin_pty_proxy::Shell::Zsh,
            Shell::Bash => atuin_pty_proxy::Shell::Bash,
            Shell::Fish => atuin_pty_proxy::Shell::Fish,
            Shell::Nu => atuin_pty_proxy::Shell::Nu,
            Shell::Xonsh | Shell::PowerShell => {
                userio::log::warn!("init-pty-proxy-unsupported-shell");
                return;
            }
        };

        print!("{}", atuin_pty_proxy::init_script(shell));
    }

    #[cfg(not(all(feature = "pty-proxy", unix)))]
    fn pty_proxy_init(&self, settings: &Settings) {
        if settings.pty_proxy.enabled {
            userio::log::warn!("init-pty-proxy-not-built");
        }
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings) -> Result<()> {
        if !settings.paths_ok() {
            userio::log::warn!("init-paths-broken");
            return Ok(());
        }

        self.pty_proxy_init(settings);

        self.static_init(settings);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::settings::{RequestedSearchMode, Settings};
    use clap::{CommandFactory, Parser};
    use rstest::rstest;

    use super::{Cmd, Key, KeySet, Keybindings, keys_set_by_env};

    /// Each variable that chooses a key is found, regardless of its value.
    #[rstest]
    #[case::none(None, KeySet::empty())]
    #[case::ctrl_r(Some("ATUIN_BIND_CTRL_R"), Key::CtrlR.into())]
    #[case::up_arrow(Some("ATUIN_BIND_UP_ARROW"), Key::UpArrow.into())]
    #[case::ai_resume(Some("ATUIN_BIND_AI_RESUME"), Key::AiResume.into())]
    fn a_key_variable_is_found(#[case] set: Option<&str>, #[case] expected: KeySet) {
        let env = atuin_common::env::MockEnv::install();
        if let Some(var) = set {
            env.set(var, "false");
        }
        assert_eq!(keys_set_by_env(), expected);
    }

    /// The keys named in `names`, a list such as "ctrl-r up-arrow ai-resume".
    fn named(names: &str) -> Keybindings {
        let has = |name| names.split_whitespace().any(|named| named == name);
        Keybindings {
            ctrl_r: has("ctrl-r"),
            up_arrow: has("up-arrow"),
            ai_resume: has("ai-resume"),
        }
    }

    #[rstest]
    #[case::defaults_with_threads(true, "ctrl-r up-arrow ai-resume", &[], "ctrl-r up-arrow ai-resume")]
    #[case::defaults_without_threads(false, "ctrl-r up-arrow ai-resume", &[], "ctrl-r up-arrow")]
    #[case::config_leaves_resume_unbound(true, "ctrl-r up-arrow", &[], "ctrl-r up-arrow")]
    #[case::flag_binds_resume(true, "ctrl-r up-arrow", &["--bind-ai-resume"], "ctrl-r up-arrow ai-resume")]
    #[case::flag_needs_threads(false, "ctrl-r up-arrow", &["--bind-ai-resume"], "ctrl-r up-arrow")]
    #[case::config_turns_keys_off(true, "", &[], "")]
    #[case::flags_win_over_config(true, "ctrl-r up-arrow ai-resume", &["--disable-ctrl-r", "--disable-up-arrow"], "ai-resume")]
    fn keys_follow_the_config_and_the_flags(
        #[case] threads: bool,
        #[case] config: &str,
        #[case] args: &[&str],
        #[case] expected: &str,
    ) {
        let mut settings = Settings::utc();
        settings.threads.enabled = threads;
        let config = named(config);
        settings.bind_ctrl_r = config.ctrl_r;
        settings.bind_up_arrow = config.up_arrow;
        settings.bind_ai_resume = config.ai_resume;
        let cmd = Cmd::try_parse_from([&["init", "zsh"], args].concat()).unwrap();
        let options = cmd.to_options(&settings);
        let bound = Keybindings {
            ctrl_r: options.enable_ctrl_r,
            up_arrow: options.enable_up_arrow,
            ai_resume: options.enable_ai_resume,
        };
        let mut expected = named(expected);
        expected.ai_resume &= cfg!(feature = "ai");
        assert_eq!(bound, expected);
    }

    /// ctrl-t is bound only in the shells that have a widget for it, and only in builds with agent
    /// support.
    #[rstest]
    #[case::zsh("zsh", true)]
    #[case::bash("bash", true)]
    #[case::fish("fish", true)]
    #[case::nu("nu", false)]
    #[case::xonsh("xonsh", false)]
    #[case::powershell("powershell", false)]
    fn resume_is_bound_only_where_a_widget_runs_it(#[case] shell: &str, #[case] widget: bool) {
        let mut settings = Settings::utc();
        settings.threads.enabled = true;
        settings.bind_ai_resume = true;
        let cmd = Cmd::try_parse_from(["init", shell]).unwrap();
        let bound = cmd.to_options(&settings).enable_ai_resume;
        assert_eq!(bound, widget && cfg!(feature = "ai"));
    }

    #[rstest]
    #[case("--disable-ctrl-r")]
    #[case("--disable-up-arrow")]
    #[case("--bind-ai-resume")]
    #[case("--disable-ai")]
    fn keybinding_flags_are_hidden(#[case] flag: &str) {
        assert!(Cmd::try_parse_from(["init", "zsh", flag]).is_ok());
        let help = Cmd::command().render_long_help().to_string();
        assert!(!help.contains(flag.trim_start_matches("--")), "{help}");
    }

    /// Only the daemon search engine has an index to prepare.
    #[rstest]
    #[case::fuzzy(RequestedSearchMode::Fuzzy, false)]
    #[case::prefix(RequestedSearchMode::Prefix, false)]
    #[case::full_text(RequestedSearchMode::Fulltext, false)]
    #[case::daemon(RequestedSearchMode::DaemonFuzzy, true)]
    fn search_index_is_prepared_only_for_the_daemon(
        #[case] mode: RequestedSearchMode,
        #[case] prepared: bool,
    ) {
        let mut settings = Settings::utc();
        settings.requested_search_mode = mode;
        let cmd = Cmd::try_parse_from(["init", "zsh"]).unwrap();
        assert_eq!(cmd.to_options(&settings).prepare_search_index, prepared);
    }
}
