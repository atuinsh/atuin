//! `atuin setup`: one screen of choices, filled in from what is on this machine, then a live list
//! of what it is doing.

use std::io::{self, IsTerminal};
#[cfg(all(feature = "ai", feature = "daemon"))]
use std::num::NonZeroU64;
#[cfg(feature = "ai")]
use std::num::NonZeroUsize;
use std::process::Stdio;
use std::sync::Arc;
#[cfg(feature = "ai")]
use std::time::{Duration, SystemTime};

use atuin_client::database::Sqlite;
use atuin_client::meta::SyncChoice;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::desktop_capture::{APPS_KEY, ENABLED_KEY};
use atuin_client::settings::{Apps, RequestedSearchMode, Settings};
use atuin_common::encryption::paseto_v4;
use atuin_common::env::var_nonempty;
use atuin_common::executable::is_executable_with_path_search;
use atuin_common::fs::can_write;
#[cfg(feature = "ai")]
use atuin_common::harnesstools::{
    InstallHookError,
    mcp::InstallMcp as _,
    session::any::{ObservableExt as _, SessionsExt as _},
};
#[cfg(feature = "ai")]
use atuin_common::sync::BlockingPool;
#[cfg(feature = "ai")]
use atuin_common::time::DurationExt as _;
use clap::Parser;
use colored::Colorize;
#[cfg(feature = "ai")]
use easy_cast::Conv;
use eyre::Result;
use tokio::sync::mpsc;
use toml_edit::Value;
use tracing::instrument;

mod agents;
#[cfg(feature = "desktop-capture")]
mod desktop;
mod form;
#[cfg(feature = "octavo")]
mod hub;
mod plan;
mod scroll;
mod shell_init;
#[cfg(feature = "octavo")]
mod upload;
#[cfg(feature = "octavo")]
mod users_file;
mod view;
mod widgets;

use agents::AnyHarness;
#[cfg(feature = "ai")]
use agents::Harness;
#[cfg(feature = "desktop-capture")]
use desktop::{AppsWrite, DesktopChange, capture_desktop};
use form::{Flag, Form as _};
#[cfg(feature = "ai")]
use plan::{AgentHistory, Mcp};
use plan::{
    AiResume, Asks, AtuinHub, Choices, Confirm, CtrlR, Daemon, DesktopApps, DesktopCapture,
    OutputCapture, Plan, ShellHistory, ThreadUploads, Threads, UpArrow,
};
use shell_init::{
    RcDirs, Stripped, Unhooked, install_shell_init, remove_shell_init, shell_inits, shells_here,
    strip_init_flags,
};

use super::config::{every_app_value, listed_apps_value, update_config_file};
use super::init::{Key, KeyContext, KeySet, Keybindings, Shell, keys_set_by_env};

/// Each flag sets a choice on the first screen, which starts from what's found here.
#[derive(Parser, Debug)]
pub struct Cmd {
    /// Set up without asking, as the flags say: sync is off unless `--sync` says otherwise
    #[arg(long)]
    yes: bool,

    /// Change only what the flags say, without asking, and leave the rest as it is
    #[arg(long)]
    only: bool,

    /// Don't import the shell history and threads already on this machine
    #[arg(long)]
    no_import: bool,

    #[command(flatten)]
    choices: Choices,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings, db: &Sqlite, store: &SqliteStore) -> Result<()> {
        let confirm = if self.yes || self.only || !io::stdout().is_terminal() {
            Confirm::Yes
        } else {
            Confirm::Ask
        };
        let found = Found::detect(settings, db).await;
        let plan = self.plan(&found, confirm)?;

        if confirm == Confirm::Yes {
            let plan = plan.effective();
            if plan.sync == Sync::Cloud && found.syncing() != Some(Sync::Cloud) {
                eyre::bail!("signing in to Atuin Cloud needs you: run `atuin setup` without --yes");
            }
            let scope = self.scope();
            let (tx, mut rx) = mpsc::unbounded_channel();
            let (_answer, answers) = mpsc::unbounded_channel();
            let offer = Box::pin(apply(&plan, &found, scope, store, &tx, answers)).await?;
            if self.no_import {
                skip_import(&offer, &tx);
            } else {
                import(&offer, &tx, plan.waits_for_imports(), plan.daemon.0).await;
            }
            #[cfg(feature = "octavo")]
            if let Some(Hub::On { backfill }) = plan.hub() {
                // Not watched: there's no screen to watch it on.
                let _upload = turn_on_hub(store, backfill, plan.upload_threads.0, &tx).await;
            }
            #[cfg(feature = "desktop-capture")]
            desktop::report_what_capture_waits_for(&plan, scope, settings, &tx).await;
            let mut steps = Vec::new();
            while let Ok(update) = rx.try_recv() {
                match update {
                    Update::Step(step, status) => view::set(&mut steps, step, status),
                    #[cfg(feature = "desktop-capture")]
                    Update::Also(step, status) => view::also(&mut steps, step, status),
                    Update::Browser { .. } | Update::NeedKey(_) => {}
                }
            }
            print_summary(&plan, &found, scope, &steps).await;
            return Ok(());
        }

        let mouse = if settings.no_mouse {
            view::Mouse::Leave
        } else {
            view::Mouse::Capture
        };
        Box::pin(view::run(found, &plan, settings, !self.no_import, mouse, store)).await
    }

    /// What setup sets up: with `--only`, just what the flags name.
    fn scope(&self) -> Scope<'_> {
        if self.only {
            Scope::Flagged(self.choices.matches())
        } else {
            Scope::All
        }
    }

    /// What's set up here, with what setup proposes unless `--only` says to leave it, and then the
    /// flags: the first screen starts from this, and `--yes` sets it up.
    fn plan(&self, found: &Found, confirm: Confirm) -> Result<Plan, form::FlagError> {
        self.plan_for(found, confirm, Login::current())
    }

    /// [`plan`](Self::plan) for a user at the screen, so that a test's plan doesn't depend on
    /// where the test runs.
    #[cfg(test)]
    fn plan_at_screen(&self, found: &Found, confirm: Confirm) -> Result<Plan, form::FlagError> {
        self.plan_for(found, confirm, Login::AtScreen)
    }

    /// [`plan`](Self::plan) for a user who is logged in at `login`.
    fn plan_for(
        &self,
        found: &Found,
        confirm: Confirm,
        login: Login,
    ) -> Result<Plan, form::FlagError> {
        let flags = self.choices.matches();
        let asks = Asks {
            // As long as Atuin is in one, so it can come out.
            agents: !found.agents.is_empty()
                || !found.connections.is_empty()
                || plan::Agents::from_args(flags).is_some(),
            hub: AtuinHub::from_args(flags).is_some(),
            searchable: found.imported > 0 || found.threads_here > 0,
            unhooks: self.only && ShellHistory::from_args(flags) == Some(ShellHistory(false)),
            removes_desktop_capture: DesktopCapture::from_args(flags)
                == Some(DesktopCapture(false)),
            away_from_screen: !found.desktop_capture && login == Login::Elsewhere,
        };
        let here = Plan::from_current_state(found, asks);
        let mut plan = if self.only {
            here
        } else {
            here.proposal(found, confirm)
        };
        plan.set_from(flags);
        plan.check(flags)?;
        Ok(plan)
    }
}

/// Why `old`, a command setup took over, fails: `atuin setup --only <flags>` does what it did.
pub fn moved_here(old: &str, flags: &str) -> eyre::Report {
    eyre::eyre!("`{old}` is now part of `atuin setup`: run `atuin setup --only {flags}`")
}

/// What setup sets up.
#[derive(Clone, Copy, Debug)]
pub enum Scope<'a> {
    /// All the plan says.
    All,
    /// Only what these flags name, as `--only` asks.
    Flagged(&'a clap::ArgMatches),
}

impl Scope<'_> {
    /// Whether setup sets up `C`.
    fn covers<C: Flag>(self) -> bool {
        match self {
            Self::All => true,
            Self::Flagged(matches) => C::from_args(matches).is_some(),
        }
    }

    /// `C` as its flags set it, setting up only what they name, before the rest of the plan rules
    /// any of it out.
    fn flagged<C: Flag>(self) -> Option<C> {
        match self {
            Self::All => None,
            Self::Flagged(matches) => C::from_args(matches),
        }
    }
}

/// Agents' hooks and Atuin's MCP server need the `ai` feature.
const AGENTS: bool = cfg!(feature = "ai");

/// Desktop capture needs the `desktop-capture` feature, and reads the accessibility APIs of Linux
/// and macOS.
const DESKTOP_CAPTURE: bool =
    cfg!(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")));

/// Where the user who runs setup is logged in, which decides whether desktop capture can get its
/// permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Login {
    /// At the screen, or on a platform that doesn't tie the permission to the screen.
    AtScreen,
    /// Elsewhere, such as over SSH, on a Mac.
    Elsewhere,
}

impl Login {
    fn current() -> Self {
        #[cfg(target_os = "macos")]
        if !atuin_common::os::macos::console::is_at_screen() {
            return Self::Elsewhere;
        }
        Self::AtScreen
    }
}

/// Agent threads need a daemon to capture them, and the `ai` feature to read them.
const THREADS: bool = cfg!(all(feature = "ai", feature = "daemon"));

/// Sync needs the sync client, which `sync` builds in.
const SYNC: bool = cfg!(feature = "sync");

/// Atuin Hub needs the hub client, which `octavo` builds in.
const HUB: bool = cfg!(feature = "octavo");

/// `--sync`: `cloud`, `off`, or a server's address.
fn parse_sync(value: &str) -> Result<Sync, String> {
    match value {
        "cloud" => Ok(Sync::Cloud),
        "off" => Ok(Sync::Off(OffBecause::Chosen)),
        address if is_address(address) => Ok(Sync::Server(address.to_string())),
        _ => {
            Err("expected cloud, off, or your server's address, starting http:// or https://"
                .into())
        }
    }
}

/// An http(s) address, as a sync server has.
fn is_address(address: &str) -> bool {
    matches!(url::Url::parse(address), Ok(url) if url.scheme().starts_with("http"))
}

/// What is on this machine, for the first screen and what setup does.
#[expect(clippy::struct_excessive_bools, reason = "what was found, each yes or no")]
pub struct Found {
    /// The current shell and the commands in its history file (0 without one), for the shells
    /// whose history setup can count.
    history: Option<(&'static str, usize)>,
    /// Commands Atuin already has, from an earlier import or from use.
    imported: i64,
    /// Threads Atuin already has here.
    threads_here: usize,
    /// The agents here, each with how many threads it has on this machine.
    agents: Vec<(AnyHarness, u64)>,
    /// Whether the shell setup runs from already loads Atuin.
    hooked: bool,
    /// The shells whose rc file loads Atuin.
    inits: Vec<Shell>,
    /// The shells here setup can hook into: those on `PATH`, or with an rc file.
    shells: Vec<Shell>,
    /// The keys that the config and the `atuin init` flags turn on, which the key rows of the
    /// form start from. `atuin init` binds ctrl-t only while threads are on as well.
    keys: Keybindings,
    /// Whether setup can take the keybinding flags off the `atuin init` lines and write the keys
    /// that they choose into the config. Setup can't if any of those files is read-only, or if an
    /// environment variable chooses a key.
    flags_movable: bool,
    /// The keys whose binding status is currently set by an environment variable, like
    /// `ATUIN_BIND_UP_ARROW=false`.
    keys_set_by_env: KeySet,
    /// Whether the daemon captures agent threads, as configured now.
    threads: bool,
    /// Whether output is captured, as configured now.
    output: bool,
    /// Whether the desktop is captured, as configured now.
    desktop_capture: bool,
    /// Which applications desktop capture reads, as the first screen offers it.
    desktop_apps: DesktopApps,
    /// Whether the daemon keeps the output proxies send it, `output.enabled`, which it does for
    /// one started by hand without `pty_proxy.enabled`.
    saves_output: bool,
    /// Whether the daemon runs, as configured now.
    daemon: bool,
    /// Whether a daemon is running, configured to or not, as one started by hand is.
    daemon_running: bool,
    /// Whether they turned the daemon off: `daemon.enabled = false` in their config or their
    /// environment, as `atuin setup --only --no-daemon` writes, rather than left unset.
    daemon_off: bool,
    signed_in: bool,
    /// Whether they turned sync off in an earlier setup, or skipped signing in there, and haven't
    /// signed in or chosen somewhere to sync since.
    sync_choice: SyncChoice,
    /// A sync server that isn't Atuin Cloud, already configured.
    server: Option<String>,
    /// Which agent threads Atuin Hub uploads while it's on, as configured now.
    upload_threads: ThreadUploads,
    /// The agents Atuin is in, and how.
    connections: Vec<(AnyHarness, Connection)>,
    /// Whether Atuin Hub is on, as configured now.
    hub: bool,
}

/// The keys whose binding status is decided by `plan`, in the context of the given `scope`.
fn planned_keys(plan: &Plan, scope: Scope<'_>) -> KeySet {
    let covered = |key| match key {
        Key::CtrlR => scope.covers::<CtrlR>(),
        Key::UpArrow => scope.covers::<UpArrow>(),
        Key::AiResume => plan.threads.0 && scope.covers::<AiResume>(),
    };
    KeySet::all().iter().filter(|&key| plan.shell.0 && covered(key)).collect()
}

/// The keybindings that will be enabled once setup applies `plan`. Setup writes these to the config
/// before it takes any corresponding flags off the init lines.
///
/// A key that `plan` doesn't decide keeps what `found` read, so that taking its flag off changes
/// nothing. `plan` decides ctrl-r and the up arrow while shell history is on and `scope` covers
/// them, and ctrl-t only while threads are on as well.
///
/// The returned set of keys may be different from the keys that will actually get bound (for
/// example, if threads are disabled); see [`bound_after`].
fn keys_after(plan: &Plan, found: &Found, scope: Scope<'_>) -> Keybindings {
    let planned = planned_keys(plan, scope);
    Keybindings {
        ctrl_r: if planned.contains(Key::CtrlR) {
            plan.ctrl_r.0
        } else {
            found.keys.ctrl_r
        },
        up_arrow: if planned.contains(Key::UpArrow) {
            plan.up_arrow.0
        } else {
            found.keys.up_arrow
        },
        ai_resume: if planned.contains(Key::AiResume) {
            plan.ai_resume.0
        } else {
            found.keys.ai_resume
        },
    }
}

/// What of Atuin an agent has: hooks that run it, and its MCP server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Connection {
    hooks: bool,
    mcp: bool,
}

impl Found {
    async fn detect(settings: &Settings, db: &Sqlite) -> Self {
        let hooked = var_nonempty("ATUIN_SESSION").is_some();
        let dirs = RcDirs::here();
        let inits = shell_inits(&dirs).await;
        let keys_set_by_env = keys_set_by_env();
        #[cfg(feature = "octavo")]
        let (upload_threads, hub) = hub::found().await;
        // As a build with Atuin Hub would read an unset config.
        #[cfg(not(feature = "octavo"))]
        let (upload_threads, hub) = (ThreadUploads::All, false);
        Self {
            history: shell_history().await,
            // Less `atuin setup` itself, which a shell that runs Atuin has recorded already.
            imported: (db.history_count(false).await.unwrap_or(0) - i64::from(hooked)).max(0),
            threads_here: threads_here(settings).await,
            agents: detect_agents().await,
            hooked,
            inits: inits.shells,
            shells: shells_here(&dirs),
            keys: inits.flags.chosen(settings),
            // An environment variable can disagree with the config file. Setup could then end up
            // removing an init flag like `--bind-ai-resume` without setting `bind_ai_resume = true`
            // in the config because `ATUIN_BIND_AI_RESUME=true` was overriding it. So, if any
            // variable chooses a key, setup will not remove any init flags.
            flags_movable: inits.strippable
                && keys_set_by_env.is_empty()
                && Settings::get_config_path().is_ok_and(|config| can_write(&config)),
            keys_set_by_env,
            threads: settings.threads.enabled,
            output: settings.output.limits().is_some() && settings.pty_proxy.enabled,
            desktop_capture: settings.desktop_capture.enabled,
            desktop_apps: DesktopApps::found(&settings.desktop_capture),
            saves_output: settings.output.limits().is_some(),
            daemon: settings.daemon.enabled,
            daemon_running: daemon_running(settings),
            daemon_off: !settings.daemon.enabled && daemon_setting_is_theirs().await,
            signed_in: signed_in(settings).await,
            sync_choice: sync_choice(settings).await,
            server: (!settings.is_hub_sync()).then(|| settings.sync_address.to_string()),
            upload_threads,
            connections: connections().await,
            hub,
        }
    }

    fn agents(&self) -> Vec<AnyHarness> {
        self.agents.iter().map(|&(agent, _)| agent).collect()
    }

    /// The agents Atuin is in, and those with its hooks, or its MCP server.
    fn connected(&self) -> Vec<AnyHarness> {
        self.connections.iter().map(|&(agent, _)| agent).collect()
    }

    fn hooked_agents(&self) -> Vec<AnyHarness> {
        self.connections.iter().filter(|(_, how)| how.hooks).map(|&(agent, _)| agent).collect()
    }

