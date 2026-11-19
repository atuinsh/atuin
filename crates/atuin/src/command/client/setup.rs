//! `atuin setup`: one screen of choices, filled in from what is on this machine, then a live list
//! of what it is doing.

use std::fs::OpenOptions;
use std::io::{self, IsTerminal, Write};
#[cfg(all(feature = "ai", feature = "daemon"))]
use std::num::NonZeroU64;
#[cfg(feature = "ai")]
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
#[cfg(feature = "ai")]
use std::time::{Duration, SystemTime};

use atuin_client::database::Sqlite;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::{RequestedSearchMode, Settings, ThreadUploads};
use atuin_common::dirs::home_dir;
use atuin_common::encryption::paseto_v4;
use atuin_common::env::var_nonempty;
use atuin_common::executable::is_executable_with_path_search;
#[cfg(feature = "ai")]
use atuin_common::harnesstools::{
    InstallHookError,
    mcp::InstallMcp as _,
    session::any::{ObservableExt as _, SessionsExt as _},
};
use atuin_common::path::{AbsPathBuf, PathBufExt as _};
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
mod encoding;
mod form;
mod plan;
mod scroll;
#[cfg(feature = "octavo")]
mod teams;
#[cfg(feature = "octavo")]
mod upload;
mod view;

use agents::AnyHarness;
#[cfg(feature = "ai")]
use agents::Harness;
use encoding::Encoding;
use form::{Flag, Form as _};
#[cfg(feature = "octavo")]
use plan::UploadThreads;
#[cfg(feature = "ai")]
use plan::{AgentHistory, Mcp};
use plan::{
    Asks, AtuinForTeams, Choices, CtrlR, Daemon, OutputCapture, Plan, ShellHistory, Threads,
    UpArrow,
};