    /// Whether a run over `scope` takes the keybinding flags off the `atuin init` lines.
    ///
    /// A run does this if the flags can move and the run covers a key, shell history, or the
    /// shells, which are the parts of setup that the init lines serve.
    fn strips_flags(&self, scope: Scope<'_>) -> bool {
        self.flags_movable
            && (scope.covers::<CtrlR>()
                || scope.covers::<UpArrow>()
                || scope.covers::<AiResume>()
                || scope.covers::<ShellHistory>()
                || scope.covers::<plan::Shells>())
    }

    /// Whether turning output capture `on` or off changes anything. Off, it stops the daemon
    /// keeping output, even with no proxy setup started.
    const fn output_changes(&self, on: bool) -> bool {
        if on {
            !self.output
        } else {
            self.saves_output
        }
    }

    fn mcp_agents(&self) -> Vec<AnyHarness> {
        self.connections.iter().filter(|(_, how)| how.mcp).map(|&(agent, _)| agent).collect()
    }

    fn threads(&self) -> u64 {
        self.agents.iter().map(|&(_, threads)| threads).sum()
    }

    /// Where this machine syncs now, if it's signed in.
    fn syncing(&self) -> Option<Sync> {
        self.signed_in.then(|| self.server.clone().map_or(Sync::Cloud, Sync::Server))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sync {
    Cloud,
    Server(String),
    Off(OffBecause),
}

impl Default for Sync {
    fn default() -> Self {
        Self::Off(OffBecause::Chosen)
    }
}

/// Why sync is off, which decides whether later setups keep it off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffBecause {
    /// They turned it off, on the screen or with `--sync off`.
    Chosen,
    /// `--yes` left it off because nobody's there to sign in, so a later setup still offers it.
    NobodyToSignIn,
}

/// Whether Atuin Hub is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hub {
    Off,
    /// Uploading what Atuin records from now on, and with `backfill` what it has already.
    On {
        backfill: bool,
    },
}

/// The lines of the list setup shows while it works, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// The config `--only` changed, which no other step tells of.
    Config,
    Shells,
    Output,
    #[cfg(feature = "desktop-capture")]
    DesktopCapture,
    /// Another `atuin` binary that will stop desktop capture. The step's explanation needs a line
    /// of its own to read whole.
    #[cfg(feature = "desktop-capture")]
    DesktopCaptureConflict,
    #[cfg(feature = "ai")]
    AgentHooks,
    #[cfg(feature = "ai")]
    Mcp,
    Daemon,
    Sync,
    #[cfg(feature = "octavo")]
    Hub,
    History,
    Threads,
    /// Watching Atuin Hub upload, after the imports it takes in.
    #[cfg(feature = "octavo")]
    Upload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Running(String),
    Done(String),
    Failed(String),
}

impl Status {
    /// This status with the phrase of `later` after its own, as [`summary`] joins phrases. The
    /// joined status fails when either status failed, and `later` replaces a running status.
    #[cfg(feature = "desktop-capture")]
    fn and(self, later: Self) -> Self {
        let failed = matches!(self, Self::Failed(_)) || matches!(later, Self::Failed(_));
        let (Self::Done(mut text) | Self::Failed(mut text)) = self else {
            return later;
        };
        let (Self::Done(added) | Self::Failed(added)) = &later else {
            return later;
        };
        text.push_str("; ");
        let mut chars = added.chars();
        text.extend(chars.next().into_iter().flat_map(char::to_lowercase));
        text.push_str(chars.as_str());
        if failed {
            Self::Failed(text)
        } else {
            Self::Done(text)
        }
    }
}

/// What the steps tell the screen.
#[derive(Debug)]
pub enum Update {
    Step(Step, Status),
    /// A status whose phrase joins what the step says already, as [`Status::and`] joins them.
    #[cfg(feature = "desktop-capture")]
    Also(Step, Status),
    /// Sign-in is waiting on the browser, at this address, showing this code.
    Browser {
        url: String,
        code: String,
        opened: bool,
    },
    /// The account has data this machine's key can't read, so sign-in needs the right key. The
    /// error says what was wrong with the last one entered.
    NeedKey(Option<String>),
}

/// What the screen tells sign-in.
#[derive(Debug)]
pub enum Answer {
    OpenAgain,
    Skip,
    Key(String),
}

type Updates = mpsc::UnboundedSender<Update>;

fn send(tx: &Updates, step: Step, status: Status) {
    let _ = tx.send(Update::Step(step, status));
}

/// Adds the phrase of `status` to what `step` says already.
#[cfg(feature = "desktop-capture")]
fn send_also(tx: &Updates, step: Step, status: Status) {
    let _ = tx.send(Update::Also(step, status));
}

/// Do what `plan` says, telling the screen as it goes, up to the imports and Atuin Hub.
/// Sign-in, if any, reads `answers`. Returns what there is to import, now sign-in has said what
/// the account holds.
async fn apply(
    plan: &Plan,
    found: &Found,
    scope: Scope<'_>,
    store: &SqliteStore,
    tx: &Updates,
    answers: mpsc::UnboundedReceiver<Answer>,
) -> Result<Offer> {
    let changed = write_config(plan, found, scope).await?;
    if matches!(scope, Scope::Flagged(_)) && !changed.is_empty() {
        let changed = format!("Set {} in your config", join_with_and(&changed));
        send(tx, Step::Config, Status::Done(changed));
    }
    #[cfg(feature = "octavo")]
    if plan.hub() == Some(Hub::Off) && found.hub {
        hub::disable(tx).await;
    }

    // The config now holds the keys that the flags chose, so the flags can come off without
    // changing the key settings. Taking the flags off now increases the chance that the `atuin
    // init` lines can be successfully removed if shell history is being turned off.
    let notes = if found.strips_flags(scope) {
        strip_init_flags(&RcDirs::here()).to_notes()
    } else {
        ShellNotes::default()
    };
    let settings = Arc::new(Settings::new()?);
    // Before anything writes records, so the daemon and sign-in agree on it.
    paseto_v4::Key::try_load_or_generate(&settings.key_path)?;

    let hooks = async {
        if scope.covers::<ShellHistory>() || scope.covers::<plan::Shells>() {
            let named = scope.flagged::<plan::Shells>().map(|shells| shells.0);
            let shells = changes::<_, ShellHistory>(
                scope,
                plan.shell.0,
                &plan.shells.0,
                &found.inits,
                named.as_deref(),
                PartialEq::eq,
            );
            let elsewhere = !plan.shell.0 && found.hooked && found.inits.is_empty();
            hook_shells(&shells, elsewhere, notes, tx);
        } else if let Some(status) = summary(notes.done, notes.failed) {
            send(tx, Step::Shells, status);
        }
        #[cfg(feature = "ai")]
        connect(plan, found, scope, &settings, tx).await;
        if scope.covers::<OutputCapture>() && found.output_changes(plan.output.0) {
            capture_output(plan.output.0, &settings, tx).await;
        }
        #[cfg(feature = "desktop-capture")]
        if let Some(change) = DesktopChange::of(plan, found, scope) {
            let apps = if changed.contains(&APPS_KEY) {
                AppsWrite::Written
            } else {
                AppsWrite::Kept
            };
            capture_desktop(change, apps, &settings, tx).await;
        }
        if stops_daemon(plan, found, scope) {
            stop_daemon(&settings, tx).await;
        }
        if wakes_daemon(plan, found, scope) {
            start_daemon(&settings, tx).await;
        }
    };
    if !scope.covers::<Sync>() {
        hooks.await;
        return Ok(Offer::default());
    }
    let sync = sync(plan, found, &settings, store, tx, answers);

    let ((), signed_in) = tokio::join!(hooks, sync);
    let signed_in = match signed_in {
        // That restarts the daemon.
        SignedIn::NeedsKey(key) => {
            let stored = use_account_key(&settings, store, &key, tx).await;
            if stored {
                sync_soon(&settings, plan.daemon.0, tx).await;
            } else {
                sign_out().await;
            }
            stored
        }
        SignedIn::Yes => {
            sync_soon(&settings, plan.daemon.0, tx).await;
            true
        }
        SignedIn::No => found.syncing().as_ref() == Some(&plan.sync),
    };
    let account = if signed_in {
        account(&settings, store).await
    } else if matches!(plan.sync, Sync::Server(_)) {
        Account::Unknown
    } else {
        Account::None
    };
    if matches!(scope, Scope::Flagged(_)) {
        return Ok(Offer::default());
    }
    Ok(offer(plan, found, account, tx))
}

/// Turn Atuin Hub on, once sign-in has said who the account is, so the hub can say whether
/// it has it, and after the imports, so what it uploads takes them in. Returns the upload to
/// watch, if it started one.
#[cfg(feature = "octavo")]
async fn turn_on_hub(
    store: &SqliteStore,
    backfill: bool,
    uploads: ThreadUploads,
    tx: &Updates,
) -> Option<upload::Upload> {
    match Settings::new() {
        Ok(settings) => hub::enable(&Arc::new(settings), store, backfill, uploads, tx).await,
        Err(e) => {
            let failed =
                format!("Couldn't turn on Atuin Hub ({e:#}). Run `atuin setup --hub` to try again");
            send(tx, Step::Hub, Status::Failed(failed));
            None
        }
    }
}

/// What the account this machine syncs with has already. Importing that here before its data
/// arrives would add it twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Account {
    /// No account: sync is off, or sign-in was skipped.
    None,
    Has {
        history: bool,
        threads: bool,
    },
    /// Their own server, which they sign in to after setup.
    Unknown,
}

/// What setup offers to import, for the daemon to do in the background.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offer {
    /// The shell, and the commands in its history file.
    history: Option<(&'static str, usize)>,
    threads: Option<u64>,
}

impl Offer {
    const fn is_empty(&self) -> bool {
        self.history.is_none() && self.threads.is_none()
    }
}

/// What's here that neither Atuin nor the account has yet. Says why the rest isn't on offer.
fn offer(plan: &Plan, found: &Found, account: Account, tx: &Updates) -> Offer {
    let (account_history, account_threads) = match account {
        Account::None => (false, false),
        Account::Has { history, threads } => (history, threads),
        Account::Unknown => (true, true),
    };
    let later = |what: &str, command: &str| {
        Some(if account == Account::Unknown {
            format!("Once you've signed in, run `{command}` to add this machine's {what}")
        } else {
            format!("Your account has {what} already: run `{command}` to add this machine's too")
        })
    };

    let mut offer = Offer::default();
    if plan.shell.0 {
        let status = match found.history {
            _ if found.imported == 1 => Some("1 command already in Atuin".into()),
            _ if found.imported > 0 => {
                Some(format!("{} commands already in Atuin", count(found.imported)))
            }
            None | Some((_, 0)) => Some("No shell history to import".into()),
            Some(_) if account_history => later("history", "atuin import auto"),
            Some(history) => {
                offer.history = Some(history);
                None
            }
        };
        if let Some(status) = status {
            send(tx, Step::History, Status::Done(status));
        }
    }
    if THREADS && plan.threads.0 {
        let status = match found.threads() {
            _ if found.threads_here == 1 => Some("1 thread already in Atuin".into()),
            _ if found.threads_here > 0 => {
                Some(format!("{} threads already in Atuin", count(found.threads_here)))
            }
            0 => Some("Capturing threads from now on".into()),
            _ if account_threads => later("threads", "atuin threads import"),
            threads => {
                offer.threads = Some(threads);
                None
            }
        };
        if let Some(status) = status {
            send(tx, Step::Threads, Status::Done(status));
        }
    }
    offer
}

/// They turned the offer down: say how to import later.
fn skip_import(offer: &Offer, tx: &Updates) {
    if offer.history.is_some() {
        let later = "Shell history not imported. Run `atuin import auto` any time";
        send(tx, Step::History, Status::Done(later.into()));
    }
    if offer.threads.is_some() {
        let later = "Threads not imported. Run `atuin threads import` any time";
        send(tx, Step::Threads, Status::Done(later.into()));
    }
}

/// Hand what they took up the offer of to the daemon, which imports it in the background, or, if
/// `wait`, before this returns. Without the `daemon`, import here.
#[cfg(feature = "daemon")]
async fn import(offer: &Offer, tx: &Updates, wait: bool, daemon: bool) {
    async fn history(shell: &str, wait: bool) -> Result<()> {
        let settings = Settings::new()?;
        let var = |name| var_nonempty(name).map(|v| v.to_string_lossy().into_owned());
        let mut client = super::daemon::ready_client(&settings).await?;
        client.import_history(shell, var("HISTFILE"), var("fish_history"), wait).await
    }
    if !daemon {
        import_here(offer, tx).await;
        return;
    }
    let (done, later) = if wait {
        ("Imported", "")
    } else {
        ("Importing", " in the background")
    };

    if let Some((shell, commands)) = offer.history {
        let commands = count(commands);
        send(tx, Step::History, Status::Running(format!("Importing {commands} commands")));
        let status = match history(shell, wait).await {
            Ok(()) => Status::Done(format!("{done} {commands} commands{later}")),
            Err(e) => Status::Failed(format!(
                "Couldn't import your shell history ({e:#}). Try `atuin import auto`"
            )),
        };
        send(tx, Step::History, status);
    }
    #[cfg(feature = "ai")]
    if let Some(threads) = offer.threads {
        let threads = if threads == 1 {
            "1 thread".to_owned()
        } else {
            format!("{} threads", count(threads))
        };
        send(tx, Step::Threads, Status::Running(format!("Importing {threads}")));
        let status = match import_threads(wait).await {
            Ok(None) => Status::Done(format!("{done} {threads}{later}")),
            Ok(Some(failed)) => Status::Failed(format!(
                "Imported {threads}, but couldn't read {} of their lines. Try `atuin threads \
                 import`",
                count(failed)
            )),
            Err(e) => Status::Failed(format!(
                "Couldn't import your threads ({e:#}). Try `atuin threads import`"
            )),
        };
        send(tx, Step::Threads, status);
    }
}

/// Have the daemon import every harness's threads, in the background unless `wait`. Returns how
/// many transcript lines and directory reads failed, if it waited and any did.
#[cfg(all(feature = "ai", feature = "daemon"))]
async fn import_threads(wait: bool) -> Result<Option<NonZeroU64>> {
    use atuin_daemon::grpc::ai::session::pb::import_sessions_event::Event;

    let settings = Settings::new()?;
    // A daemon already running picks the new config up itself; this starts one that isn't.
    super::daemon::ready_client(&settings).await?;
    let mut client = atuin_daemon::AiClient::from_settings(&settings).await?;
    client.wait_for_sessions(|_| {}).await?;
    if !wait {
        client.import_sessions_in_background().await?;
        return Ok(None);
    }
    let mut events = client.import_sessions(None).await?;
    let mut failed = None;
    while let Some(event) = events.message().await? {
        // A line or directory it couldn't read counts here, not as a stream error.
        if let Some(Event::Summary(summary)) = event.event {
            failed = NonZeroU64::new(summary.failed);
        }
    }
    Ok(failed)
}

#[cfg(not(feature = "daemon"))]
async fn import(offer: &Offer, tx: &Updates, _wait: bool, _daemon: bool) {
    import_here(offer, tx).await;
}

/// Import shell history here, with no daemon to hand it to, before returning.
async fn import_here(offer: &Offer, tx: &Updates) {
    if let Some((shell, _)) = offer.history {
        send(tx, Step::History, Status::Running("Importing your shell history".into()));
        let status = match quietly(&["import", shell]).await {
            Ok(()) => Status::Done("Imported your shell history".into()),
            Err(e) => Status::Failed(format!(
                "Couldn't import your shell history ({e}). Try `atuin import auto`"
            )),
        };
        send(tx, Step::History, status);
    }
}

/// How sign-in went.
pub enum SignedIn {
    No,
    Yes,
    /// The account's data needs this key, not this machine's.
    NeedsKey(paseto_v4::Key),
}

/// Swap in the key the account needs. Returns whether that worked.
#[cfg(feature = "sync")]
async fn use_account_key(
    settings: &Settings,
    store: &SqliteStore,
    key: &paseto_v4::Key,
    tx: &Updates,
) -> bool {
    send(tx, Step::Sync, Status::Running("Re-encrypting with your account's key".into()));
    // What it says on the way would land in the middle of the screen.
    let quiet = atuin_common::userio::log::SINK.push(atuin_common::userio::log::Nop);
    let stored = super::account::login::store_key(settings, store, key).await;
    drop(quiet);
    if let Err(e) = &stored {
        let failed = format!(
            "Couldn't switch to your account's key ({e:#}). Run `atuin login` to try again"
        );
        send(tx, Step::Sync, Status::Failed(failed));
    }
    stored.is_ok()
}

/// Only sign-in asks for a key.
#[cfg(not(feature = "sync"))]
async fn use_account_key(_: &Settings, _: &SqliteStore, _: &paseto_v4::Key, _: &Updates) -> bool {
    false
}

/// Forget the login, as `atuin logout` does.
pub(super) async fn sign_out() {
    if let Ok(settings) = Settings::new()
        && let Ok(meta) = settings.meta_store().await
    {
        let _ = meta.delete_session().await;
        let _ = meta.delete_hub_session().await;
    }
}

/// Record `choice` for later setups: they propose sync off while it's disabled.
#[cfg(feature = "sync")]
async fn save_sync_choice(settings: &Settings, choice: SyncChoice) {
    if let Ok(meta) = settings.meta_store().await {
        let _ = meta.save_sync_choice(choice).await;
    }
}

async fn sync_choice(settings: &Settings) -> SyncChoice {
    match settings.meta_store().await {
        Ok(meta) => meta.sync_choice().await.unwrap_or_default(),
        Err(_) => SyncChoice::default(),
    }
}

/// Start the daemon, if it isn't running, to sync, and tell it of the sign-in as `atuin login`
/// does. Without the `daemon`, the shell syncs on its next command.
#[cfg(feature = "daemon")]
async fn sync_soon(settings: &Settings, daemon: bool, tx: &Updates) {
    use atuin_daemon::service::ActiveService;

    if !daemon {
        send(tx, Step::Sync, Status::Done("Signed in, syncing in the background".into()));
        return;
    }
    let status = match ActiveService::find_or_spawn(settings).await {
        Ok(_) => {
            #[cfg(feature = "sync")]
            super::account::login::tell_daemon_logged_in(settings).await;
            Status::Done("Signed in, syncing in the background".into())
        }
        Err(e) => Status::Failed(format!(
            "Signed in, but the daemon didn't start ({e:#}). Run `atuin sync` to sync now"
        )),
    };
    send(tx, Step::Sync, status);
}

/// The shell syncs on its next command.
#[cfg(not(feature = "daemon"))]
async fn sync_soon(_: &Settings, _: bool, tx: &Updates) {
    send(tx, Step::Sync, Status::Done("Signed in, syncing in the background".into()));
}

/// Write what `plan` sets in the config, as far as `scope` covers. Returns the keys it changed.
async fn write_config(plan: &Plan, found: &Found, scope: Scope<'_>) -> Result<Vec<&'static str>> {
    let changes = config_changes(plan, found, scope, &Settings::new()?);
    let keys: Vec<_> = changes.iter().map(|&(key, _)| key).collect();
    if !keys.is_empty() {
        update_config_file(changes).await?;
    }
    Ok(keys)
}

/// What `plan` changes in the config `settings` were read from, as far as `scope` covers.
fn config_changes(
    plan: &Plan,
    found: &Found,
    scope: Scope<'_>,
    settings: &Settings,
) -> Vec<(&'static str, Value)> {
    let mut changes = Vec::new();

    let daemon_fuzzy = settings.requested_search_mode == RequestedSearchMode::DaemonFuzzy;
    // Off where the config doesn't say so, as an untouched one doesn't, it's written down, so a
    // later setup keeps it off.
    let unsaid = !plan.daemon.0 && !found.daemon_off && plan.ask::<Daemon>().is_some();
    let daemon = scope.covers::<Daemon>() && (plan.daemon.0 != settings.daemon.enabled || unsaid);
    if daemon {
        // A daemon on already may be managed externally (systemd, launchd), so only one setup
        // turns on is left to start itself.
        changes.push(("daemon.enabled", Value::from(plan.daemon.0)));
        changes.push(("daemon.autostart", Value::from(plan.daemon.0)));
    }
    let searches = daemon || (scope.covers::<ShellHistory>() && plan.shell.0);
    if plan.daemon.0 && searches && !daemon_fuzzy {
        changes.push(("search_mode", Value::from("daemon-fuzzy")));
    } else if daemon && !plan.daemon.0 && daemon_fuzzy {
        changes.push(("search_mode", Value::from("fuzzy")));
    }
    let keys = keys_after(plan, found, scope);

    let planned = planned_keys(plan, scope);
    let strips = found.strips_flags(scope);
    let in_settings = Keybindings::from_settings(settings);
    for key in KeySet::all() {
        let differs = keys.get(key) != in_settings.get(key);
        // Either the user decided the key, or an init flag removed by setup decided it. A key that
        // an environment variable chooses is written even if `settings`, which holds the variable's
        // value, agrees, to ensure that Atuin will bind the key as requested without the variable.
        if (planned.contains(key) || strips) && (differs || found.keys_set_by_env.contains(key)) {
            changes.push((key.setting(), Value::from(keys.get(key))));
        }
    }

    if scope.covers::<DesktopCapture>()
        && plan.desktop_capture.0 != settings.desktop_capture.enabled
    {
        changes.push((ENABLED_KEY, Value::from(plan.desktop_capture.0)));
    }
    // A hand-written list stays as it is when the choice stays on the listed applications.
    if sets_desktop_apps(plan, found, scope) {
        let all = settings.desktop_capture.apps == Apps::All;
        match plan.desktop_apps {
            DesktopApps::All if !all => changes.push((APPS_KEY, every_app_value())),
            DesktopApps::Listed if all => changes.push((APPS_KEY, listed_apps_value([]))),
            DesktopApps::All | DesktopApps::Listed => {}
        }
    }

    if scope.covers::<Threads>() && plan.threads.0 != settings.threads.enabled {
        changes.push(("threads.enabled", Value::from(plan.threads.0)));
    }
    if scope.covers::<OutputCapture>() && found.output_changes(plan.output.0) {
        // Off, the pty proxy is left running: only `pty_proxy.enabled` says to run it.
        if plan.output.0 {
            changes.push(("pty_proxy.enabled", Value::from(true)));
        }
        changes.push(("output.enabled", Value::from(plan.output.0)));
    }
    if scope.covers::<Sync>() {
        match &plan.sync {
            Sync::Server(address) => {
                changes.push(("sync_address", Value::from(address.as_str())));
            }
            Sync::Cloud if !settings.is_hub_sync() => {
                let cloud = atuin_client::settings::DEFAULT_SYNC_URL.as_str();
                changes.push(("sync_address", Value::from(cloud)));
            }
            Sync::Cloud | Sync::Off(_) => {}
        }
    }
    changes
}

/// Whether setup writes the applications that desktop capture reads, as far as `scope` reaches.
///
/// Setup writes them when capture ends up on and either `scope` names them or `scope` turns
/// capture on. Capture that turns on without the applications captures nothing.
fn sets_desktop_apps(plan: &Plan, found: &Found, scope: Scope<'_>) -> bool {
    let turns_on = scope.covers::<DesktopCapture>() && !found.desktop_capture;
    plan.desktop_capture.0 && (scope.covers::<DesktopApps>() || turns_on)
}

/// Put `atuin init` into the rc files of the shells `changes` adds, and take it out of those it
/// removes. `elsewhere` when shell history is to be off, but this shell loads Atuin from somewhere
/// setup can't tell. `notes` start the step's summary.
fn hook_shells(changes: &Changes<Shell>, elsewhere: bool, notes: ShellNotes, tx: &Updates) {
    let dirs = RcDirs::here();
    let ShellNotes {
        mut done,
        mut failed,
    } = notes;
    if !changes.add.is_empty() {
        match install_shell_init(&dirs, &changes.add) {
            Ok(hooked) if !hooked.is_empty() => {
                done.push(format!("hooked into {}", join_with_and(&hooked)));
            }
            Ok(_) => {
                let shells: Vec<_> = changes.add.iter().map(|shell| shell.name()).collect();
                let run = if shells.len() == 1 {
                    "runs"
                } else {
                    "run"
                };
                done.push(format!("{} {run} Atuin already", join_with_and(&shells)));
            }
            Err(e) => failed.push(format!("couldn't hook into your shell: {e}")),
        }
    }
    if !changes.remove.is_empty() {
        match remove_shell_init(&dirs, &changes.remove) {
            Ok(Unhooked { shells, left }) => {
                if !shells.is_empty() {
                    done.push(format!("unhooked {}", join_with_and(&shells)));
                }
                done.extend(left.into_iter().map(|(rc, line)| {
                    let rc = rc.display();
                    format!("left `{line}` in {rc}, which setup didn't write: take it out to stop")
                }));
            }
            Err(e) => failed.push(format!("couldn't unhook your shell: {e}")),
        }
    }
    if elsewhere {
        done.push(
            "this shell loads Atuin from a file setup didn't write: take it out there".into(),
        );
    }
    if let Some(status) = summary(done, failed) {
        send(tx, Step::Shells, status);
    }
}

/// Hook the agents into Atuin and connect them to its MCP server, as far as `scope` reaches.
#[cfg(feature = "ai")]
async fn connect(plan: &Plan, found: &Found, scope: Scope<'_>, settings: &Settings, tx: &Updates) {
    let AgentChanges { hooks, mcp } = agent_changes(plan, found, scope);
    if let Some(hooks) = hooks {
        hook_agents(&hooks, settings, tx).await;
    }
    if let Some(mcp) = mcp {
        connect_agents(&mcp, tx).await;
    }
}

/// What setup changes of the agents' hooks, and of their MCP server: `None` for a part it leaves
/// as it is.
#[cfg(feature = "ai")]
struct AgentChanges {
    hooks: Option<Changes<AnyHarness>>,
    mcp: Option<Changes<AnyHarness>>,
}

/// The [`AgentChanges`] `plan` makes, as far as `scope` reaches. Agents named alone get each part
/// as it's on; named beside a part's flag, as `--no-mcp --agents codex` names them, only that
/// part.
#[cfg(feature = "ai")]
fn agent_changes(plan: &Plan, found: &Found, scope: Scope<'_>) -> AgentChanges {
    let agents = &plan.agents.0;
    let same = |a: &AnyHarness, b: &AnyHarness| a.name() == b.name();
    let named = scope.flagged::<plan::Agents>().map(|agents| agents.0);
    let named = named.as_deref();
    let (hooks, mcp) = (scope.covers::<AgentHistory>(), scope.covers::<Mcp>());
    let alone = named.is_some() && !hooks && !mcp;
    let hooks = (hooks || alone).then(|| {
        let (on, had) = (plan.agent_history.0, found.hooked_agents());
        changes::<_, AgentHistory>(scope, on, agents, &had, named, same)
    });
    let mcp = (mcp || alone).then(|| {
        let (on, had) = (plan.mcp.0, found.mcp_agents());
        changes::<_, Mcp>(scope, on, agents, &had, named, same)
    });
    AgentChanges { hooks, mcp }
}

/// Install the hooks of the agents `changes` adds, and take out those of the agents it removes.
///
/// When desktop capture is on, Claude Code's hooks include the prompt hook, which adds recent
/// desktop activity to each prompt.
#[cfg(feature = "ai")]
async fn hook_agents(changes: &Changes<AnyHarness>, settings: &Settings, tx: &Updates) {
    if changes.add.is_empty() && changes.remove.is_empty() {
        return;
    }
    send(tx, Step::AgentHooks, Status::Running("Recording what your agents run".into()));
    let (mut hooked, mut unhooked, mut restart) = (Vec::new(), Vec::new(), Vec::new());
    let mut unload = Vec::new();
    let (mut failed, mut stuck) = (Vec::new(), Vec::new());
    for &agent in &changes.add {
        match agent.install_hooks().await {
            Ok(_) | Err(InstallHookError::AlreadyInstalled) => {
                if let Err(e) = add_prompt_hook(agent, settings).await {
                    failed.push(format!("{} ({e})", agent.display_name()));
                    continue;
                }
                hooked.push(agent.display_name());
                if loads_hooks_on_restart(agent) {
                    restart.push(agent.display_name());
                }
            }
            Err(InstallHookError::Unsupported) => {}
            Err(e) => failed.push(format!("{} ({e})", agent.display_name())),
        }
    }
    for &agent in &changes.remove {
        match agent.uninstall_hooks().await {
            Ok(true) => {
                unhooked.push(agent.display_name());
                if loads_hooks_on_restart(agent) {
                    unload.push(agent.display_name());
                }
            }
            Ok(false) => {}
            Err(e) => stuck.push(format!("{} ({e})", agent.display_name())),
        }
    }
    let mut done = Vec::new();
    if !hooked.is_empty() {
        done.push(format!("recording commands from {}", join_with_and(&hooked)));
    }
    if !restart.is_empty() {
        done.push(format!("restart {} to load the hooks", join_with_and(&restart)));
    }
    if !unhooked.is_empty() {
        done.push(format!("took the hooks out of {}", join_with_and(&unhooked)));
    }
    if !unload.is_empty() {
        done.push(format!("restart {} to stop recording there", join_with_and(&unload)));
    }
    let mut failures = Vec::new();
    if !failed.is_empty() {
        failures.push(format!("couldn't hook into {}", failed.join(", ")));
    }
    if !stuck.is_empty() {
        failures.push(format!("couldn't take the hooks out of {}", stuck.join(", ")));
    }
    if let Some(status) = summary(done, failures) {
        send(tx, Step::AgentHooks, status);
    }
}

/// Registers the prompt hook of desktop capture for `agent` when capture is on and `agent` is
/// Claude Code.
#[cfg(all(feature = "ai", feature = "desktop-capture"))]
async fn add_prompt_hook(agent: AnyHarness, settings: &Settings) -> Result<(), InstallHookError> {
    if settings.desktop_capture.enabled && matches!(agent, AnyHarness::ClaudeCode(_)) {
        super::desktop_capture::ensure_prompt_hook().await?;
    }
    Ok(())
}

#[cfg(all(feature = "ai", not(feature = "desktop-capture")))]
#[expect(clippy::unused_async, reason = "matches the desktop capture build")]
async fn add_prompt_hook(_: AnyHarness, _: &Settings) -> Result<(), InstallHookError> {
    Ok(())
}

/// Whether `agent` loads new hooks only once restarted, and keeps those it loaded until then:
/// those that load them as extensions, and Kiro, which reads them as a session starts. The rest
/// read them on their next run.
#[cfg(feature = "ai")]
const fn loads_hooks_on_restart(agent: AnyHarness) -> bool {
    matches!(
        agent,
        AnyHarness::Amp(_) | AnyHarness::Kiro(_) | AnyHarness::Opencode(_) | AnyHarness::Pi(_)
    )
}

/// Turn output capture `on` or off for the daemon, which reads it only as it starts.
async fn capture_output(on: bool, settings: &Settings, tx: &Updates) {
    let done = if on {
        "Capturing what your commands print, from your next terminal"
    } else {
        "Not capturing what your commands print"
    };
    // A daemon with autostart off may be managed externally (systemd, launchd), so it's left
    // to whoever manages it.
    let status = if !settings.daemon.enabled {
        Status::Done(done.into())
    } else if !settings.daemon.autostart {
        Status::Done(format!("{done}, once you restart the daemon"))
    } else {
        match restart_daemon(settings).await {
            Ok(()) => Status::Done(done.into()),
            Err(e) => Status::Failed(format!(
                "{done}, but the daemon didn't restart ({e:#}): run `atuin daemon restart`"
            )),
        }
    };
    send(tx, Step::Output, status);
}

/// Restart the daemon, if it's running, quietly: what it says would land in the middle of the
/// screen.
#[cfg(feature = "daemon")]
async fn restart_daemon(settings: &Settings) -> Result<()> {
    let _quiet = atuin_common::userio::log::SINK.push(atuin_common::userio::log::Nop);
    super::daemon::restart_cmd(settings, true).await
}

#[cfg(not(feature = "daemon"))]
#[expect(clippy::unused_async, reason = "matches the daemon build")]
async fn restart_daemon(_: &Settings) -> Result<()> {
    Ok(())
}

/// Whether `plan` stops the daemon, as far as `scope` reaches: one configured on, or, with
/// `--only --no-daemon`, one running though configured off. Setting all of it up, a daemon
/// started by hand is left to whoever started it.
fn stops_daemon(plan: &Plan, found: &Found, scope: Scope<'_>) -> bool {
    let asked = scope.flagged::<Daemon>() == Some(Daemon(false));
    scope.covers::<Daemon>() && !plan.daemon.0 && (found.daemon || (asked && found.daemon_running))
}

/// Whether a daemon is running here, as its pidfile says.
#[cfg(feature = "daemon")]
fn daemon_running(settings: &Settings) -> bool {
    atuin_daemon::service::pidfile::Pidfile::new(&settings.daemon.pidfile_path).is_live()
}

#[cfg(not(feature = "daemon"))]
const fn daemon_running(_: &Settings) -> bool {
    false
}

/// Whether `plan` turns on the daemon, thread capture, or desktop capture, as far as `scope`
/// reaches, so the daemon starts now. Left for the shell to start, the daemon wouldn't start
/// without shell history.
fn wakes_daemon(plan: &Plan, found: &Found, scope: Scope<'_>) -> bool {
    let daemon = scope.covers::<Daemon>() && !found.daemon;
    let threads = scope.covers::<Threads>() && plan.threads.0 && !found.threads;
    let desktop =
        scope.covers::<DesktopCapture>() && plan.desktop_capture.0 && !found.desktop_capture;
    plan.daemon.0 && (daemon || threads || desktop)
}

/// Start the daemon, quietly, unless something else manages it: one without `autostart`.
#[cfg(feature = "daemon")]
async fn start_daemon(settings: &Settings, tx: &Updates) {
    use atuin_daemon::service::ActiveService;

    if !settings.daemon.autostart {
        let left = "Turned the daemon on: start it the way you manage it";
        send(tx, Step::Daemon, Status::Done(left.into()));
        return;
    }
    let quiet = atuin_common::userio::log::SINK.push(atuin_common::userio::log::Nop);
    let started = ActiveService::ready(settings).await;
    drop(quiet);
    let status = match started {
        Ok(_) => Status::Done("Started the daemon".into()),
        // A refusal already says how to let this atuin run the daemon.
        Err(e) => Status::Failed(e.other_program().map_or_else(
            || format!("Couldn't start the daemon ({e:#}): run `atuin daemon start`"),
            |refusal| format!("Couldn't start the daemon: {refusal}"),
        )),
    };
    send(tx, Step::Daemon, status);
}

/// There's no daemon to start.
#[cfg(not(feature = "daemon"))]
#[expect(clippy::unused_async, reason = "matches the daemon build")]
async fn start_daemon(_: &Settings, _: &Updates) {}

/// Stop the daemon, and keep launchd from starting it again at the next login.
#[cfg(feature = "daemon")]
async fn stop_daemon(settings: &Settings, tx: &Updates) {
    use atuin_daemon::service::ActiveService;

    let quiet = atuin_common::userio::log::SINK.push(atuin_common::userio::log::Nop);
    let stopped = super::daemon::stop_cmd(settings).await;
    drop(quiet);
    let status = match stopped {
        Err(e) => {
            Status::Failed(format!("Couldn't stop the daemon ({e:#}): run `atuin daemon stop`"))
        }
        Ok(()) => match ActiveService::remove_launchd_agent().await {
            Ok(()) => Status::Done("Stopped the daemon".into()),
            Err(e) => Status::Failed(format!(
                "Stopped the daemon, but couldn't keep launchd from starting it again ({e:#})"
            )),
        },
    };
    send(tx, Step::Daemon, status);
}

/// There's no daemon to stop, only its config to turn off.
#[cfg(not(feature = "daemon"))]
#[expect(clippy::unused_async, reason = "matches the daemon build")]
async fn stop_daemon(_: &Settings, tx: &Updates) {
    send(tx, Step::Daemon, Status::Done("Turned the daemon off".into()));
}

/// Connect the agents `changes` adds to Atuin's MCP server, and disconnect those it removes.
#[cfg(feature = "ai")]
async fn connect_agents(changes: &Changes<AnyHarness>, tx: &Updates) {
    let now = SystemTime::now();
    let pool = BlockingPool::new(NonZeroUsize::MIN);
    let mut connected = Vec::new();
    let mut failed = Vec::new();
    let mut connect = Vec::new();
    for &agent in &changes.add {
        match McpAction::for_agent(agent, now, &pool).await {
            McpAction::Connect => connect.push(agent),
            McpAction::ReportStale { command } => failed.push(format!(
                "{} (`{command}` isn't on your PATH, so setup couldn't point its Atuin MCP server \
                 at this atuin: run setup again where `{command}` is on your PATH)",
                agent.display_name(),
            )),
            McpAction::ReportMissing { command, ago } => failed.push(format!(
                "{} (`{command}` isn't on your PATH, though it was used here {})",
                agent.display_name(),
                ago.display().days_ago(),
            )),
            McpAction::Skip => {}
        }
    }
    if connect.is_empty() && failed.is_empty() && changes.remove.is_empty() {
        return;
    }
    send(tx, Step::Mcp, Status::Running("Letting your agents search Atuin".into()));
    // What an install says would land in the middle of the screen; the step says it instead.
    let quiet = atuin_common::userio::log::SINK.push(atuin_common::userio::log::Nop);
    for agent in &connect {
        match agent.install_mcp().await {
            Ok(()) => connected.push(agent.display_name()),
            Err(e) => failed.push(format!("{} ({e})", agent.display_name())),
        }
    }
    let (mut disconnected, mut stuck) = (Vec::new(), Vec::new());
    for agent in &changes.remove {
        match agent.uninstall_mcp().await {
            Ok(true) => disconnected.push(agent.display_name()),
            Ok(false) => {}
            Err(e) => stuck.push(format!("{} ({e})", agent.display_name())),
        }
    }
    drop(quiet);
    let mut done = Vec::new();
    if !connected.is_empty() {
        done.push(format!(
            "{} can search Atuin from their next session",
            join_with_and(&connected)
        ));
    }
    if !disconnected.is_empty() {
        done.push(format!("disconnected {} from Atuin", join_with_and(&disconnected)));
    }
    let mut failures = Vec::new();
    if !failed.is_empty() {
        failures.push(format!("couldn't connect {}", failed.join(", ")));
    }
    if !stuck.is_empty() {
        failures.push(format!("couldn't disconnect {}", stuck.join(", ")));
    }
    if let Some(status) = summary(done, failures) {
        send(tx, Step::Mcp, status);
    }
}

/// How recently an agent must have written a session here for setup to report that its command is
/// missing from `PATH`.
///
/// If the agent was used within this window, the user probably still uses it. In that case,
/// something is more likely wrong with `PATH`. For example, the shell may not have picked up a new
/// install yet. The user would want to know about that.
///
/// If the agent hasn't been used for longer, the user probably tried it and then uninstalled it.
/// Reporting it on every run would only nag.
#[cfg(feature = "ai")]
const STILL_USED: Duration = Duration::from_hours(14 * 24);

/// What setup does about connecting an agent to Atuin's MCP server.
#[cfg(feature = "ai")]
#[derive(Debug, PartialEq, Eq)]
enum McpAction {
    /// Setup connects the agent.
    Connect,
    /// Setup reports that the agent's command isn't on `PATH`. The agent was used here `ago`.
    ReportMissing {
        command: &'static str,
        ago: Duration,
    },
    /// Setup reports that the agent's command isn't on `PATH`, so setup can't update the MCP
    /// server that the agent already has. That server may run another atuin, or one that is gone.
    ReportStale {
        command: &'static str,
    },
    /// Setup leaves the agent alone and doesn't mention it. The agent's command isn't on `PATH`,
    /// and the agent hasn't been used here recently.
    Skip,
}

/// Whether an agent's configuration already holds Atuin's MCP server.
#[cfg(feature = "ai")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpEntry {
    Present,
    Absent,
}