use super::config::update_config_file;
use super::init::Shell;

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
        let yes = self.yes || self.only || !io::stdout().is_terminal();
        let found = Found::detect(settings, db).await;
        let plan = self.plan(&found, yes)?;

        if yes {
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
            if let Some(Teams::On { backfill }) = plan.teams() {
                // Not watched: there's no screen to watch it on.
                let _upload = turn_on_teams(store, backfill, &tx).await;
            }
            let mut steps = Vec::new();
            while let Ok(update) = rx.try_recv() {
                if let Update::Step(step, status) = update {
                    view::set(&mut steps, step, status);
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
        Box::pin(view::run(found, &plan, !self.no_import, mouse, store)).await
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
    fn plan(&self, found: &Found, yes: bool) -> Result<Plan, form::FlagError> {
        let flags = self.choices.matches();
        let asks = Asks {
            // As long as Atuin is in one, so it can come out.
            agents: !found.agents.is_empty()
                || !found.connections.is_empty()
                || plan::Agents::from_args(flags).is_some(),
            teams: AtuinForTeams::from_args(flags).is_some(),
            searchable: found.imported > 0 || found.threads_here > 0,
            unhooks: self.only && ShellHistory::from_args(flags) == Some(ShellHistory(false)),
        };
        let here = Plan::from_current_state(found, asks);
        let mut plan = if self.only {
            here
        } else {
            here.proposal(found, yes)
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

/// Agent threads need a daemon to capture them, and the `ai` feature to read them.
const THREADS: bool = cfg!(all(feature = "ai", feature = "daemon"));

/// Sync needs the sync client, which `sync` builds in.
const SYNC: bool = cfg!(feature = "sync");

/// Atuin for Teams needs the hub client, which `octavo` builds in.
const TEAMS: bool = cfg!(feature = "octavo");

/// `--sync`: `cloud`, `off`, or a server's address.
fn parse_sync(value: &str) -> Result<Sync, String> {
    match value {
        "cloud" => Ok(Sync::Cloud),
        "off" => Ok(Sync::Off),
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
    /// Whether `atuin init` binds ctrl-r, and the up arrow, as configured now.
    ctrl_r: bool,
    up_arrow: bool,
    /// Whether the daemon captures agent threads, as configured now.
    capture: bool,
    /// Whether output is captured, as configured now.
    output: bool,
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
    /// A sync server that isn't Atuin Cloud, already configured.
    server: Option<String>,
    /// Which agent threads Atuin for Teams uploads while it's on, as configured now.
    upload_threads: ThreadUploads,
    /// The agents Atuin is in, and how.
    connections: Vec<(AnyHarness, Connection)>,
    /// Whether Atuin for Teams is on, as configured now.
    teams: bool,
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
        #[cfg(feature = "octavo")]
        let (upload_threads, teams) = (settings.teams.upload_threads, settings.teams.enabled);
        // As a build with Atuin for Teams would read an unset config.
        #[cfg(not(feature = "octavo"))]
        let (upload_threads, teams) = (ThreadUploads::All, false);
        Self {
            history: shell_history().await,
            // Less `atuin setup` itself, which a shell that runs Atuin has recorded already.
            imported: (db.history_count(false).await.unwrap_or(0) - i64::from(hooked)).max(0),
            threads_here: threads_here(settings).await,
            agents: detect_agents().await,
            hooked,
            inits: shell_inits(&RcDirs::here()).await,
            shells: shells_here(&RcDirs::here()),
            ctrl_r: settings.bind_ctrl_r,
            up_arrow: settings.bind_up_arrow,
            capture: settings.threads.capture,
            output: settings.output.limits().is_some() && settings.pty_proxy.enabled,
            saves_output: settings.output.limits().is_some(),
            daemon: settings.daemon.enabled,
            daemon_running: daemon_running(settings),
            daemon_off: !settings.daemon.enabled && daemon_setting_is_theirs().await,
            signed_in: signed_in(settings).await,
            server: (!settings.is_hub_sync()).then(|| settings.sync_address.to_string()),
            upload_threads,
            connections: connections().await,
            teams,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Sync {
    Cloud,
    Server(String),
    #[default]
    Off,
}

/// Whether Atuin for Teams is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Teams {
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
    #[cfg(feature = "ai")]
    AgentHooks,
    #[cfg(feature = "ai")]
    Mcp,
    Daemon,
    Sync,
    #[cfg(feature = "octavo")]
    Teams,
    History,
    Threads,
    /// Watching Atuin for Teams upload, after the imports it takes in.
    #[cfg(feature = "octavo")]
    Upload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Running(String),
    Done(String),
    Failed(String),
}

/// What the steps tell the screen.
#[derive(Debug)]
pub enum Update {
    Step(Step, Status),
    /// Sign-in is waiting on the browser, at this address.
    Browser {
        url: String,
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

/// Do what `plan` says, telling the screen as it goes, up to the imports and Atuin for Teams.
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
        let changed = format!("Set {} in your config", names(&changed));
        send(tx, Step::Config, Status::Done(changed));
    }
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
            hook_shells(&shells, !plan.shell.0 && found.hooked && found.inits.is_empty(), tx);
        }
        #[cfg(feature = "ai")]
        connect(plan, found, scope, tx).await;
        if scope.covers::<OutputCapture>() && found.output_changes(plan.output.0) {
            capture_output(plan.output.0, &settings, tx).await;
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

/// Turn Atuin for Teams on, once sign-in has said who the account is, so the hub can say whether
/// it has it, and after the imports, so what it uploads takes them in. Returns the upload to
/// watch, if it started one.
#[cfg(feature = "octavo")]
async fn turn_on_teams(
    store: &SqliteStore,
    backfill: bool,
    tx: &Updates,
) -> Option<upload::Upload> {
    match Settings::new() {
        Ok(settings) => teams::enable(&Arc::new(settings), store, backfill, tx).await,
        Err(e) => {
            let failed = format!(
                "Couldn't turn on Atuin for Teams ({e:#}). Run `atuin setup --hub` to try again"
            );
            send(tx, Step::Teams, Status::Failed(failed));
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
    // Without shell history, the keys are left as they are: the shell may run Atuin already.
    if plan.shell.0 {
        if scope.covers::<CtrlR>() && plan.ctrl_r.0 != settings.bind_ctrl_r {
            changes.push(("bind_ctrl_r", Value::from(plan.ctrl_r.0)));
        }
        if scope.covers::<UpArrow>() && plan.up_arrow.0 != settings.bind_up_arrow {
            changes.push(("bind_up_arrow", Value::from(plan.up_arrow.0)));
        }
    }
    if scope.covers::<Threads>() && plan.threads.0 != settings.threads.capture {
        changes.push(("threads.capture", Value::from(plan.threads.0)));
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
            Sync::Cloud | Sync::Off => {}
        }
    }
    // Turned on after sign-in, once the hub has said the account has it.
    #[cfg(feature = "octavo")]
    if plan.teams() == Some(Teams::Off) && settings.teams.enabled {
        changes.push(("teams.enabled", Value::from(false)));
    }
    // Written before it's turned on, so its backfill takes only what it uploads.
    #[cfg(feature = "octavo")]
    if matches!(plan.teams(), Some(Teams::On { .. }))
        && scope.covers::<UploadThreads>()
        && plan.upload_threads.0 != settings.teams.upload_threads
    {
        changes.push(("teams.upload_threads", Value::from(plan.upload_threads.0.as_str())));
    }
    changes
}

/// Put `atuin init` into the rc files of the shells `changes` adds, and take it out of those it
/// removes. `elsewhere` when shell history is to be off, but this shell loads Atuin from somewhere
/// setup can't tell.
fn hook_shells(changes: &Changes<Shell>, elsewhere: bool, tx: &Updates) {
    let dirs = RcDirs::here();
    let mut done = Vec::new();
    let mut failed = Vec::new();
    if !changes.add.is_empty() {
        match install_shell_init(&dirs, &changes.add) {
            Ok(hooked) if !hooked.is_empty() => {
                done.push(format!("hooked into {}", names(&hooked)));
            }
            Ok(_) => {
                let shells: Vec<_> = changes.add.iter().map(|shell| shell.name()).collect();
                let run = if shells.len() == 1 {
                    "runs"
                } else {
                    "run"
                };
                done.push(format!("{} {run} Atuin already", names(&shells)));
            }
            Err(e) => failed.push(format!("couldn't hook into your shell: {e}")),
        }
    }
    if !changes.remove.is_empty() {
        match remove_shell_init(&dirs, &changes.remove) {
            Ok(Unhooked { shells, left }) => {
                if !shells.is_empty() {
                    done.push(format!("unhooked {}", names(&shells)));
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
async fn connect(plan: &Plan, found: &Found, scope: Scope<'_>, tx: &Updates) {
    let AgentChanges { hooks, mcp } = agent_changes(plan, found, scope);
    if let Some(hooks) = hooks {
        hook_agents(&hooks, tx).await;
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
#[cfg(feature = "ai")]
async fn hook_agents(changes: &Changes<AnyHarness>, tx: &Updates) {
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
                hooked.push(agent.display_name());
                if loads_hooks_on_restart(agent) {
                    restart.push(agent.display_name());
                }
            }
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
        done.push(format!("recording commands from {}", names(&hooked)));
    }
    if !restart.is_empty() {
        done.push(format!("restart {} to load the hooks", names(&restart)));
    }
    if !unhooked.is_empty() {
        done.push(format!("took the hooks out of {}", names(&unhooked)));
    }
    if !unload.is_empty() {
        done.push(format!("restart {} to stop recording there", names(&unload)));
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

/// Whether `plan` turns on the daemon, or thread capture, as far as `scope` reaches, so the daemon
/// starts now. Left for the shell to start, it wouldn't without shell history.
fn wakes_daemon(plan: &Plan, found: &Found, scope: Scope<'_>) -> bool {
    let daemon = scope.covers::<Daemon>() && !found.daemon;
    let threads = scope.covers::<Threads>() && plan.threads.0 && !found.capture;
    plan.daemon.0 && (daemon || threads)
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
        Err(e) => {
            Status::Failed(format!("Couldn't start the daemon ({e:#}): run `atuin daemon start`"))
        }
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
        done.push(format!("{} can search Atuin from their next session", names(&connected)));
    }
    if !disconnected.is_empty() {
        done.push(format!("disconnected {} from Atuin", names(&disconnected)));
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
    /// Setup leaves the agent alone and doesn't mention it. The agent's command isn't on `PATH`,
    /// and the agent hasn't been used here recently.
    Skip,
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
        let sessions = agent.sessions(pool);
        let last_active = tokio::task::spawn_blocking(move || sessions?.last_active());
        let last_active = last_active.await.ok().flatten();
        // A session with a timestamp in the future counts as brand new.
        Self::without_command(
            command,
            last_active.map(|at| now.duration_since(at).unwrap_or_default()),
        )
    }

    /// The action for an agent whose `command` isn't on `PATH`. The agent was last used here
    /// `ago`.
    fn without_command(command: &'static str, ago: Option<Duration>) -> Self {
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
    match &plan.sync {
        Sync::Off if syncing.is_some() => {
            let off = "Signed out: sync is off, and everything stays on this machine";
            send(tx, Step::Sync, Status::Done(off.into()));
            return SignedIn::No;
        }
        Sync::Off => return SignedIn::No,
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
        match status {
            Status::Done(text) => println!("  {} {text}", "✓".green()),
            Status::Failed(text) => println!("  {} {text}", "!".yellow().bold()),
            Status::Running(text) => println!("  {} {}", "·".dimmed(), text.dimmed()),
        }
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

    let mut tips: Vec<(&str, &str)> = Vec::new();
    if plan.ctrl_r.0 {
        tips.push(("ctrl-r", "search your shell history"));
    }
    if plan.up_arrow.0 {
        tips.push(("up arrow", "search it from what you've typed"));
    }
    if plan.threads.0 {
        tips.push(("atuin resume", "pick a thread back up, in any agent"));
    }
    // A shell picks up its key bindings as it starts.
    let keys_changed = plan.ctrl_r.0 != found.ctrl_r || plan.up_arrow.0 != found.up_arrow;
    let shell_changed = !found.hooked || keys_changed || plan.output.0 != found.output;
    let new_terminal = plan.shell.0 && shell_changed;
    if new_terminal || !tips.is_empty() {
        println!();
        let heading = match (new_terminal, tips.is_empty()) {
            (true, true) => "Open a new terminal",
            (true, false) => "Open a new terminal, then try",
            (false, _) => "Try",
        };
        println!("  {}", heading.bold());
        for (keys, what) in tips {
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
        match status {
            Status::Done(text) => println!("  {} {text}", "✓".green()),
            Status::Failed(text) => println!("  {} {text}", "!".yellow().bold()),
            Status::Running(text) => println!("  {} {}", "·".dimmed(), text.dimmed()),
        }
    }
}

/// Where the shells setup hooks into keep their rc files.
struct RcDirs {
    home: PathBuf,
    /// Where zsh reads its rc file from: `$ZDOTDIR`, else the home.
    zdotdir: PathBuf,
    /// The user's documents, where PowerShell keeps its profiles on Windows.
    documents: PathBuf,
}

impl RcDirs {
    fn here() -> Self {
        Self {
            home: home_dir(),
            zdotdir: var_nonempty("ZDOTDIR").map_or_else(home_dir, PathBuf::from),
            documents: atuin_common::dirs::documents_dir(),
        }
    }

    /// The rc files `shell` reads, which setup writes `atuin init` into. PowerShell's are its
    /// profiles: on Windows, PowerShell 7 and Windows PowerShell each have one, and Atuin's
    /// script runs in both.
    fn rc_files(&self, shell: Shell) -> Vec<PathBuf> {
        const PROFILE: &str = "Microsoft.PowerShell_profile.ps1";
        match shell {
            Shell::Zsh => vec![self.zdotdir.join(".zshrc")],
            Shell::Bash => vec![self.home.join(".bashrc")],
            Shell::Fish => vec![self.home.join(".config").joined("fish").joined("config.fish")],
            Shell::PowerShell if cfg!(windows) => ["PowerShell", "WindowsPowerShell"]
                .map(|dir| self.documents.join(dir).joined(PROFILE))
                .into(),
            Shell::PowerShell => {
                vec![self.home.join(".config").joined("powershell").joined(PROFILE)]
            }
            Shell::Nu | Shell::Xonsh => Vec::new(),
        }
    }
}

/// What in an rc file shows that it loads Atuin, for the shells setup hooks into.
const fn init_marker(shell: Shell) -> Option<&'static str> {
    match shell {
        Shell::Zsh => Some("atuin init zsh"),
        Shell::Bash => Some("atuin init bash"),
        Shell::Fish => Some("atuin init fish"),
        Shell::PowerShell => Some("atuin init powershell"),
        Shell::Nu | Shell::Xonsh => None,
    }
}

/// The programs that run `shell`, any of which on `PATH` means it's here.
const fn programs(shell: Shell) -> &'static [&'static str] {
    match shell {
        Shell::Zsh => &["zsh"],
        Shell::Bash => &["bash"],
        Shell::Fish => &["fish"],
        Shell::PowerShell => &["pwsh", "powershell"],
        Shell::Nu => &["nu"],
        Shell::Xonsh => &["xonsh"],
    }
}

/// The shells an rc file of which loads Atuin.
async fn shell_inits(dirs: &RcDirs) -> Vec<Shell> {
    let mut inits = Vec::new();
    for shell in plan::HOOKABLE {
        let Some(marker) = init_marker(shell) else {
            continue;
        };
        for rc in dirs.rc_files(shell) {
            let read = tokio::fs::read(&rc).await;
            let text = read.ok().and_then(|bytes| Encoding::decode(&bytes).ok());
            if text.is_some_and(|(_, rc)| rc.contains(marker)) {
                inits.push(shell);
                break;
            }
        }
    }
    inits
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

/// The shells here that setup can hook into: those on `PATH`, or with an rc file.
fn shells_here(dirs: &RcDirs) -> Vec<Shell> {
    let here = |shell: &Shell| {
        let rc = dirs.rc_files(*shell).iter().any(|rc| rc.exists());
        rc || programs(*shell).iter().any(is_executable_with_path_search)
    };
    plan::HOOKABLE.into_iter().filter(here).collect()
}

/// Add `atuin init` to the rc files of each of `shells`. Returns the shells it was added to, less
/// those it was in already.
fn install_shell_init(dirs: &RcDirs, shells: &[Shell]) -> io::Result<Vec<&'static str>> {
    let mut hooked = Vec::new();
    for &shell in shells {
        let Some(marker) = init_marker(shell) else {
            continue;
        };
        let mut added = false;
        for rc in dirs.rc_files(shell) {
            let init = match shell {
                Shell::Zsh => "\neval \"$(atuin init zsh)\"\n",
                // A `.bashrc` made here would be the only one bash reads, so it needs the
                // installer's PATH.
                Shell::Bash if !rc.exists() && installer_env(&dirs.home).exists() => {
                    ". \"$HOME/.atuin/bin/env\"\neval \"$(atuin init bash)\"\n"
                }
                Shell::Bash => "\neval \"$(atuin init bash)\"\n",
                Shell::Fish => "\nif status is-interactive\n    atuin init fish | source\nend\n",
                Shell::PowerShell => "\natuin init powershell | Out-String | Invoke-Expression\n",
                Shell::Nu | Shell::Xonsh => continue,
            };
            // Fish and PowerShell keep theirs in a directory a fresh install hasn't made.
            if let Some(dir) = rc.parent() {
                std::fs::create_dir_all(dir)?;
            }
            added |= append_once(&rc, marker, init)?;
        }
        if added {
            hooked.push(shell.name());
        }
    }
    Ok(hooked)
}

/// The script the installer writes to put Atuin on `PATH`.
fn installer_env(home: &Path) -> PathBuf {
    home.join(".atuin").joined("bin").joined("env")
}

/// The shells whose rc files `remove_shell_init` took `atuin init` out of, and the lines loading
/// Atuin it left, each with its rc file, which setup didn't write.
struct Unhooked {
    shells: Vec<&'static str>,
    left: Vec<(PathBuf, String)>,
}

/// Take the `atuin init` that [`install_shell_init`] writes out of the rc files of each of
/// `shells`.
fn remove_shell_init(dirs: &RcDirs, shells: &[Shell]) -> io::Result<Unhooked> {
    let mut unhooked = Unhooked {
        shells: Vec::new(),
        left: Vec::new(),
    };
    for &shell in shells {
        let Some(marker) = init_marker(shell) else {
            continue;
        };
        let mut removed = false;
        for rc in dirs.rc_files(shell) {
            // An rc file linked from a dotfiles repo is saved there, keeping the link.
            let target = AbsPathBuf::canonicalize(&rc).map_or_else(|_| rc.clone(), PathBuf::from);
            let read = match std::fs::read(&target) {
                Ok(read) => read,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let (encoding, text) = Encoding::decode(&read)?;
            let without = without_init(&text, shell);
            if let Some(without) = &without {
                let bytes = encoding.encode(without, true);
                atuin_common::fs::replace(&target, &bytes, |now| now == read)
                    .map_err(io::Error::other)?;
                removed = true;
            }
            let after = without.as_deref().unwrap_or(&text);
            let left = after.lines().filter(|line| line.contains(marker));
            unhooked.left.extend(left.map(|line| (rc.clone(), line.trim().to_owned())));
        }
        if removed {
            unhooked.shells.push(shell.name());
        }
    }
    Ok(unhooked)
}

/// `rc` without the `atuin init` that [`install_shell_init`] writes for `shell`, and the blank
/// line it writes before it. `None` when it has none. Anything else loading Atuin stays, as
/// setup can't tell what a change to it would break: the same line indented, or inside a block
/// the user opened, as `if command -v atuin; then` does.
fn without_init(rc: &str, shell: Shell) -> Option<String> {
    let written: &[&str] = match shell {
        Shell::Zsh => &["eval \"$(atuin init zsh)\""],
        Shell::Bash => &["eval \"$(atuin init bash)\""],
        Shell::Fish => &["if status is-interactive", "    atuin init fish | source", "end"],
        Shell::PowerShell => &["atuin init powershell | Out-String | Invoke-Expression"],
        Shell::Nu | Shell::Xonsh => return None,
    };
    let lines: Vec<&str> = rc.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut at = 0;
    let mut removed = false;
    while at < lines.len() {
        let block = lines.get(at..at + written.len());
        let is_written = block.is_some_and(|block| {
            block.iter().zip(written).all(|(line, written)| line.trim_end() == *written)
        });
        let before = kept.iter().rev().find(|line| !line.trim().is_empty());
        let after = lines.iter().skip(at + written.len()).find(|line| !line.trim().is_empty());
        let nested = before.is_some_and(|line| opens_block(line))
            || after.is_some_and(|line| closes_block(line));
        if is_written && !nested {
            if kept.last().is_some_and(|line| line.trim().is_empty()) {
                kept.pop();
            }
            at += written.len();
            removed = true;
            continue;
        }
        kept.push(lines[at]);
        at += 1;
    }
    if !removed {
        return None;
    }
    // As the file ends its lines, which on Windows is often `\r\n`.
    let newline = newline_of(rc);
    let mut without = kept.join(newline);
    if rc.ends_with('\n') && !without.is_empty() {
        without.push_str(newline);
    }
    Some(without)
}

/// Whether `line` opens a block the lines after it are in, as `then` and `{` do.
fn opens_block(line: &str) -> bool {
    let last = line.trim_end().rsplit([' ', '\t', ';']).next();
    last.is_some_and(|word| matches!(word, "then" | "do" | "else" | "{"))
}

/// Whether `line` closes a block, as `fi` and `}` do.
fn closes_block(line: &str) -> bool {
    let first = line.trim_start().split([' ', '\t', ';']).next();
    first.is_some_and(|word| matches!(word, "fi" | "done" | "esac" | "}" | "end"))
}

/// How `text` ends its lines: `\r\n` if it does anywhere, else `\n`.
fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Append `text` to the file at `path` unless it holds `marker`, with the file's encoding and line
/// endings. Returns whether it appended.
fn append_once(path: &Path, marker: &str, text: &str) -> io::Result<bool> {
    let (encoding, newline) = match std::fs::read(path) {
        Ok(read) => match Encoding::decode(&read)? {
            (_, existing) if existing.contains(marker) => return Ok(false),
            (encoding, existing) => (encoding, newline_of(&existing)),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => (Encoding::Utf8, "\n"),
        Err(e) => return Err(e),
    };
    let text = encoding.encode(&text.replace('\n', newline), false);
    OpenOptions::new().create(true).append(true).open(path)?.write_all(&text)?;
    Ok(true)
}

/// "Claude Code, Codex and pi".
fn names(names: &[&str]) -> String {
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
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
    use tempfile::TempDir;

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
        assert_eq!(McpAction::without_command("grok", ago), expected);
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
        assert_eq!(names(list), expected);
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
            teams: false,
            searchable: false,
            unhooks: false,
        };
        let here = Plan::from_current_state(&found, asks);
        let names = |plan: &Plan| plan.agents.0.iter().map(Harness::name).collect::<Vec<_>>();
        assert_eq!(names(&here), ["codex"]);
        assert_eq!((here.agent_history, here.mcp), (AgentHistory(true), Mcp(false)));

        let proposed = here.proposal(&found, false);
        assert_eq!(names(&proposed), ["codex"]);
        assert_eq!((proposed.agent_history, proposed.mcp), (AgentHistory(true), Mcp(true)));
    }

    #[rstest]
    #[case::zsh(Shell::Zsh, "export A=1\n\neval \"$(atuin init zsh)\"\n", Some("export A=1\n"))]
    #[case::bash_keeps_the_path(
        Shell::Bash,
        ". \"$HOME/.atuin/bin/env\"\neval \"$(atuin init bash)\"\n",
        Some(". \"$HOME/.atuin/bin/env\"\n")
    )]
    #[case::fish(
        Shell::Fish,
        "set -x A 1\n\nif status is-interactive\n    atuin init fish | source\nend\n",
        Some("set -x A 1\n")
    )]
    #[case::alone(Shell::Zsh, "\neval \"$(atuin init zsh)\"\n", Some(""))]
    #[case::powershell(
        Shell::PowerShell,
        "Set-Alias g git\r\n\r\natuin init powershell | Out-String | Invoke-Expression\r\n",
        Some("Set-Alias g git\r\n")
    )]
    #[case::its_own_flags(Shell::Zsh, "eval \"$(atuin init zsh --disable-up-arrow)\"\n", None)]
    #[case::guarded(
        Shell::Bash,
        "if command -v atuin >/dev/null; then\n  eval \"$(atuin init bash)\"\nfi\n",
        None
    )]
    #[case::guarded_unindented(
        Shell::Bash,
        "if command -v atuin >/dev/null; then\neval \"$(atuin init bash)\"\nfi\n",
        None
    )]
    #[case::in_a_function(Shell::Zsh, "atuin_on() {\neval \"$(atuin init zsh)\"\n}\n", None)]
    #[case::fish_in_a_function(
        Shell::Fish,
        "function atuin_on\nif status is-interactive\n    atuin init fish | source\nend\nend\n",
        None
    )]
    #[case::after_a_block(
        Shell::Zsh,
        "if true; then\n  export A=1\nfi\n\neval \"$(atuin init zsh)\"\n",
        Some("if true; then\n  export A=1\nfi\n")
    )]
    #[case::none(Shell::Zsh, "export A=1\n", None)]
    fn the_init_setup_wrote_comes_out_and_nothing_else(
        #[case] shell: Shell,
        #[case] rc: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(without_init(rc, shell).as_deref(), expected);
    }

    /// A profile written with Windows line endings gets the init with them too.
    #[rstest]
    fn shell_init_keeps_the_files_line_endings() {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let profile = dirs.rc_files(Shell::PowerShell).remove(0);
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        std::fs::write(&profile, "Set-Alias g git\r\n").unwrap();

        install_shell_init(&dirs, &[Shell::PowerShell]).unwrap();
        let hooked = std::fs::read_to_string(&profile).unwrap();
        let init = "atuin init powershell | Out-String | Invoke-Expression";
        assert_eq!(hooked, format!("Set-Alias g git\r\n\r\n{init}\r\n"));
        remove_shell_init(&dirs, &[Shell::PowerShell]).unwrap();
        assert_eq!(std::fs::read_to_string(&profile).unwrap(), "Set-Alias g git\r\n");
    }

    /// A profile Windows PowerShell 5.1 wrote as UTF-16 gets the init in UTF-16, and setup finds it
    /// there and takes it out again.
    #[rstest]
    #[tokio::test]
    async fn shell_init_keeps_the_files_encoding(
        #[values(Encoding::Utf16Le, Encoding::Utf16Be)] encoding: Encoding,
    ) {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let profile = dirs.rc_files(Shell::PowerShell).remove(0);
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        let mine = encoding.encode("Set-Alias g git\r\n", true);
        std::fs::write(&profile, &mine).unwrap();

        install_shell_init(&dirs, &[Shell::PowerShell]).unwrap();
        let init = "atuin init powershell | Out-String | Invoke-Expression";
        let hooked = format!("Set-Alias g git\r\n\r\n{init}\r\n");
        let read = std::fs::read(&profile).unwrap();
        assert_eq!(Encoding::decode(&read).unwrap(), (encoding, hooked));
        assert_eq!(shell_inits(&dirs).await, [Shell::PowerShell]);
        remove_shell_init(&dirs, &[Shell::PowerShell]).unwrap();
        assert_eq!(std::fs::read(&profile).unwrap(), mine);
    }

    /// An rc file linked from a dotfiles repo loses the init there, and stays a link.
    #[cfg(unix)]
    #[rstest]
    fn shell_init_comes_out_where_a_link_leads() {
        let home = TempDir::new().unwrap();
        let dotfiles = home.path().join("dotfiles");
        std::fs::create_dir(&dotfiles).unwrap();
        let tracked = dotfiles.join("zshrc");
        std::fs::write(&tracked, "export A=1\n").unwrap();
        let zshrc = home.path().join(".zshrc");
        std::os::unix::fs::symlink(&tracked, &zshrc).unwrap();
        let dirs = RcDirs::under(home.path());
        install_shell_init(&dirs, &[Shell::Zsh]).unwrap();

        remove_shell_init(&dirs, &[Shell::Zsh]).unwrap();
        assert!(zshrc.symlink_metadata().unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&tracked).unwrap(), "export A=1\n");
    }

    /// Taken out, the init setup wrote leaves the rc file as it found it, and says what else loads
    /// Atuin that it left.
    #[rstest]
    fn shell_init_comes_out_as_it_went_in() {
        let home = TempDir::new().unwrap();
        let zshrc = home.path().join(".zshrc");
        std::fs::write(&zshrc, "export A=1\n").unwrap();
        let bashrc = home.path().join(".bashrc");
        let custom = "eval \"$(atuin init bash --disable-up-arrow)\"";
        std::fs::write(&bashrc, format!("{custom}\n")).unwrap();
        let dirs = RcDirs::under(home.path());
        install_shell_init(&dirs, &[Shell::Zsh]).unwrap();

        let shells = [Shell::Zsh, Shell::Bash];
        let unhooked = remove_shell_init(&dirs, &shells).unwrap();
        assert_eq!(unhooked.shells, ["zsh"]);
        assert_eq!(unhooked.left, [(bashrc, custom.to_owned())]);
        assert_eq!(std::fs::read_to_string(&zshrc).unwrap(), "export A=1\n");
    }

    #[fixture]
    fn found() -> Found {
        Found {
            history: Some(("zsh", 48210)),
            imported: 0,
            threads_here: 0,
            agents: vec![(AnyHarness::ClaudeCode(atuin_common::harnesstools::ccode::Ccode), 353)],
            hooked: false,
            inits: Vec::new(),
            shells: vec![Shell::Zsh, Shell::Bash],
            ctrl_r: true,
            up_arrow: true,
            capture: false,
            output: false,
            saves_output: false,
            daemon: false,
            daemon_running: false,
            daemon_off: false,
            signed_in: false,
            server: None,
            upload_threads: ThreadUploads::All,
            connections: Vec::new(),
            teams: false,
        }
    }

    impl RcDirs {
        /// Every rc file under `root`.
        fn under(root: &Path) -> Self {
            Self {
                home: root.to_owned(),
                zdotdir: root.to_owned(),
                documents: root.join("Documents"),
            }
        }
    }

    /// `atuin setup` with `args`.
    fn setup(args: &[&str]) -> Cmd {
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
        assert_eq!(setup(&flags).plan(&found, false).unwrap(), Plan {
            asks: Asks {
                agents: true,
                teams: false,
                searchable: false,
                unhooks: false,
            },
            shell: ShellHistory(true),
            shells: plan::Shells(vec![Shell::Zsh, Shell::Bash]),
            output: OutputCapture(false),
            threads: Threads(THREADS),
            agent_history: AgentHistory(false),
            agents: plan::Agents(found.agents()),
            ctrl_r: CtrlR(true),
            up_arrow: UpArrow(false),
            mcp: Mcp(false),
            daemon: Daemon(true),
            sync: Sync::Off,
            atuin_for_teams: AtuinForTeams(false),
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

    #[rstest]
    #[case::ctrl_r(&["--no-shell-history", "--ctrl-r"], "`--ctrl-r` needs shell history: `--shell-history`")]
    #[case::up_arrow(&["--no-shell-history", "--up-arrow"], "`--up-arrow` needs shell history: `--shell-history`")]
    #[cfg_attr(
        all(feature = "pty-proxy", feature = "daemon", unix),
        case::output(&["--no-daemon", "--output-capture"], "`--output-capture` needs the daemon: `--daemon`")
    )]
    #[cfg_attr(
        all(feature = "ai", feature = "daemon"),
        case::threads(&["--no-daemon", "--threads"], "`--threads` needs the daemon: `--daemon`")
    )]
    #[cfg_attr(
        feature = "octavo",
        case::teams(
            &["--hub", "--sync", "off"],
            "`--hub` needs sync: `--sync cloud`, or your own server's address",
        )
    )]
    #[cfg_attr(
        feature = "octavo",
        case::upload_threads(
            &["--no-hub", "--upload-threads", "workspaces"],
            "`--upload-threads workspaces` needs Atuin for Teams: `--hub`",
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
    #[cfg_attr(feature = "octavo", case::backfill(&["--backfill"], "`--backfill` can't be on here: setup asks about Atuin for Teams only with `--hub`"))]
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
        let error = setup(args).plan(&found, false).unwrap_err();
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
        assert_eq!(setup(&args).plan(&found, false).unwrap().mcp, Mcp(true));
    }

    #[rstest]
    fn agent_flags_need_an_agent(mut found: Found) {
        found.agents.clear();
        let expected = if AGENTS {
            "`--mcp` can't be on here: there's no coding agent on this machine"
        } else {
            "`--mcp` can't be on here: this atuin was built without agent support"
        };
        let error = setup(&["--mcp"]).plan(&found, false).unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert!(setup(&["--no-mcp"]).plan(&found, false).is_ok());
    }

    /// `--only` changes what its flags name and nothing else, as the commands it replaced did.
    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    fn only_starts_from_what_is_here(mut found: Found) {
        found.capture = true;
        found.inits = vec![Shell::Zsh];
        let here = Plan::from_current_state(&found, Asks {
            agents: true,
            teams: false,
            searchable: false,
            unhooks: false,
        });
        let plan = setup(&["--only", "--mcp", "--no-threads"]).plan(&found, true).unwrap();
        assert_eq!(plan, Plan {
            mcp: Mcp(true),
            threads: Threads(false),
            ..here
        });
    }

    /// What `atuin setup <args>` writes, against a config that reads as `settings`.
    fn changes(found: &Found, settings: &Settings, args: &[&str]) -> Vec<(&'static str, String)> {
        let cmd = setup(args);
        let plan = cmd.plan(found, true).unwrap().effective();
        let changes = config_changes(&plan, found, cmd.scope(), settings);
        changes.into_iter().map(|(key, value)| (key, value.to_string().trim().to_owned())).collect()
    }

    /// Turns the daemon on, here and in its config.
    fn daemon_on(found: &mut Found, settings: &mut Settings) {
        found.daemon = true;
        settings.daemon.enabled = true;
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
    #[case::daemon_off_keeps_another_search(&["--only", "--no-daemon"], |found: &mut Found, settings: &mut Settings| {
        daemon_on(found, settings);
        settings.requested_search_mode = RequestedSearchMode::Prefix;
    }, &[("daemon.enabled", "false"), ("daemon.autostart", "false")])]
    #[case::key(&["--only", "--no-up-arrow"], |found: &mut Found, _: &mut Settings| found.inits = vec![Shell::Zsh], &[
        ("bind_up_arrow", "false"),
    ])]
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
        case::uploads_with_teams(
            &["--only", "--hub", "--upload-threads", "workspaces"],
            |found: &mut Found, settings: &mut Settings| {
                daemon_on(found, settings);
                found.signed_in = true;
            },
            &[("teams.upload_threads", "\"workspaces\"")],
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
        let plan = cmd.plan(&found, true).unwrap().effective();
        assert_eq!(stops_daemon(&plan, &found, cmd.scope()), expected);
    }

    /// With `--only`, shells named beside `--no-shell-history` are those it comes out of. Named
    /// alone while shell history is off, they need it on.
    #[rstest]
    fn shells_named_narrow_unhooking(mut found: Found) {
        found.inits = vec![Shell::Zsh, Shell::Bash];
        let plan = setup(&["--only", "--no-shell-history", "--shells", "bash"]).plan(&found, true);
        assert_eq!(plan.unwrap().effective().shells, plan::Shells(vec![Shell::Bash]));

        found.inits.clear();
        let error = setup(&["--only", "--shells", "zsh"]).plan(&found, true).unwrap_err();
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
    #[case::daemon_off(&["--only", "--no-daemon"], true, false, false)]
    #[case::elsewhere(&["--only", "--no-up-arrow"], false, false, false)]
    fn the_daemon_wakes_for_what_needs_it(
        mut found: Found,
        #[case] args: &[&str],
        #[case] daemon: bool,
        #[case] capture: bool,
        #[case] expected: bool,
    ) {
        (found.daemon, found.capture) = (daemon, capture);
        let cmd = setup(args);
        let plan = cmd.plan(&found, true).unwrap().effective();
        assert_eq!(wakes_daemon(&plan, &found, cmd.scope()), expected);
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
        let plan = cmd.plan(&found, true).unwrap().effective();
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
        assert_eq!(setup(&[]).plan(&found, false).unwrap().daemon, Daemon(expected));
    }

    #[rstest]
    #[case::asked(false, None, Sync::Cloud)]
    #[case::not_asked(true, None, Sync::Off)]
    #[case::their_server(false, Some("https://atuin.example.com"), Sync::Server("https://atuin.example.com".into()))]
    fn sync_without_the_flag(
        mut found: Found,
        #[case] yes: bool,
        #[case] server: Option<&str>,
        #[case] expected: Sync,
    ) {
        found.server = server.map(Into::into);
        // Without sync, there's nowhere to sync to.
        let expected = if cfg!(feature = "sync") {
            expected
        } else {
            Sync::Off
        };
        assert_eq!(setup(&[]).plan(&found, yes).unwrap().sync, expected);
    }

    #[rstest]
    #[case::cloud(None, Sync::Cloud)]
    #[case::their_server(Some("https://atuin.example.com"), Sync::Server("https://atuin.example.com".into()))]
    fn sync_stays_where_it_is_signed_in(
        mut found: Found,
        #[values(false, true)] yes: bool,
        #[case] server: Option<&str>,
        #[case] expected: Sync,
    ) {
        found.signed_in = true;
        found.server = server.map(Into::into);
        assert_eq!(setup(&[]).plan(&found, yes).unwrap().sync, expected);
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
        found.capture = true;
        if !agents {
            found.agents.clear();
        }
        assert_eq!(setup(flags).plan(&found, false).unwrap().threads.0, expected);
    }

    #[cfg(feature = "octavo")]
    #[rstest]
    #[case::not_asked(&[], None)]
    #[case::asked(&["--hub"], Some(Teams::On { backfill: false }))]
    #[case::as_18_24_8_spelled_it(&["--teams"], Some(Teams::On { backfill: false }))]
    #[case::with_backfill(&["--hub", "--backfill"], Some(Teams::On { backfill: true }))]
    #[case::turned_off(&["--no-hub"], Some(Teams::Off))]
    fn teams_is_asked_about_with_its_flags(
        mut found: Found,
        #[case] flags: &[&str],
        #[case] expected: Option<Teams>,
    ) {
        found.signed_in = true;
        assert_eq!(setup(flags).plan(&found, false).unwrap().effective().teams(), expected);
    }

    #[rstest]
    #[case("cloud", Some(Sync::Cloud))]
    #[case("off", Some(Sync::Off))]
    #[case("http://localhost:8888", Some(Sync::Server("http://localhost:8888".into())))]
    #[case("atuin.example.com", None)]
    fn sync_flag_takes_cloud_off_or_an_address(
        #[case] value: &str,
        #[case] expected: Option<Sync>,
    ) {
        assert_eq!(parse_sync(value).ok(), expected);
    }

    #[rstest]
    fn shell_init_is_added_once() {
        let home = TempDir::new().unwrap();
        std::fs::create_dir_all(home.path().join(".atuin/bin")).unwrap();
        std::fs::write(home.path().join(".atuin/bin/env"), "").unwrap();

        let shells = [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::PowerShell];
        let dirs = RcDirs::under(home.path());
        let first = install_shell_init(&dirs, &shells).unwrap();
        let again = install_shell_init(&dirs, &shells).unwrap();
        let all = vec!["zsh", "bash", "fish", "powershell"];
        assert_eq!((first, again), (all, Vec::<&str>::new()));

        let bashrc = std::fs::read_to_string(home.path().join(".bashrc")).unwrap();
        assert_eq!(bashrc, ". \"$HOME/.atuin/bin/env\"\neval \"$(atuin init bash)\"\n");
        let zshrc = std::fs::read_to_string(home.path().join(".zshrc")).unwrap();
        assert_eq!(zshrc.matches("atuin init zsh").count(), 1);
        // Into a config directory fish hadn't made yet.
        let fish = std::fs::read_to_string(home.path().join(".config/fish/config.fish")).unwrap();
        assert_eq!(fish.matches("atuin init fish").count(), 1);
        let profiles = dirs.rc_files(Shell::PowerShell);
        let profile = std::fs::read_to_string(&profiles[0]).unwrap();
        assert_eq!(profile, "\natuin init powershell | Out-String | Invoke-Expression\n");
    }

    #[rstest]
    fn shell_init_goes_only_where_it_is_asked_for() {
        let home = TempDir::new().unwrap();
        install_shell_init(&RcDirs::under(home.path()), &[Shell::Fish]).unwrap();
        assert!(!home.path().join(".zshrc").exists() && !home.path().join(".bashrc").exists());
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
        assert_eq!(setup(args).plan(&found, false).unwrap().shells.0, expected);
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
        let plan = setup(&["--agents", "codex", "--agent-history"]).plan(&found, false).unwrap();
        let names: Vec<_> = plan.agents.0.iter().map(Harness::name).collect();
        assert_eq!((names, plan.agent_history), (vec!["codex"], AgentHistory(true)));
    }
}