#[cfg(feature = "ai")]
impl McpAction {
    async fn for_agent(agent: AnyHarness, now: SystemTime, pool: &BlockingPool) -> Self {
        let Some(command) = agent.mcp_program() else {
            return Self::Connect;
        };
        if is_executable_with_path_search(command) {
            return Self::Connect;
        }
        let entry = match agent.has_mcp().await {
            Ok(true) => McpEntry::Present,
            Ok(false) | Err(_) => McpEntry::Absent,
        };
        let sessions = agent.sessions(pool);
        let last_active = tokio::task::spawn_blocking(move || sessions?.last_active());
        let last_active = last_active.await.ok().flatten();
        // A session with a timestamp in the future counts as brand new.
        Self::without_command(
            command,
            entry,
            last_active.map(|at| now.duration_since(at).unwrap_or_default()),
        )
    }

    /// The action for an agent whose `command` isn't on `PATH`, and whose configuration holds
    /// Atuin's MCP server as `entry` says. The agent was last used here `ago`.
    ///
    /// An entry that setup can't update is reported however long ago the agent was used, because
    /// the entry keeps the path of whichever atuin set it up.
    fn without_command(command: &'static str, entry: McpEntry, ago: Option<Duration>) -> Self {
        if entry == McpEntry::Present {
            return Self::ReportStale { command };
        }
        match ago {
            Some(ago) if ago < STILL_USED => Self::ReportMissing { command, ago },
            _ => Self::Skip,
        }
    }
}

/// The agents or shells to put a part of Atuin in, and to take it out of.
#[derive(Debug, PartialEq, Eq)]
struct Changes<T> {
    add: Vec<T>,
    remove: Vec<T>,
}

/// The changes that give `wanted` the part of Atuin that `C` chooses while it's `on`, from the
/// agents or shells that `had` it. Setting up all of the plan, the rest lose it. Setting up only
/// what flags name, one loses it only when `C`'s flag turns it off, and is one of those `named`,
/// if the flags name any: a flag naming agents or shells adds them, and takes none out.
fn changes<T: Copy, C: Flag>(
    scope: Scope<'_>,
    on: bool,
    wanted: &[T],
    had: &[T],
    named: Option<&[T]>,
    same: impl Fn(&T, &T) -> bool,
) -> Changes<T> {
    let add = if on {
        wanted.to_vec()
    } else {
        Vec::new()
    };
    let is_in = |list: &[T], item: &T| list.iter().any(|other| same(other, item));
    let loses = |item: &&T| match scope {
        Scope::All => !is_in(&add, item),
        Scope::Flagged(_) => {
            !on && scope.covers::<C>() && named.is_none_or(|named| is_in(named, item))
        }
    };
    let remove = had.iter().filter(loses).copied().collect();
    Changes { add, remove }
}

/// A step's status from the clauses saying what it did and what it couldn't, `None` when it did
/// nothing. Failed when it couldn't do some of it.
fn summary(done: Vec<String>, failed: Vec<String>) -> Option<Status> {
    let is_failed = !failed.is_empty();
    let text = done.into_iter().chain(failed).collect::<Vec<_>>().join("; ");
    let mut chars = text.chars();
    let first = chars.next()?;
    let text = first.to_uppercase().chain(chars).collect();
    Some(if is_failed {
        Status::Failed(text)
    } else {
        Status::Done(text)
    })
}

/// Sign in to Atuin Cloud, or say how to sign in to a server of their own.
#[cfg(feature = "sync")]
async fn sync(
    plan: &Plan,
    found: &Found,
    settings: &Arc<Settings>,
    store: &SqliteStore,
    tx: &Updates,
    mut answers: mpsc::UnboundedReceiver<Answer>,
) -> SignedIn {
    use atuin_client::hub::{HubAuthSession, HubAuthStatus};

    // A login for somewhere else would keep syncing there, or send its token to the new server.
    let syncing = found.syncing();
    if syncing.is_some() && syncing.as_ref() != Some(&plan.sync) {
        sign_out().await;
    }
    match plan.sync {
        Sync::Off(OffBecause::Chosen) => save_sync_choice(settings, SyncChoice::Disabled).await,
        Sync::Off(OffBecause::NobodyToSignIn) => {}
        Sync::Cloud | Sync::Server(_) => save_sync_choice(settings, SyncChoice::Enabled).await,
    }
    match &plan.sync {
        Sync::Off(_) if syncing.is_some() => {
            let off = "Signed out: sync is off, and everything stays on this machine";
            send(tx, Step::Sync, Status::Done(off.into()));
            return SignedIn::No;
        }
        Sync::Off(_) => return SignedIn::No,
        Sync::Server(address) if syncing.as_ref() == Some(&plan.sync) => {
            send(tx, Step::Sync, Status::Done(format!("Syncing with {address}")));
            return SignedIn::No;
        }
        Sync::Server(address) => {
            let next = format!("Syncing with {address}: run `atuin login` to sign in");
            send(tx, Step::Sync, Status::Done(next));
            return SignedIn::No;
        }
        Sync::Cloud if syncing == Some(Sync::Cloud) => return SignedIn::Yes,
        Sync::Cloud => {}
    }

    let skipped = || Status::Done("Sync is off. Run `atuin login` to turn it on".into());
    send(tx, Step::Sync, Status::Running("Sign in to sync".into()));

    let session = match HubAuthSession::start(&settings.hub_endpoint()).await {
        Ok(session) => session,
        Err(e) => {
            let failed = format!("Couldn't reach Atuin Cloud ({e:#}). Run `atuin login` later");
            send(tx, Step::Sync, Status::Failed(failed));
            return SignedIn::No;
        }
    };
    let opened = session.open_in_browser();
    let _ = tx.send(Update::Browser {
        url: session.auth_url.to_string(),
        code: session.short_code().to_string(),
        opened,
    });

    let deadline = tokio::time::Instant::now() + atuin_client::hub::DEFAULT_AUTH_TIMEOUT;
    let token = loop {
        tokio::select! {
            () = tokio::time::sleep(atuin_client::hub::DEFAULT_POLL_INTERVAL) => {
                match session.poll().await {
                    Ok(HubAuthStatus::Complete(token)) => break token,
                    Ok(HubAuthStatus::Failed(e)) => {
                        send(tx, Step::Sync, Status::Failed(format!("Sign-in failed: {e}")));
                        return SignedIn::No;
                    }
                    Ok(HubAuthStatus::Pending) | Err(_) => {}
                }
                if tokio::time::Instant::now() > deadline {
                    send(tx, Step::Sync, skipped());
                    return SignedIn::No;
                }
            }
            answer = answers.recv() => match answer {
                Some(Answer::OpenAgain) => { let _ = session.open_in_browser(); }
                Some(Answer::Skip) | None => {
                    save_sync_choice(settings, SyncChoice::Disabled).await;
                    send(tx, Step::Sync, skipped());
                    return SignedIn::No;
                }
                Some(Answer::Key(_)) => {}
            },
        }
    };

    send(tx, Step::Sync, Status::Running("Signing in".into()));
    if let Err(e) = atuin_client::hub::save_session(&token, settings).await {
        send(tx, Step::Sync, Status::Failed(format!("Couldn't save your sign-in: {e:#}")));
        return SignedIn::No;
    }

    let Ok(needs) = account_key(settings, store, tx, &mut answers).await else {
        // Signed in with a key that can't read the account's data is worse than not signed in:
        // sync would fail on every run.
        sign_out().await;
        let later = "Sync is off until you have your key: run `atuin key` on a machine that \
                     syncs, then `atuin login` here";
        send(tx, Step::Sync, Status::Done(later.into()));
        return SignedIn::No;
    };
    needs.map_or(SignedIn::Yes, SignedIn::NeedsKey)
}

#[cfg(not(feature = "sync"))]
async fn sync(
    _: &Plan,
    _: &Found,
    _: &Arc<Settings>,
    _: &SqliteStore,
    _: &Updates,
    _: mpsc::UnboundedReceiver<Answer>,
) -> SignedIn {
    SignedIn::No
}

/// The key the signed-in account needs, `None` when this machine's already reads its data. Asks
/// for it when it doesn't; `Err` when they gave up.
#[cfg(feature = "sync")]
async fn account_key(
    settings: &Arc<Settings>,
    store: &SqliteStore,
    tx: &Updates,
    answers: &mut mpsc::UnboundedReceiver<Answer>,
) -> Result<Option<paseto_v4::Key>, ()> {
    use atuin_client::record::sync::{ClientSource, SyncError, SyncSession};

    let Ok(key) = paseto_v4::Key::try_load_from_path(&settings.key_path) else {
        return Err(());
    };
    let session = SyncSession::builder()
        .store(store.clone())
        .client_source(ClientSource::FromSettings {
            settings,
            caps: None,
        })
        .build()
        .connect()
        .await;
    // Can't check it now (a network blip): sync says so on its first run if it's wrong.
    let Ok(session) = session else {
        return Ok(None);
    };
    match session.keyed(&key).key_valid().await {
        Some(SyncError::WrongKey) => {}
        _ => return Ok(None),
    }

    let mut error = None;
    loop {
        let _ = tx.send(Update::NeedKey(error.take()));
        let Some(Answer::Key(entered)) = answers.recv().await else {
            return Err(());
        };
        let Ok(candidate) = paseto_v4::Key::try_from_mnemonic(entered.trim()) else {
            error = Some("That isn't a key. Paste the words `atuin key` shows".into());
            continue;
        };
        send(tx, Step::Sync, Status::Running("Checking your key".into()));
        match session.keyed(&candidate).key_valid().await {
            Some(SyncError::WrongKey) => {
                error = Some("That key can't read this account's data".into());
            }
            _ => return Ok(Some(candidate)),
        }
    }
}

/// What the account has from machines other than this one: everything, when the server can't
/// say.
#[cfg(feature = "sync")]
async fn account(settings: &Arc<Settings>, store: &SqliteStore) -> Account {
    use atuin_client::record::sync::{ClientSource, SyncSession};
    use atuin_domain::record::RecordTag;

    let status = async {
        let session = SyncSession::builder()
            .store(store.clone())
            .client_source(ClientSource::FromSettings {
                settings,
                caps: None,
            })
            .build()
            .connect()
            .await
            .ok()?;
        Some((settings.host_id().await.ok()?, session.record_status().await.ok()?))
    };
    let Some((here, status)) = status.await else {
        return Account::Has {
            history: true,
            threads: true,
        };
    };
    let has = |tag: RecordTag| {
        status.hosts.iter().any(|(host, tags)| *host != here && tags.contains_key(&tag))
    };
    Account::Has {
        history: has(RecordTag::History),
        threads: has(RecordTag::AiSession),
    }
}

#[cfg(not(feature = "sync"))]
async fn account(_: &Arc<Settings>, _: &SqliteStore) -> Account {
    Account::None
}

/// Run atuin with `args` in its own process, keeping what it prints off the screen.
async fn quietly(args: &[&str]) -> Result<()> {
    let output = tokio::process::Command::new(std::env::current_exe()?)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await?;
    if output.status.success() {
        return Ok(());
    }
    // eyre's report: `Error: <what went wrong>`, then where.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason =
        stderr.lines().find_map(|line| line.strip_prefix("Error: ")).unwrap_or("it failed");
    eyre::bail!("{}", reason.trim())
}

/// The commands in this shell's history file, for the shells `atuin import auto` knows by name.
async fn shell_history() -> Option<(&'static str, usize)> {
    use atuin_client::import::Importer;
    use atuin_client::import::bash::Bash;
    use atuin_client::import::fish::Fish;
    use atuin_client::import::powershell::PowerShell;
    use atuin_client::import::zsh::Zsh;

    async fn entries<I: Importer>() -> usize {
        match I::new().await {
            Ok(mut importer) => importer.entries().await.unwrap_or(0),
            Err(_) => 0,
        }
    }

    // As `atuin import auto` tells: Windows has no `$SHELL`, but PowerShell sets `PSModulePath`.
    if cfg!(windows) {
        var_nonempty("PSModulePath")?;
        return Some(("powershell", entries::<PowerShell>().await));
    }
    let shell = var_nonempty("SHELL")?;
    let shell = shell.to_string_lossy();
    if shell.ends_with("/pwsh") {
        Some(("powershell", entries::<PowerShell>().await))
    } else if shell.ends_with("/zsh") {
        Some(("zsh", entries::<Zsh>().await))
    } else if shell.ends_with("/bash") {
        Some(("bash", entries::<Bash>().await))
    } else if shell.ends_with("/fish") {
        Some(("fish", entries::<Fish>().await))
    } else {
        None
    }
}

/// The agents on this machine, with the threads each has here: those that have kept any, and
/// those on `PATH`.
#[cfg(feature = "ai")]
async fn detect_agents() -> Vec<(AnyHarness, u64)> {
    use futures_util::StreamExt;

    let pool = BlockingPool::new(NonZeroUsize::new(4).expect("4 isn't 0"));
    let mut agents = Vec::new();
    for &agent in AnyHarness::all() {
        let threads = match agent.sessions(&pool).map(|sessions| sessions.existing()) {
            Some(Ok(existing)) => {
                Some(existing.filter(|s| std::future::ready(s.is_ok())).count().await)
            }
            _ => None,
        };
        let installed = is_executable_with_path_search(agent.program());
        if threads.is_some() || installed {
            agents.push((agent, u64::conv(threads.unwrap_or(0))));
        }
    }
    agents
}

#[cfg(not(feature = "ai"))]
async fn detect_agents() -> Vec<(AnyHarness, u64)> {
    Vec::new()
}

#[cfg(feature = "sync")]
async fn signed_in(settings: &Settings) -> bool {
    use atuin_client::settings::SyncAuth;

    !matches!(settings.resolve_sync_auth().await, SyncAuth::NotLoggedIn { .. })
}

#[cfg(not(feature = "sync"))]
async fn signed_in(_: &Settings) -> bool {
    false
}

/// The threads Atuin has on this machine, read beside a daemon that may be writing them.
#[cfg(feature = "ai")]
async fn threads_here(settings: &Settings) -> usize {
    use atuin_client::ai_session::{AiSessionDatabase, SessionFilter, sidecar_path};

    match AiSessionDatabase::open_read_only(sidecar_path(settings)).await {
        Ok(db) => db.list_sessions(&SessionFilter::default()).await.map_or(0, |s| s.len()),
        Err(_) => 0,
    }
}

#[cfg(not(feature = "ai"))]
async fn threads_here(_: &Settings) -> usize {
    0
}

/// The threads setup just imported, newest first, for the summary.
#[cfg(all(feature = "ai", feature = "daemon"))]
async fn latest_threads(settings: &Settings) -> Vec<atuin_ai::commands::Recent> {
    atuin_ai::commands::recent(settings, 3).await.unwrap_or_default()
}

/// A key or command that the summary suggests trying, and what it does.
#[derive(Debug, PartialEq, Eq)]
struct Tip {
    keys: &'static str,
    what: &'static str,
}

/// What the summary suggests trying after setup applies `plan`.
fn tips(plan: &Plan, found: &Found, scope: Scope<'_>) -> impl Iterator<Item = Tip> {
    let tip = |shown: bool, keys, what| shown.then_some(Tip { keys, what });
    [
        tip(plan.ctrl_r.0, "ctrl-r", "search your shell history"),
        tip(plan.up_arrow.0, "up arrow", "search it from what you've typed"),
        tip(bound_after(plan, found, scope).ai_resume, "ctrl-t", "resume an agent thread"),
        tip(plan.threads.0, "atuin resume", "pick a thread back up, in any agent"),
    ]
    .into_iter()
    .flatten()
}

/// The keys that `atuin init` will bind once setup applies `plan`.
fn bound_after(plan: &Plan, found: &Found, scope: Scope<'_>) -> Keybindings {
    keys_after(plan, found, scope).bound(KeyContext {
        threads: plan.threads.0,
        ai_resume_shell: plan.shells.0.iter().copied().any(Shell::binds_ai_resume),
    })
}

/// Whether `plan` changes any keys that `atuin init` binds.
fn keys_changed(plan: &Plan, found: &Found, scope: Scope<'_>) -> bool {
    let now = found.keys.bound(KeyContext {
        threads: found.threads,
        ai_resume_shell: found.inits.iter().copied().any(Shell::binds_ai_resume),
    });
    bound_after(plan, found, scope) != now
}

/// What setup did, and what to try next, left in the scrollback.
async fn print_summary(plan: &Plan, found: &Found, scope: Scope<'_>, steps: &[(Step, Status)]) {
    if matches!(scope, Scope::Flagged(_)) {
        print_steps(steps);
        return;
    }
    let stopped = steps.iter().any(|(_, status)| matches!(status, Status::Running(_)));
    println!();
    let how = if stopped {
        "setup stopped"
    } else {
        "is ready"
    };
    println!("  {} {}", "atuin".bold().green(), how.bold());
    println!();
    for (_, status) in steps {
        print_status(status);
    }
    if stopped {
        println!();
        println!("  Run {} again to finish.", "atuin setup".bold());
    }

    #[cfg(all(feature = "ai", feature = "daemon"))]
    if plan.threads.0
        && let Ok(settings) = Settings::new()
    {
        let latest = latest_threads(&settings).await;
        if !latest.is_empty() {
            println!();
            println!("  {}", "Your latest threads".bold());
            let age = latest.iter().map(|t| t.age.chars().count()).max().unwrap_or(0);
            let columns: usize =
                crossterm::terminal::size().map_or(100, |(columns, _)| columns.into());
            // The indent, the columns before the title and their gaps, and `…` on a long one.
            let title = columns.saturating_sub(4 + age + 2 + 11 + 2 + 1).max(20);
            for thread in latest {
                println!(
                    "    {:<age$}  {:<11}  {}",
                    thread.age.dimmed(),
                    thread.harness.harness().map_or("", |harness| harness.display_name()),
                    atuin_ai::commands::one_line(&thread.title, title),
                );
            }
        }
    }

    let mut tips = tips(plan, found, scope).peekable();
    // A shell picks up its key bindings as it starts.
    let shell_changed =
        !found.hooked || keys_changed(plan, found, scope) || plan.output.0 != found.output;
    let new_terminal = plan.shell.0 && shell_changed;
    let no_tips = tips.peek().is_none();
    if new_terminal || !no_tips {
        println!();
        let heading = match (new_terminal, no_tips) {
            (true, true) => "Open a new terminal",
            (true, false) => "Open a new terminal, then try",
            (false, _) => "Try",
        };
        println!("  {}", heading.bold());
        for Tip { keys, what } in tips {
            println!("    {:<14} {}", keys.bold(), what.dimmed());
        }
    }

    println!();
    println!("  {}", "Thanks for using Atuin, we hope you love it ♥".bold());
    for (what, link) in [
        ("Star us on GitHub", "https://github.com/atuinsh/atuin"),
        ("Say hi on Discord", "https://discord.gg/jR3tfchVvW"),
        ("Ask on the forum", "https://forum.atuin.sh"),
        ("Read the docs", "https://docs.atuin.sh"),
    ] {
        println!("    {what:<18} {}", link.dimmed());
    }
    println!();
}

/// What the steps that changed something said, as `--only` leaves them in the scrollback.
fn print_steps(steps: &[(Step, Status)]) {
    if steps.is_empty() {
        println!("  {}", "Nothing to change".dimmed());
    }
    for (_, status) in steps {
        print_status(status);
    }
}

/// Prints the status for one step.
fn print_status(status: &Status) {
    match status {
        Status::Done(text) => println!("{}", status_text("✓".green(), text, |line| line.normal())),
        Status::Failed(text) => {
            println!("{}", status_text("!".yellow().bold(), text, |line| line.normal()));
        }
        Status::Running(text) => {
            println!("{}", status_text("·".dimmed(), text, |line| line.dimmed()));
        }
    }
}

/// The text representing a step's status. Puts `marker` before the first line of `text`, and
/// indents every subsequent line of `text` to line up.
fn status_text(
    marker: impl std::fmt::Display,
    text: &str,
    style: impl Fn(&str) -> colored::ColoredString,
) -> impl std::fmt::Display {
    std::fmt::from_fn(move |f| {
        let mut lines = text.split('\n');
        write!(f, "  {marker} {}", style(lines.next().unwrap_or_default()))?;
        lines.try_for_each(|line| write!(f, "\n    {}", style(line)))
    })
}

/// Whether they set `daemon.enabled` themselves, in `ATUIN_DAEMON__ENABLED` or the config file,
/// rather than leaving it to its default. A config that can't be read leaves it to its default.
async fn daemon_setting_is_theirs() -> bool {
    if var_nonempty("ATUIN_DAEMON__ENABLED").is_some() {
        return true;
    }
    let Ok(path) = Settings::get_config_path() else {
        return false;
    };
    tokio::fs::read_to_string(path).await.is_ok_and(|text| sets_daemon_enabled(&text))
}

/// Whether the config `text` sets `daemon.enabled`, as a line of its own or inline.
fn sets_daemon_enabled(text: &str) -> bool {
    let doc = text.parse::<toml_edit::DocumentMut>();
    doc.is_ok_and(|doc| doc.get("daemon").and_then(|daemon| daemon.get("enabled")).is_some())
}

/// Each agent Atuin is in, and how. One whose config can't be read reads as without it.
#[cfg(feature = "ai")]
async fn connections() -> Vec<(AnyHarness, Connection)> {
    let mut connections = Vec::new();
    for &agent in AnyHarness::all() {
        let hooks = agent.has_hooks().await.unwrap_or_else(|err| {
            tracing::debug!(?err, agent = agent.name(), "can't tell whether its hooks run atuin");
            false
        });
        let mcp = agent.has_mcp().await.unwrap_or_else(|err| {
            tracing::debug!(?err, agent = agent.name(), "can't tell whether it has atuin's mcp");
            false
        });
        if hooks || mcp {
            connections.push((agent, Connection { hooks, mcp }));
        }
    }
    connections
}

#[cfg(not(feature = "ai"))]
async fn connections() -> Vec<(AnyHarness, Connection)> {
    Vec::new()
}

/// What the shells step says about the init flags that setup removed, to be reported with the rest
/// of that step.
#[derive(Default)]
struct ShellNotes {
    done: Vec<String>,
    failed: Vec<String>,
}

impl Stripped {
    /// What the shells step says about the flags that setup took off.
    fn to_notes(&self) -> ShellNotes {
        let mut notes = ShellNotes::default();
        if !self.changed.is_empty() {
            let files = join_with_and(self.changed.iter().map(|file| file.display()));
            let done = format!(
                "took the keybinding flags off `atuin init` in {files}, since your config chooses \
                 the keys"
            );
            notes.done.push(done);
        }
        notes.failed.extend(self.failed.iter().map(|unwritten| {
            let file = unwritten.path.display();
            format!(
                "couldn't take the keybinding flags off `atuin init` in {file}: {}",
                unwritten.error
            )
        }));
        notes
    }
}

/// Join `items` as a list separated by "and": "a, b and c".
fn join_with_and<I>(items: I) -> impl std::fmt::Display
where
    I: IntoIterator + Clone,
    I::Item: std::fmt::Display,
{
    std::fmt::from_fn(move |f| {
        let mut items = items.clone().into_iter().peekable();
        let mut first = true;
        while let Some(item) = items.next() {
            let gap = match (first, items.peek()) {
                (true, _) => "",
                (false, Some(_)) => ", ",
                (false, None) => " and ",
            };
            write!(f, "{gap}{item}")?;
            first = false;
        }
        Ok(())
    })
}

/// 48210 as "48,210".
fn count(n: impl std::fmt::Display) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;

    const DAY: Duration = Duration::from_hours(24);
    const JUST_INSIDE: Duration = STILL_USED.saturating_sub(Duration::from_secs(1));

    #[rstest]
    #[case::today(Some(Duration::ZERO), reported(Duration::ZERO))]
    #[case::last_week(Some(7 * DAY), reported(7 * DAY))]
    #[case::just_inside(Some(JUST_INSIDE), reported(JUST_INSIDE))]
    #[case::just_outside(Some(STILL_USED), McpAction::Skip)]
    #[case::months_ago(Some(90 * DAY), McpAction::Skip)]
    #[case::no_sessions(None, McpAction::Skip)]
    fn a_missing_agent_is_mentioned_only_while_still_used(
        #[case] ago: Option<Duration>,
        #[case] expected: McpAction,
    ) {
        assert_eq!(McpAction::without_command("grok", McpEntry::Absent, ago), expected);
    }

    /// An agent that already has Atuin's MCP server, but whose command isn't on `PATH`, keeps the
    /// server's old path, so setup says so however long ago the agent was used.
    #[rstest]
    fn an_mcp_server_that_setup_cannot_update_is_reported(
        #[values(Some(Duration::ZERO), Some(90 * DAY), None)] ago: Option<Duration>,
    ) {
        let action = McpAction::without_command("claude", McpEntry::Present, ago);
        assert_eq!(action, McpAction::ReportStale { command: "claude" });
    }

    const fn reported(ago: Duration) -> McpAction {
        McpAction::ReportMissing {
            command: "grok",
            ago,
        }
    }

    #[rstest]
    #[case(0, "0")]
    #[case(999, "999")]
    #[case(1000, "1,000")]
    #[case(48210, "48,210")]
    #[case(1_234_567, "1,234,567")]
    fn counts_have_thousands_separators(#[case] n: u64, #[case] expected: &str) {
        assert_eq!(count(n), expected);
    }

    #[rstest]
    #[case(&[], "")]
    #[case(&["pi"], "pi")]
    #[case(&["Claude Code", "Codex"], "Claude Code and Codex")]
    #[case(&["Claude Code", "Codex", "pi"], "Claude Code, Codex and pi")]
    fn names_read_as_a_list(#[case] list: &[&str], #[case] expected: &str) {
        assert_eq!(join_with_and(list).to_string(), expected);
    }

    #[rstest]
    #[case::one_line("Started the daemon", "  ! Started the daemon")]
    #[case::two_lines("socket in use\nplease set it", "  ! socket in use\n    please set it")]
    fn status_text_indents_each_later_line(#[case] text: &str, #[case] expected: &str) {
        assert_eq!(status_text("!", text, |line| line.normal()).to_string(), expected);
    }

    #[rstest]
    #[case::done(&["did it", "and that"], &[], Some(Status::Done("Did it; and that".into())))]
    #[case::partly(&["did it"], &["couldn't hook pi"], Some(Status::Failed("Did it; couldn't hook pi".into())))]
    #[case::failed(&[], &["couldn't hook pi"], Some(Status::Failed("Couldn't hook pi".into())))]
    #[case::nothing(&[], &[], None)]
    fn a_summary_says_what_was_done_and_what_failed(
        #[case] done: &[&str],
        #[case] failed: &[&str],
        #[case] expected: Option<Status>,
    ) {
        let strings = |clauses: &[&str]| clauses.iter().map(ToString::to_string).collect();
        assert_eq!(summary(strings(done), strings(failed)), expected);
    }

    /// Setting up the whole plan, what it leaves out comes out. Setting up only what flags name,
    /// shells or agents named are added, and only a flag turning the part off takes any out.
    #[rstest]
    #[case::all(&[], true, &[Shell::Zsh, Shell::Fish], &[Shell::Zsh, Shell::Bash], (&[Shell::Zsh, Shell::Fish][..], &[Shell::Bash][..]))]
    #[case::all_off(&[], false, &[Shell::Zsh], &[Shell::Zsh, Shell::Bash], (&[][..], &[Shell::Zsh, Shell::Bash][..]))]
    #[case::only_adds(&["--only", "--shells", "fish"], true, &[Shell::Fish], &[Shell::Zsh], (&[Shell::Fish][..], &[][..]))]
    // Turned off, the plan has no shells left to want.
    #[case::only_turned_off(&["--only", "--no-shell-history"], false, &[], &[Shell::Zsh, Shell::Bash], (&[][..], &[Shell::Zsh, Shell::Bash][..]))]
    // As `--no-agent-history --agents kiro` takes Kiro's hooks out, and no one else's.
    #[case::only_turned_off_where_named(&["--only", "--no-shell-history", "--shells", "bash"], false, &[], &[Shell::Zsh, Shell::Bash], (&[][..], &[Shell::Bash][..]))]
    #[case::only_off_unnamed(&["--only", "--shells", "zsh"], false, &[Shell::Zsh], &[Shell::Zsh, Shell::Bash], (&[][..], &[][..]))]
    fn changes_add_what_is_wanted_and_take_out_what_is_not(
        #[case] args: &[&str],
        #[case] on: bool,
        #[case] wanted: &[Shell],
        #[case] had: &[Shell],
        #[case] expected: (&[Shell], &[Shell]),
    ) {
        let cmd = setup(args);
        let named = cmd.scope().flagged::<plan::Shells>().map(|shells| shells.0);
        let named = named.as_deref();
        let changes =
            super::changes::<_, ShellHistory>(cmd.scope(), on, wanted, had, named, PartialEq::eq);
        assert_eq!((changes.add.as_slice(), changes.remove.as_slice()), expected);
    }

    /// What Atuin is in shows on the screen as it is, and stays so but for its MCP server, which
    /// comes with each agent connected.
    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    fn the_agents_start_from_what_atuin_is_in(mut found: Found) {
        let codex = AnyHarness::Codex(atuin_common::harnesstools::codex::Codex);
        let hooks_only = Connection {
            hooks: true,
            mcp: false,
        };
        found.connections = vec![(codex, hooks_only)];
        let asks = Asks {
            agents: true,
            hub: false,
            searchable: false,
            unhooks: false,
            removes_desktop_capture: false,
            away_from_screen: false,
        };
        let here = Plan::from_current_state(&found, asks);
        let names = |plan: &Plan| plan.agents.0.iter().map(Harness::name).collect::<Vec<_>>();
        assert_eq!(names(&here), ["codex"]);
        assert_eq!((here.agent_history, here.mcp), (AgentHistory(true), Mcp(false)));

        let proposed = here.proposal(&found, Confirm::Ask);
        assert_eq!(names(&proposed), ["codex"]);
        assert_eq!((proposed.agent_history, proposed.mcp), (AgentHistory(true), Mcp(true)));
    }

    /// The shells step names every rc file that lost its flags and every rc file that setup
    /// couldn't write. A failed rc file doesn't hide the rc files that changed.
    #[rstest]
    fn the_flag_removal_is_reported_through_the_summary() {
        let stripped = |changed: &[&str], failed: &[&str]| Stripped {
            changed: changed.iter().map(std::path::PathBuf::from).collect(),
            failed: failed
                .iter()
                .map(|path| shell_init::Unwritten {
                    path: std::path::PathBuf::from(path),
                    error: io::Error::other("denied"),
                })
                .collect(),
        };
        let status = |stripped: Stripped| {
            let ShellNotes { done, failed } = stripped.to_notes();
            summary(done, failed)
        };
        let off = "Took the keybinding flags off `atuin init` in .zshrc and .bashrc, since your \
                   config chooses the keys";
        assert_eq!(status(stripped(&[".zshrc", ".bashrc"], &[])), Some(Status::Done(off.into())));
        assert_eq!(status(stripped(&[], &[])), None);
        let partly = "Took the keybinding flags off `atuin init` in .zshrc, since your config \
                      chooses the keys; couldn't take the keybinding flags off `atuin init` in \
                      .bashrc: denied";
        let partly = Some(Status::Failed(partly.into()));
        assert_eq!(status(stripped(&[".zshrc"], &[".bashrc"])), partly);
    }

    /// The summary suggests ctrl-t only where `atuin init` will bind it, and asks for a new
    /// terminal when that changes.
    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    #[case::threads_on(Shell::Zsh, true, true, true)]
    #[case::threads_off(Shell::Zsh, true, false, false)]
    #[case::key_off(Shell::Zsh, false, true, false)]
    #[case::shell_without_the_widget(Shell::PowerShell, true, true, false)]
    fn the_thread_key_is_suggested_only_where_it_is_bound(
        mut found: Found,
        #[case] shell: Shell,
        #[case] ai_resume: bool,
        #[case] threads: bool,
        #[case] bound: bool,
    ) {
        found.inits = vec![shell];
        let plan = Plan {
            shell: ShellHistory(true),
            ai_resume: AiResume(ai_resume),
            threads: Threads(threads),
            ..Plan::from_current_state(&found, Asks::default())
        };
        let ctrl_t = Tip {
            keys: "ctrl-t",
            what: "resume an agent thread",
        };
        assert_eq!(tips(&plan, &found, Scope::All).any(|tip| tip == ctrl_t), bound);
        // `found` has threads off, so ctrl-t isn't bound now.
        assert_eq!(keys_changed(&plan, &found, Scope::All), bound);
    }

    /// What a run of setup starts from: the init lines, the config, and the run's `args`.
    struct Round {
        lines: &'static [InitLine],
        threads: bool,
        bind_ai_resume: bool,
        args: &'static [&'static str],
    }

    /// An `atuin init` line in the rc file of `shell`.
    struct InitLine {
        shell: Shell,
        flags: &'static str,
    }

    impl InitLine {
        /// The line with `flags` in place of its own.
        fn text(&self, flags: &str) -> String {
            format!("eval \"$(atuin init {}{flags})\"\n", self.shell.name())
        }
    }

    /// The keys that `atuin init` binds by its own rules, with the init lines and the config as
    /// they are.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Bound {
        now: Keybindings,
        /// The keys that it binds once threads are on.
        with_threads: Keybindings,
    }

    /// What a run of setup should leave bound.
    enum Expected {
        /// The keys bound before the run.
        Unchanged,
        Bound(Bound),
    }

    /// What a run of setup left, and whether it wrote a key or changed an rc file.
    struct After {
        bound: Bound,
        changed: bool,
    }

    async fn bound(dirs: &RcDirs, settings: &Settings) -> Bound {
        let flags = shell_inits(dirs).await.flags;
        Bound {
            now: flags.bound(settings, Shell::Zsh),
            with_threads: flags.chosen(settings),
        }
    }

    /// Run setup's key steps once over `dirs` and `settings`: detect, plan, write the keys into
    /// `settings`, and take the flags off the init lines.
    async fn run_keys(
        found: &mut Found,
        settings: &mut Settings,
        dirs: &RcDirs,
        args: &[&str],
    ) -> After {
        let inits = shell_inits(dirs).await;
        found.inits = inits.shells;
        found.keys = inits.flags.chosen(settings);
        found.threads = settings.threads.enabled;
        let cmd = setup(args);
        let plan = cmd.plan(found, Confirm::Yes).unwrap().effective();
        let mut changed = false;
        for (key, value) in config_changes(&plan, found, cmd.scope(), settings) {
            let value = value.as_bool();
            match key {
                "bind_ctrl_r" => settings.bind_ctrl_r = value.unwrap(),
                "bind_up_arrow" => settings.bind_up_arrow = value.unwrap(),
                "bind_ai_resume" => settings.bind_ai_resume = value.unwrap(),
                "threads.enabled" => settings.threads.enabled = value.unwrap(),
                _ => continue,
            }
            changed = true;
        }
        if found.strips_flags(cmd.scope()) {
            let stripped = strip_init_flags(dirs);
            assert!(stripped.failed.is_empty(), "{:?}", stripped.failed);
            changed |= !stripped.changed.is_empty();
        }

        // What setup shows agrees with what `atuin init` binds.
        let bound = bound(dirs, settings).await;
        assert_eq!(bound.now.ctrl_r, plan.ctrl_r.0);
        assert_eq!(bound.now.up_arrow, plan.up_arrow.0);
        assert_eq!(bound.now.ai_resume, bound_after(&plan, found, cmd.scope()).ai_resume);
        After { bound, changed }
    }

    /// A run of setup that covers the keys or the shells takes every flag off the init lines. Any
    /// run keeps each key that it doesn't change and leaves nothing for the next run to change.
    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    #[case::no_flags(
        Round { lines: &[InitLine { shell: Shell::Zsh, flags: "" }], threads: false, bind_ai_resume: true, args: &["--no-threads"] },
        Expected::Unchanged,
        false
    )]
    #[case::flag_moves(
        Round {
            lines: &[InitLine { shell: Shell::Zsh, flags: " --disable-up-arrow" }],
            threads: false,
            bind_ai_resume: true,
            args: &["--no-threads"],
        },
        Expected::Unchanged,
        true
    )]
    #[case::one_shell_flag_turns_the_key_off_for_both(
        Round {
            lines: &[
                InitLine { shell: Shell::Zsh, flags: " --disable-up-arrow" },
                InitLine { shell: Shell::Bash, flags: "" },
            ],
            threads: false,
            bind_ai_resume: true,
            args: &["--no-threads"],
        },
        Expected::Bound(Bound { now: Keybindings { ctrl_r: true, up_arrow: false, ai_resume: false }, with_threads: Keybindings { ctrl_r: true, up_arrow: false, ai_resume: true } }),
        true
    )]
    #[case::thread_flag_goes_without_threads(
        Round {
            lines: &[InitLine { shell: Shell::Zsh, flags: " --bind-ai-resume" }],
            threads: false,
            bind_ai_resume: true,
            args: &["--no-threads"],
        },
        Expected::Unchanged,
        true
    )]
    #[case::thread_flag_outlasts_threads_off(
        Round {
            lines: &[InitLine { shell: Shell::Zsh, flags: " --bind-ai-resume" }],
            threads: false,
            bind_ai_resume: false,
            args: &["--no-threads"],
        },
        Expected::Unchanged,
        true
    )]
    #[case::thread_flag_moves_with_threads(
        Round {
            lines: &[InitLine { shell: Shell::Zsh, flags: " --bind-ai-resume" }],
            threads: true,
            bind_ai_resume: false,
            args: &["--threads"],
        },
        Expected::Unchanged,
        true
    )]
    #[case::a_scoped_run_moves_every_flag(
        Round {
            lines: &[InitLine { shell: Shell::Zsh, flags: " --disable-ctrl-r --disable-up-arrow" }],
            threads: false,
            bind_ai_resume: true,
            args: &["--only", "--no-up-arrow"],
        },
        Expected::Unchanged,
        true
    )]
    #[case::a_run_without_keys_leaves_the_flags(
        Round {
            lines: &[InitLine { shell: Shell::Zsh, flags: " --disable-ctrl-r --disable-ai" }],
            threads: true,
            bind_ai_resume: true,
            args: &["--only", "--threads", "--daemon"],
        },
        Expected::Unchanged,
        false
    )]
    #[case::a_run_turns_a_flagged_key_on(
        Round {
            lines: &[InitLine { shell: Shell::Zsh, flags: " --disable-up-arrow" }],
            threads: false,
            bind_ai_resume: true,
            args: &["--only", "--up-arrow"],
        },
        Expected::Bound(Bound { now: Keybindings { ctrl_r: true, up_arrow: true, ai_resume: false }, with_threads: Keybindings { ctrl_r: true, up_arrow: true, ai_resume: true } }),
        true
    )]
    #[tokio::test]
    async fn a_setup_run_leaves_nothing_for_the_next(
        mut found: Found,
        #[case] round: Round,
        #[case] expected: Expected,
        #[case] changed: bool,
    ) {
        let home = tempfile::TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let rc = |line: &InitLine| dirs.rc_files(line.shell).remove(0);
        for line in round.lines {
            std::fs::write(rc(line), line.text(line.flags)).unwrap();
        }
        let mut settings = Settings::utc();
        settings.threads.enabled = round.threads;
        settings.bind_ai_resume = round.bind_ai_resume;
        let before = bound(&dirs, &settings).await;

        let first = run_keys(&mut found, &mut settings, &dirs, round.args).await;
        assert_eq!(first.changed, changed);
        let strips = found.strips_flags(setup(round.args).scope());
        for line in round.lines {
            let flags = if strips {
                ""
            } else {
                line.flags
            };
            assert_eq!(std::fs::read_to_string(rc(line)).unwrap(), line.text(flags));
        }
        let second = run_keys(&mut found, &mut settings, &dirs, round.args).await;
        assert!(!second.changed);
        assert_eq!(second.bound, first.bound);
        let expected = match expected {
            Expected::Unchanged => before,
            Expected::Bound(bound) => bound,
        };
        assert_eq!(first.bound, expected);
    }

    #[fixture]
    pub(super) fn found() -> Found {
        Found {
            history: Some(("zsh", 48210)),
            imported: 0,
            threads_here: 0,
            agents: vec![(AnyHarness::ClaudeCode(atuin_common::harnesstools::ccode::Ccode), 353)],
            hooked: false,
            inits: Vec::new(),
            shells: vec![Shell::Zsh, Shell::Bash],
            keys: Keybindings {
                ctrl_r: true,
                up_arrow: true,
                ai_resume: true,
            },
            flags_movable: true,
            keys_set_by_env: KeySet::empty(),
            threads: false,
            output: false,
            desktop_capture: false,
            desktop_apps: DesktopApps::All,
            saves_output: false,
            daemon: false,
            daemon_running: false,
            daemon_off: false,
            signed_in: false,
            sync_choice: SyncChoice::Enabled,
            server: None,
            upload_threads: ThreadUploads::All,
            connections: Vec::new(),
            hub: false,
        }
    }

    /// `atuin setup` with `args`.
    pub(super) fn setup(args: &[&str]) -> Cmd {
        Cmd::parse_from(std::iter::once("setup").chain(args.iter().copied()))
    }

    /// What `offer` offers, and what it says about the rest.
    fn offered(found: &Found, account: Account) -> (Offer, Vec<(Step, Status)>) {
        let plan = Plan {
            shell: ShellHistory(true),
            threads: Threads(true),
            ..Plan::from_current_state(found, Asks::default())
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let offer = offer(&plan, found, account, &tx);
        let mut steps = Vec::new();
        while let Ok(Update::Step(step, status)) = rx.try_recv() {
            steps.push((step, status));
        }
        (offer, steps)
    }

    fn done(step: Step, text: &str) -> (Step, Status) {
        (step, Status::Done(text.into()))
    }

    #[rstest]
    #[case::no_account(Account::None)]
    #[case::new_account(Account::Has { history: false, threads: false })]
    fn what_no_one_has_is_offered(found: Found, #[case] account: Account) {
        let (offer, steps) = offered(&found, account);
        assert_eq!(offer, Offer {
            history: Some(("zsh", 48210)),
            threads: THREADS.then_some(353),
        });
        assert_eq!(steps, []);
    }

    #[rstest]
    fn what_the_account_has_is_left_to_them(found: Found) {
        let account = Account::Has {
            history: true,
            threads: true,
        };
        let (offer, steps) = offered(&found, account);
        assert!(offer.is_empty());
        let mut expected = vec![done(
            Step::History,
            "Your account has history already: run `atuin import auto` to add this machine's too",
        )];
        if THREADS {
            expected.push(done(
                Step::Threads,
                "Your account has threads already: run `atuin threads import` to add this \
                 machine's too",
            ));
        }
        assert_eq!(steps, expected);
    }

    #[rstest]
    fn their_own_server_is_theirs_to_import_into(found: Found) {
        let (offer, steps) = offered(&found, Account::Unknown);
        assert!(offer.is_empty());
        assert_eq!(
            steps[0],
            done(
                Step::History,
                "Once you've signed in, run `atuin import auto` to add this machine's history"
            )
        );
    }

    #[rstest]
    fn what_atuin_has_is_not_offered_again(mut found: Found) {
        found.imported = 4812;
        found.threads_here = 25;
        let (offer, steps) = offered(&found, Account::None);
        assert!(offer.is_empty());
        let mut expected = vec![done(Step::History, "4,812 commands already in Atuin")];
        if THREADS {
            expected.push(done(Step::Threads, "25 threads already in Atuin"));
        }
        assert_eq!(steps, expected);
    }

    #[rstest]
    fn flags_turn_choices_off(found: Found) {
        let flags = ["--no-agent-history", "--no-up-arrow", "--no-mcp", "--sync", "off"];
        assert_eq!(setup(&flags).plan_at_screen(&found, Confirm::Ask).unwrap(), Plan {
            asks: Asks {
                agents: true,
                hub: false,
                searchable: false,
                unhooks: false,
                removes_desktop_capture: false,
                away_from_screen: false,
            },
            shell: ShellHistory(true),
            shells: plan::Shells(vec![Shell::Zsh, Shell::Bash]),
            output: OutputCapture(false),
            desktop_capture: DesktopCapture(false),
            desktop_apps: DesktopApps::All,
            threads: Threads(THREADS),
            agent_history: AgentHistory(false),
            agents: plan::Agents(found.agents()),
            ctrl_r: CtrlR(true),
            up_arrow: UpArrow(false),
            ai_resume: AiResume(true),
            mcp: Mcp(false),
            daemon: Daemon(true),
            sync: Sync::Off(OffBecause::Chosen),
            atuin_hub: AtuinHub(false),
            upload_threads: plan::UploadThreads(ThreadUploads::All),
            backfill: plan::Backfill(false),
        });
    }

    /// `install.sh` passes its own arguments to setup, so each flag that has shipped must keep
    /// parsing.
    #[rstest]
    fn every_flag_that_shipped_still_parses() {
        let shipped = [
            "--yes",
            "--no-shell-history",
            "--no-threads",
            "--no-agent-history",
            "--no-ctrl-r",
            "--no-up-arrow",
            "--no-mcp",
            "--sync",
            "off",
            "--no-import",
            "--hub",
        ];
        let parsed = Cmd::try_parse_from(std::iter::once("setup").chain(shipped));
        assert!(parsed.is_ok(), "{parsed:?}");
    }

    /// `install.sh` takes a setup whose full help lists `--no-up-arrow` as one that takes flags.
    #[rstest]
    fn full_help_lists_what_install_sh_looks_for() {
        let help = <Cmd as clap::CommandFactory>::command().render_long_help().to_string();
        assert!(help.contains("--no-up-arrow"), "{help}");
    }

    /// On a Mac, setup turns desktop capture on only for the user at the screen, and leaves
    /// capture that is on already to the user wherever the user is.
    #[cfg(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")))]
    #[rstest]
    #[case::away_turning_on(
        false,
        Login::Elsewhere,
        Err(
            "`--desktop-capture` needs you to run setup at this Mac's screen: log in there, then \
             run `atuin setup` in a terminal there"
        )
    )]
    #[case::at_screen_turning_on(false, Login::AtScreen, Ok(true))]
    #[case::away_keeping_on(true, Login::Elsewhere, Ok(true))]
    fn desktop_capture_turns_on_only_at_the_screen(
        #[case] on_already: bool,
        #[case] login: Login,
        #[case] expected: Result<bool, &str>,
        found: Found,
    ) {
        let found = Found {
            daemon: true,
            desktop_capture: on_already,
            ..found
        };

        let plan = setup(&["--only", "--desktop-capture"]).plan_for(&found, Confirm::Yes, login);

        let on = plan.map(|plan| plan.effective().desktop_capture.0);
        assert_eq!(on.map_err(|error| error.to_string()), expected.map_err(str::to_owned));
    }

    /// Away from the screen, the first screen shows capture as needing setup at the screen, and
    /// the proposal leaves capture off.
    #[cfg(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")))]
    #[rstest]
    fn away_from_the_screen_capture_stays_off(found: Found) {
        let found = Found {
            daemon: true,
            ..found
        };

        let plan = setup(&["--desktop-capture"]).plan_for(&found, Confirm::Ask, Login::Elsewhere);

        assert!(plan.is_err());
        let plan = setup(&[]).plan_for(&found, Confirm::Ask, Login::Elsewhere).unwrap();
        let needs = plan.ask::<DesktopCapture>().and_then(Result::err).map(|needs| needs.what);
        assert_eq!(needs, Some("you to run setup at this Mac's screen"));
        assert!(!plan.effective().desktop_capture.0);
    }

    #[rstest]
    #[case::ctrl_r(&["--no-shell-history", "--ctrl-r"], "`--ctrl-r` needs shell history: `--shell-history`")]
    #[case::up_arrow(&["--no-shell-history", "--up-arrow"], "`--up-arrow` needs shell history: `--shell-history`")]
    #[cfg_attr(
        feature = "ai",
        case::ai_resume(&["--ai-resume", "--no-threads"], "`--ai-resume` needs threads: `--threads`")
    )]
    #[cfg_attr(
        all(feature = "pty-proxy", feature = "daemon", unix),
        case::output(&["--no-daemon", "--output-capture"], "`--output-capture` needs the daemon: `--daemon`")
    )]
    #[cfg_attr(
        all(feature = "ai", feature = "daemon"),
        case::threads(&["--no-daemon", "--threads"], "`--threads` needs the daemon: `--daemon`")
    )]
    #[cfg_attr(
        all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")),
        case::desktop_capture(
            &["--no-daemon", "--desktop-capture"],
            "`--desktop-capture` needs the daemon: `--daemon`",
        )
    )]
    #[cfg_attr(
        all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")),
        case::desktop_apps(
            &["--desktop-apps", "all"],
            "`--desktop-apps all` needs desktop capture: `--desktop-capture`",
        )
    )]
    #[cfg_attr(
        feature = "octavo",
        case::hub(
            &["--hub", "--sync", "off"],
            "`--hub` needs sync: `--sync cloud`, or your own server's address",
        )
    )]
    #[cfg_attr(
        feature = "octavo",
        case::upload_threads(
            &["--no-hub", "--upload-threads", "workspaces"],
            "`--upload-threads workspaces` needs Atuin Hub: `--hub`",
        )
    )]
    #[case::shells(&["--no-shell-history", "--shells", "zsh"], "`--shells zsh` needs shell history: `--shell-history`")]
    #[cfg_attr(
        all(feature = "ai", feature = "daemon"),
        case::mcp(
            &["--no-shell-history", "--no-threads", "--no-agent-history", "--mcp"],
            "`--mcp` needs history or threads to search: `--shell-history`, `--agent-history` or `--threads`",
        )
    )]
    #[cfg_attr(feature = "octavo", case::backfill(&["--backfill"], "`--backfill` can't be on here: setup asks about Atuin Hub only with `--hub`"))]
    #[cfg_attr(
        feature = "octavo",
        case::backfill_of_workspaces(
            &["--hub", "--upload-threads", "workspaces", "--backfill"],
            "`--backfill` needs threads from outside workspaces: `--upload-threads all`",
        )
    )]
    fn flags_that_cannot_turn_on_say_what_they_need(
        found: Found,
        #[case] args: &[&str],
        #[case] expected: &str,
    ) {
        let error = setup(args).plan_at_screen(&found, Confirm::Ask).unwrap_err();
        assert_eq!(error.to_string(), expected);
    }

    /// Agents search what Atuin holds already, with nothing more captured.
    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    #[case::commands(48210, 0)]
    #[case::threads(0, 12)]
    fn mcp_searches_what_atuin_holds(
        mut found: Found,
        #[case] imported: i64,
        #[case] threads: usize,
    ) {
        (found.imported, found.threads_here) = (imported, threads);
        let args = ["--no-shell-history", "--no-threads", "--no-agent-history", "--mcp"];
        assert_eq!(setup(&args).plan_at_screen(&found, Confirm::Ask).unwrap().mcp, Mcp(true));
    }

    #[rstest]
    fn agent_flags_need_an_agent(mut found: Found) {
        found.agents.clear();
        let expected = if AGENTS {
            "`--mcp` can't be on here: there's no coding agent on this machine"
        } else {
            "`--mcp` can't be on here: this atuin was built without agent support"
        };
        let error = setup(&["--mcp"]).plan_at_screen(&found, Confirm::Ask).unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert!(setup(&["--no-mcp"]).plan_at_screen(&found, Confirm::Ask).is_ok());
    }

    /// `--only` changes what its flags name and nothing else, as the commands it replaced did.
    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    fn only_starts_from_what_is_here(mut found: Found) {
        found.threads = true;
        found.inits = vec![Shell::Zsh];
        let here = Plan::from_current_state(&found, Asks {
            agents: true,
            hub: false,
            searchable: false,
            unhooks: false,
            removes_desktop_capture: false,
            away_from_screen: false,
        });
        let plan = setup(&["--only", "--mcp", "--no-threads"])
            .plan_at_screen(&found, Confirm::Yes)
            .unwrap();
        assert_eq!(plan, Plan {
            mcp: Mcp(true),
            threads: Threads(false),
            ..here
        });
    }

    /// What `atuin setup <args>` writes, against a config that reads as `settings`.
    fn changes(found: &Found, settings: &Settings, args: &[&str]) -> Vec<(&'static str, String)> {
        let cmd = setup(args);
        let plan = cmd.plan_at_screen(found, Confirm::Yes).unwrap().effective();
        let changes = config_changes(&plan, found, cmd.scope(), settings);
        changes.into_iter().map(|(key, value)| (key, value.to_string().trim().to_owned())).collect()
    }

    /// Turns the daemon on, here and in its config.
    fn daemon_on(found: &mut Found, settings: &mut Settings) {
        found.daemon = true;
        settings.daemon.enabled = true;
    }

    /// Turns desktop capture on, here and in its config, capturing `apps`.
    #[cfg(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")))]
    fn desktop_on(found: &mut Found, settings: &mut Settings, apps: Apps) {
        found.desktop_capture = true;
        settings.desktop_capture.enabled = true;
        settings.desktop_capture.apps = apps;
        found.desktop_apps = DesktopApps::found(&settings.desktop_capture);
    }

    #[rstest]
    #[cfg_attr(feature = "daemon", case::daemon_on(&["--only", "--daemon"], |_: &mut Found, _: &mut Settings| {}, &[
        ("daemon.enabled", "true"),
        ("daemon.autostart", "true"),
        ("search_mode", "\"daemon-fuzzy\""),
    ]))]
    #[case::daemon_off(&["--only", "--no-daemon"], |found: &mut Found, settings: &mut Settings| {
        daemon_on(found, settings);
        settings.requested_search_mode = RequestedSearchMode::DaemonFuzzy;
    }, &[
        ("daemon.enabled", "false"),
        ("daemon.autostart", "false"),
        ("search_mode", "\"fuzzy\""),
    ])]
    #[cfg_attr(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")), case::desktop_capture_on(&["--only", "--desktop-capture"], daemon_on, &[
        ("desktop_capture.enabled", "true"),
        ("desktop_capture.apps", "\"all\""),
    ]))]
    #[cfg_attr(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")), case::desktop_capture_on_in_a_full_setup(
        &["--desktop-capture", "--no-threads"],
        daemon_on,
        &[
            ("search_mode", "\"daemon-fuzzy\""),
            ("desktop_capture.enabled", "true"),
            ("desktop_capture.apps", "\"all\""),
        ],
    ))]
    #[cfg_attr(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")), case::desktop_apps_all_as_capture_turns_on(
        &["--only", "--desktop-capture", "--desktop-apps", "all"],
        daemon_on,
        &[("desktop_capture.enabled", "true"), ("desktop_capture.apps", "\"all\"")],
    ))]
    #[cfg_attr(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")), case::desktop_apps_keep_a_written_list(
        &["--only", "--desktop-capture", "--desktop-apps", "listed"],
        |found: &mut Found, settings: &mut Settings| {
            daemon_on(found, settings);
            desktop_on(found, settings, Apps::Listed(vec!["firefox".to_owned()]));
        },
        &[],
    ))]
    #[cfg_attr(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")), case::desktop_apps_listed_instead_of_all(
        &["--only", "--desktop-apps", "listed"],
        |found: &mut Found, settings: &mut Settings| {
            daemon_on(found, settings);
            desktop_on(found, settings, Apps::All);
        },
        &[("desktop_capture.apps", "[]")],
    ))]
    #[cfg_attr(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")), case::desktop_apps_left_while_capture_is_off(
        &["--only", "--no-desktop-capture", "--desktop-apps", "listed"],
        |found: &mut Found, settings: &mut Settings| {
            daemon_on(found, settings);
            desktop_on(found, settings, Apps::All);
        },
        &[("desktop_capture.enabled", "false")],
    ))]
    #[case::daemon_off_keeps_another_search(&["--only", "--no-daemon"], |found: &mut Found, settings: &mut Settings| {
        daemon_on(found, settings);
        settings.requested_search_mode = RequestedSearchMode::Prefix;
    }, &[("daemon.enabled", "false"), ("daemon.autostart", "false")])]
    #[case::flag_kept_off_is_written(&["--only", "--no-up-arrow"], |found: &mut Found, settings: &mut Settings| {
        found.inits = vec![Shell::Zsh];
        found.keys.up_arrow = false;
        settings.bind_up_arrow = true;
    }, &[("bind_up_arrow", "false")])]
    // Turning shell history off takes the flags off, so that the init lines can come out.
    #[case::flag_moves(&["--only", "--no-shell-history"], |found: &mut Found, settings: &mut Settings| {
        found.inits = vec![Shell::Zsh];
        found.keys.up_arrow = false;
        settings.bind_up_arrow = true;
    }, &[("bind_up_arrow", "false")])]
    #[case::flag_out_of_scope_stays(&["--only", "--no-output-capture"], |found: &mut Found, settings: &mut Settings| {
        found.inits = vec![Shell::Zsh];
        found.keys.up_arrow = false;
        settings.bind_up_arrow = true;
    }, &[])]
    // Another shell's line may bind the key, so the key that the user kept off is written.
    #[case::key_kept_off_over_a_flag_that_stays(&["--only", "--no-up-arrow"], |found: &mut Found, settings: &mut Settings| {
        found.inits = vec![Shell::Zsh, Shell::Bash];
        found.keys.up_arrow = false;
        found.flags_movable = false;
        settings.bind_up_arrow = true;
    }, &[("bind_up_arrow", "false")])]
    #[case::flag_that_stays_is_left_out(&["--only", "--no-output-capture"], |found: &mut Found, settings: &mut Settings| {
        found.inits = vec![Shell::Zsh];
        found.keys.up_arrow = false;
        found.flags_movable = false;
        settings.bind_up_arrow = true;
    }, &[])]
    #[case::key_changed_over_a_flag_that_stays(&["--only", "--up-arrow"], |found: &mut Found, settings: &mut Settings| {
        found.inits = vec![Shell::Zsh];
        found.keys.up_arrow = false;
        found.flags_movable = false;
        settings.bind_up_arrow = false;
    }, &[("bind_up_arrow", "true")])]
    // The config file may say otherwise, so the variable's value is written.
    #[case::key_from_a_variable(&["--only", "--up-arrow"], |found: &mut Found, _: &mut Settings| {
        found.inits = vec![Shell::Zsh];
        found.flags_movable = false;
        found.keys_set_by_env = Key::UpArrow.into();
    }, &[("bind_up_arrow", "true")])]
    #[case::key_from_a_variable_out_of_scope(&["--only", "--no-output-capture"], |found: &mut Found, _: &mut Settings| {
        found.inits = vec![Shell::Zsh];
        found.flags_movable = false;
        found.keys_set_by_env = Key::UpArrow.into();
    }, &[])]
    #[case::key(&["--only", "--no-up-arrow"], |found: &mut Found, _: &mut Settings| found.inits = vec![Shell::Zsh], &[
        ("bind_up_arrow", "false"),
    ])]
    #[cfg_attr(feature = "ai", case::ai_resume_on(&["--only", "--ai-resume", "--threads"], |found: &mut Found, settings: &mut Settings| {
        daemon_on(found, settings);
        found.inits = vec![Shell::Zsh];
        found.threads = true;
        settings.threads.enabled = true;
    }, &[]))]
    #[cfg_attr(feature = "ai", case::ai_resume_off(&["--only", "--no-ai-resume", "--threads"], |found: &mut Found, settings: &mut Settings| {
        daemon_on(found, settings);
        found.inits = vec![Shell::Zsh];
        found.threads = true;
        settings.threads.enabled = true;
    }, &[("bind_ai_resume", "false")]))]
    #[cfg_attr(feature = "ai", case::ai_resume_left_without_threads(&["--only", "--no-threads", "--no-ai-resume"], |found: &mut Found, _: &mut Settings| found.inits = vec![Shell::Zsh], &[]))]
    #[case::keys_left_without_shell_history(&["--no-shell-history", "--no-up-arrow", "--no-threads"], |_: &mut Found, _: &mut Settings| {}, &[
        ("daemon.enabled", "true"),
        ("daemon.autostart", "true"),
        ("search_mode", "\"daemon-fuzzy\""),
    ])]
    #[cfg_attr(
        all(feature = "pty-proxy", feature = "daemon", unix),
        case::output_on(&["--only", "--output-capture"], |found: &mut Found, settings: &mut Settings| {
            daemon_on(found, settings);
            found.inits = vec![Shell::Zsh];
        }, &[("pty_proxy.enabled", "true"), ("output.enabled", "true")])
    )]
    #[case::output_off(&["--only", "--no-output-capture"], |found: &mut Found, _: &mut Settings| {
        (found.output, found.saves_output) = (true, true);
    }, &[("output.enabled", "false")])]
    // A proxy started by hand still sends its output to keep.
    #[case::output_off_without_the_proxy(&["--only", "--no-output-capture"], |found: &mut Found, _: &mut Settings| {
        found.saves_output = true;
    }, &[("output.enabled", "false")])]
    #[case::output_off_already(&["--only", "--no-output-capture"], |_: &mut Found, _: &mut Settings| {}, &[])]
    // Off by default only, so a later setup would propose it on.
    #[cfg_attr(feature = "daemon", case::daemon_off_unsaid(&["--only", "--no-daemon"], |_: &mut Found, _: &mut Settings| {}, &[
        ("daemon.enabled", "false"),
        ("daemon.autostart", "false"),
    ]))]
    #[cfg_attr(feature = "daemon", case::daemon_off_already(&["--only", "--no-daemon"], |found: &mut Found, _: &mut Settings| {
        found.daemon_off = true;
    }, &[]))]
    #[cfg_attr(
        all(feature = "ai", feature = "daemon"),
        case::mcp_is_not_config(
            &["--only", "--mcp"],
            |found: &mut Found, _: &mut Settings| found.inits = vec![Shell::Zsh],
            &[],
        )
    )]
    #[cfg_attr(
        feature = "octavo",
        // The user's own atuin.lua says which threads go up, not config.toml.
        case::uploads_stay_out_of_config_toml(
            &["--only", "--hub", "--upload-threads", "workspaces"],
            |found: &mut Found, settings: &mut Settings| {
                daemon_on(found, settings);
                found.signed_in = true;
            },
            &[],
        )
    )]
    #[cfg_attr(
        feature = "octavo",
        case::uploads_as_they_are(
            &["--only", "--hub", "--upload-threads", "all"],
            |found: &mut Found, settings: &mut Settings| {
                daemon_on(found, settings);
                found.signed_in = true;
            },
            &[],
        )
    )]
    fn config_changes_as_far_as_the_flags_reach(
        mut found: Found,
        #[case] args: &[&str],
        #[case] given: fn(&mut Found, &mut Settings),
        #[case] expected: &[(&str, &str)],
    ) {
        let mut settings = Settings::utc();
        settings.daemon.enabled = false;
        settings.requested_search_mode = RequestedSearchMode::Fuzzy;
        settings.bind_up_arrow = true;
        given(&mut found, &mut settings);
        let changes = changes(&found, &settings, args);
        let changes: Vec<_> = changes.iter().map(|(key, value)| (*key, value.as_str())).collect();
        assert_eq!(changes, expected);
    }

    /// `--only --no-daemon` stops a daemon running, even one configured off; setting all of it up,
    /// only one configured on.
    #[rstest]
    #[case::configured_on(&["--only", "--no-daemon"], true, false, true)]
    #[case::started_by_hand(&["--only", "--no-daemon"], false, true, true)]
    #[case::not_running(&["--only", "--no-daemon"], false, false, false)]
    #[case::all_leaves_one_started_by_hand(&["--no-daemon"], false, true, false)]
    #[case::elsewhere(&["--only", "--no-up-arrow"], true, true, false)]
    fn the_daemon_stops_as_asked(
        mut found: Found,
        #[case] args: &[&str],
        #[case] daemon: bool,
        #[case] running: bool,
        #[case] expected: bool,
    ) {
        (found.daemon, found.daemon_running) = (daemon, running);
        let cmd = setup(args);
        let plan = cmd.plan_at_screen(&found, Confirm::Yes).unwrap().effective();
        assert_eq!(stops_daemon(&plan, &found, cmd.scope()), expected);
    }

    /// With `--only`, shells named beside `--no-shell-history` are those it comes out of. Named
    /// alone while shell history is off, they need it on.
    #[rstest]
    fn shells_named_narrow_unhooking(mut found: Found) {
        found.inits = vec![Shell::Zsh, Shell::Bash];
        let plan = setup(&["--only", "--no-shell-history", "--shells", "bash"])
            .plan_at_screen(&found, Confirm::Yes);
        assert_eq!(plan.unwrap().effective().shells, plan::Shells(vec![Shell::Bash]));

        found.inits.clear();
        let error =
            setup(&["--only", "--shells", "zsh"]).plan_at_screen(&found, Confirm::Yes).unwrap_err();
        assert_eq!(error.to_string(), "`--shells zsh` needs shell history: `--shell-history`");
    }

    /// The daemon starts as setup turns it or thread capture on, as nothing else may start it
    /// before the shell's next command.
    #[rstest]
    #[cfg_attr(feature = "daemon", case::daemon_on(&["--only", "--daemon"], false, false, true))]
    #[cfg_attr(feature = "daemon", case::daemon_on_already(&["--only", "--daemon"], true, false, false))]
    #[cfg_attr(all(feature = "ai", feature = "daemon"), case::threads_on(&["--only", "--threads", "--daemon"], false, false, true))]
    #[cfg_attr(all(feature = "ai", feature = "daemon"), case::threads_on_already(&["--only", "--threads"], true, true, false))]
    #[cfg_attr(all(feature = "ai", feature = "daemon"), case::threads_with_daemon_on(&["--only", "--threads"], true, false, true))]
    #[cfg_attr(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")), case::desktop_capture_on(&["--only", "--desktop-capture"], true, false, true))]
    #[case::daemon_off(&["--only", "--no-daemon"], true, false, false)]
    #[case::elsewhere(&["--only", "--no-up-arrow"], false, false, false)]
    fn the_daemon_wakes_for_what_needs_it(
        mut found: Found,
        #[case] args: &[&str],
        #[case] daemon: bool,
        #[case] threads: bool,
        #[case] expected: bool,
    ) {
        (found.daemon, found.threads) = (daemon, threads);
        let cmd = setup(args);
        let plan = cmd.plan_at_screen(&found, Confirm::Yes).unwrap().effective();
        assert_eq!(wakes_daemon(&plan, &found, cmd.scope()), expected);
    }

    /// `--desktop-apps` sets which applications capture reads, beside `--desktop-capture`.
    #[cfg(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")))]
    #[rstest]
    fn desktop_apps_flag_sets_the_applications(mut found: Found) {
        found.daemon = true;
        let args = ["--only", "--desktop-apps", "listed", "--desktop-capture"];
        let plan = setup(&args).plan_at_screen(&found, Confirm::Yes).unwrap();
        assert_eq!(plan.desktop_apps, DesktopApps::Listed);
    }

    /// The first screen proposes every application, unless capture is on with a list, or on
    /// with an empty list, which the user may be about to fill.
    #[rstest]
    #[case::all(false, Apps::All, DesktopApps::All)]
    #[case::listed(false, Apps::Listed(vec!["firefox".to_owned()]), DesktopApps::Listed)]
    #[case::empty_while_on(true, Apps::Listed(Vec::new()), DesktopApps::Listed)]
    #[case::empty_while_off(false, Apps::Listed(Vec::new()), DesktopApps::All)]
    fn found_desktop_apps(
        #[case] enabled: bool,
        #[case] apps: Apps,
        #[case] expected: DesktopApps,
    ) {
        let config = atuin_client::settings::DesktopCapture {
            enabled,
            apps,
            ..Default::default()
        };
        assert_eq!(DesktopApps::found(&config), expected);
    }

    /// Setup turns desktop capture on and off as the flags say, brings the login files up to
    /// date when capture stays on, and removes them for `--no-desktop-capture` even when
    /// capture is already off.
    #[cfg(all(feature = "desktop-capture", any(target_os = "linux", target_os = "macos")))]
    #[rstest]
    #[case::turn_on(&["--only", "--desktop-capture"], false, Some(DesktopChange::TurnOn))]
    #[case::keep_on(&["--only", "--desktop-capture"], true, Some(DesktopChange::KeepOn))]
    #[case::keep_on_in_a_full_setup(&[], true, Some(DesktopChange::KeepOn))]
    #[case::turn_off(&["--only", "--no-desktop-capture"], true, Some(DesktopChange::TurnOff))]
    #[case::left_on_by_daemon_flags(&["--only", "--no-daemon"], true, None)]
    #[case::keep_off(&["--only", "--no-desktop-capture"], false, Some(DesktopChange::KeepOff))]
    #[case::keep_off_in_a_full_setup(&["--no-desktop-capture"], false, Some(DesktopChange::KeepOff))]
    #[case::left_off_in_a_full_setup(&[], false, None)]
    #[case::elsewhere(&["--only", "--no-up-arrow"], true, None)]
    fn desktop_capture_changes(
        mut found: Found,
        #[case] args: &[&str],
        #[case] on: bool,
        #[case] expected: Option<DesktopChange>,
    ) {
        found.daemon = true;
        found.desktop_capture = on;
        let cmd = setup(args);
        let plan = cmd.plan_at_screen(&found, Confirm::Yes).unwrap().effective();
        assert_eq!(DesktopChange::of(&plan, &found, cmd.scope()), expected);
    }

    #[cfg(feature = "desktop-capture")]
    #[rstest]
    #[case::done_and_done(Status::Done("A".into()), Status::Done("B c".into()), Status::Done("A; b c".into()))]
    #[case::done_and_failed(Status::Done("A".into()), Status::Failed("B".into()), Status::Failed("A; b".into()))]
    #[case::failed_and_done(Status::Failed("A".into()), Status::Done("B".into()), Status::Failed("A; b".into()))]
    #[case::running_gives_way(Status::Running("A".into()), Status::Done("B".into()), Status::Done("B".into()))]
    fn statuses_join_their_phrases(
        #[case] earlier: Status,
        #[case] later: Status,
        #[case] expected: Status,
    ) {
        assert_eq!(earlier.and(later), expected);
    }

    /// With `--only`, agents named alone get each part as it's on, and named beside a part's
    /// flag, only that part.
    #[cfg(feature = "ai")]
    #[rstest]
    #[case::disconnect_only(&["--only", "--no-mcp", "--agents", "codex"], None, Some((&[][..], &["codex"][..])))]
    #[case::unhook_only(&["--only", "--no-agent-history", "--agents", "claude-code"], Some((&[][..], &["claude-code"][..])), None)]
    #[case::named_alone(&["--only", "--agents", "codex"], Some((&["codex"][..], &[][..])), Some((&["codex"][..], &[][..])))]
    #[case::elsewhere(&["--only", "--no-up-arrow"], None, None)]
    fn named_agents_change_the_parts_flagged_beside_them(
        mut found: Found,
        #[case] args: &[&str],
        #[case] hooks: Option<(&[&'static str], &[&'static str])>,
        #[case] mcp: Option<(&[&'static str], &[&'static str])>,
    ) {
        use atuin_common::harnesstools::ccode::Ccode;
        use atuin_common::harnesstools::codex::Codex;

        let with = |hooks, mcp| Connection { hooks, mcp };
        found.connections = vec![
            (AnyHarness::ClaudeCode(Ccode), with(true, false)),
            (AnyHarness::Codex(Codex), with(false, true)),
        ];
        let cmd = setup(args);
        let plan = cmd.plan_at_screen(&found, Confirm::Yes).unwrap().effective();
        let changes = agent_changes(&plan, &found, cmd.scope());
        let names = |agents: &[AnyHarness]| agents.iter().map(Harness::name).collect::<Vec<_>>();
        let names = |changes: Option<Changes<AnyHarness>>| {
            changes.map(|changes| (names(&changes.add), names(&changes.remove)))
        };
        let as_vecs =
            |(add, remove): (&[&'static str], &[&'static str])| (add.to_vec(), remove.to_vec());
        assert_eq!(names(changes.hooks), hooks.map(as_vecs));
        assert_eq!(names(changes.mcp), mcp.map(as_vecs));
    }

    #[rstest]
    #[case::set("[daemon]\nenabled = false\n", true)]
    #[case::inline("daemon = { enabled = false }\n", true)]
    #[case::commented_out("[daemon]\n# enabled = false\nautostart = true\n", false)]
    #[case::other_keys("[daemon]\nsync_frequency = 60\n", false)]
    #[case::not_toml("[daemon\n", false)]
    fn the_daemon_is_theirs_only_where_they_set_it(#[case] text: &str, #[case] expected: bool) {
        assert_eq!(sets_daemon_enabled(text), expected);
    }

    /// Threads, output capture and the daemon's search need the daemon, so it's on but where
    /// they turned it off.
    #[rstest]
    #[case::left_unset(false, true)]
    #[case::turned_off(true, false)]
    fn the_daemon_is_on_unless_turned_off(
        mut found: Found,
        #[case] daemon_off: bool,
        #[case] expected: bool,
    ) {
        found.daemon_off = daemon_off;
        assert_eq!(
            setup(&[]).plan_at_screen(&found, Confirm::Ask).unwrap().daemon,
            Daemon(expected)
        );
    }

    #[rstest]
    #[case::asked(Confirm::Ask, None, SyncChoice::Enabled, Sync::Cloud)]
    #[case::not_asked(
        Confirm::Yes,
        None,
        SyncChoice::Enabled,
        Sync::Off(OffBecause::NobodyToSignIn)
    )]
    #[case::their_server(Confirm::Ask, Some("https://atuin.example.com"), SyncChoice::Enabled, Sync::Server("https://atuin.example.com".into()))]
    #[case::turned_off(Confirm::Ask, None, SyncChoice::Disabled, Sync::Off(OffBecause::Chosen))]
    #[case::turned_off_with_their_server(
        Confirm::Ask,
        Some("https://atuin.example.com"),
        SyncChoice::Disabled,
        Sync::Off(OffBecause::Chosen)
    )]
    fn sync_without_the_flag(
        mut found: Found,
        #[case] confirm: Confirm,
        #[case] server: Option<&str>,
        #[case] sync_choice: SyncChoice,
        #[case] expected: Sync,
    ) {
        found.server = server.map(Into::into);
        found.sync_choice = sync_choice;
        // Without sync, there's nowhere to sync to.
        let expected = if cfg!(feature = "sync") {
            expected
        } else if confirm == Confirm::Yes {
            Sync::Off(OffBecause::NobodyToSignIn)
        } else {
            Sync::Off(OffBecause::Chosen)
        };
        assert_eq!(setup(&[]).plan_at_screen(&found, confirm).unwrap().sync, expected);
    }

    #[rstest]
    fn sync_off_from_the_flag_is_chosen(
        found: Found,
        #[values(Confirm::Ask, Confirm::Yes)] confirm: Confirm,
    ) {
        let sync = setup(&["--sync", "off"]).plan_at_screen(&found, confirm).unwrap().sync;
        assert_eq!(sync, Sync::Off(OffBecause::Chosen));
    }

    #[rstest]
    #[case::cloud(None, Sync::Cloud)]
    #[case::their_server(Some("https://atuin.example.com"), Sync::Server("https://atuin.example.com".into()))]
    fn sync_stays_where_it_is_signed_in(
        mut found: Found,
        #[values(Confirm::Ask, Confirm::Yes)] confirm: Confirm,
        #[values(SyncChoice::Enabled, SyncChoice::Disabled)] sync_choice: SyncChoice,
        #[case] server: Option<&str>,
        #[case] expected: Sync,
    ) {
        found.signed_in = true;
        found.sync_choice = sync_choice;
        found.server = server.map(Into::into);
        assert_eq!(setup(&[]).plan_at_screen(&found, confirm).unwrap().sync, expected);
    }

    #[rstest]
    #[case::found_and_kept(true, &[], true)]
    #[case::found_and_turned_off(true, &["--no-threads"], false)]
    #[case::none_found_and_left(false, &[], true)]
    #[case::none_found_and_turned_off(false, &["--no-threads"], false)]
    fn thread_capture(
        mut found: Found,
        #[case] agents: bool,
        #[case] flags: &[&str],
        #[case] expected: bool,
    ) {
        found.threads = true;
        if !agents {
            found.agents.clear();
        }
        assert_eq!(setup(flags).plan_at_screen(&found, Confirm::Ask).unwrap().threads.0, expected);
    }

    #[cfg(feature = "octavo")]
    #[rstest]
    #[case::not_asked(&[], None)]
    #[case::asked(&["--hub"], Some(Hub::On { backfill: false }))]
    #[case::as_18_24_8_spelled_it(&["--teams"], Some(Hub::On { backfill: false }))]
    #[case::with_backfill(&["--hub", "--backfill"], Some(Hub::On { backfill: true }))]
    #[case::turned_off(&["--no-hub"], Some(Hub::Off))]
    fn hub_is_asked_about_with_its_flags(
        mut found: Found,
        #[case] flags: &[&str],
        #[case] expected: Option<Hub>,
    ) {
        found.signed_in = true;
        assert_eq!(
            setup(flags).plan_at_screen(&found, Confirm::Ask).unwrap().effective().hub(),
            expected
        );
    }

    #[rstest]
    #[case("cloud", Some(Sync::Cloud))]
    #[case("off", Some(Sync::Off(OffBecause::Chosen)))]
    #[case("http://localhost:8888", Some(Sync::Server("http://localhost:8888".into())))]
    #[case("atuin.example.com", None)]
    fn sync_flag_takes_cloud_off_or_an_address(
        #[case] value: &str,
        #[case] expected: Option<Sync>,
    ) {
        assert_eq!(parse_sync(value).ok(), expected);
    }

    #[rstest]
    #[case::shells(&["--shells", "fish,zsh"], &[Shell::Fish, Shell::Zsh])]
    #[case::shells_again(&["--shells", "fish", "--shells", "bash"], &[Shell::Fish, Shell::Bash])]
    #[case::here(&[], &[Shell::Zsh, Shell::Bash])]
    fn the_shells_are_those_here_unless_the_flags_name_some(
        found: Found,
        #[case] args: &[&str],
        #[case] expected: &[Shell],
    ) {
        assert_eq!(setup(args).plan_at_screen(&found, Confirm::Ask).unwrap().shells.0, expected);
    }

    #[rstest]
    fn a_shell_setup_cannot_hook_is_refused() {
        let error = Cmd::try_parse_from(["setup", "--shells", "nu"]).unwrap_err().to_string();
        assert!(error.contains("setup hooks into zsh, bash, fish, powershell"), "{error}");
    }

    /// `--agents` picks the agents to set up, of all atuin knows, whether or not they're here.
    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    fn agents_are_those_the_flags_name(mut found: Found) {
        found.agents.clear();
        let plan = setup(&["--agents", "codex", "--agent-history"])
            .plan_at_screen(&found, Confirm::Ask)
            .unwrap();
        let names: Vec<_> = plan.agents.0.iter().map(Harness::name).collect();
        assert_eq!((names, plan.agent_history), (vec!["codex"], AgentHistory(true)));
    }
}
