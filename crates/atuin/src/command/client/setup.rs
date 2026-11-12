//! `atuin setup`: one screen of choices, filled in from what is on this machine, then a live list
//! of what it is doing.

use std::fs::OpenOptions;
use std::io::{self, IsTerminal, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use atuin_client::database::Sqlite;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::dirs::home_dir;
use atuin_common::encryption::paseto_v4;
use atuin_common::env::var_nonempty;
use atuin_common::executable::is_executable_with_path_search;
use atuin_common::harnesstools::mcp::InstallMcp as _;
use atuin_common::harnesstools::session::any::{ObservableExt as _, SessionsExt as _};
use atuin_common::harnesstools::{AnyHarness, Harness, InstallHookError};
use atuin_common::path::PathBufExt as _;
use atuin_common::sync::BlockingPool;
use atuin_common::time::DurationExt as _;
use clap::Parser;
use colored::Colorize;
use easy_cast::Conv;
use eyre::Result;
use tokio::sync::mpsc;
use toml_edit::Value;
use tracing::instrument;

#[cfg(feature = "octavo")]
mod teams;
mod view;

/// Each flag sets a choice on the first screen, which starts from what's found here.
#[derive(Parser, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "a flag each, on or off")]
pub struct Cmd {
    /// Set up without asking, as the flags say: sync is off unless `--sync` says otherwise
    #[arg(long)]
    yes: bool,

    /// Don't capture your shell history
    #[arg(long)]
    no_shell_history: bool,

    /// Don't capture your conversations with your agents
    #[arg(long)]
    no_threads: bool,

    /// Don't capture the commands your agents run
    #[arg(long)]
    no_agent_history: bool,

    /// Leave ctrl-r to your shell
    #[arg(long)]
    no_ctrl_r: bool,

    /// Leave the up arrow to your shell
    #[arg(long)]
    no_up_arrow: bool,

    /// Don't let your agents search Atuin over MCP
    #[arg(long)]
    no_mcp: bool,

    /// Where to sync: `cloud`, `off`, or your own server's address
    #[arg(long, value_name = "cloud|off|URL", value_parser = parse_sync)]
    sync: Option<Sync>,

    /// Don't import the shell history and threads already on this machine
    #[arg(long)]
    no_import: bool,

    /// Turn on Atuin for Teams
    // Hidden until Atuin for Teams ships.
    #[arg(long, hide = true)]
    teams: bool,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings, db: &Sqlite, store: &SqliteStore) -> Result<()> {
        if self.teams && !TEAMS {
            eyre::bail!(
                "this atuin has no Atuin for Teams: it was built without the `octavo` feature"
            );
        }
        let yes = self.yes || !io::stdout().is_terminal();
        let found = Found::detect(settings, db).await;
        let plan = self.plan(&found, yes);
        if yes && plan.sync == Sync::Cloud && found.syncing() != Some(Sync::Cloud) {
            eyre::bail!("signing in to Atuin Cloud needs you: run `atuin setup` without --yes");
        }
        if yes && plan.teams == Some(Teams::Off) && self.teams {
            eyre::bail!("Atuin for Teams needs sync: run `atuin setup --teams` without --yes");
        }

        if yes {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let (_answer, answers) = mpsc::unbounded_channel();
            let offer = Box::pin(apply(&plan, &found, store, &tx, answers)).await?;
            if self.no_import {
                skip_import(&offer, &tx);
            } else {
                import(&offer, &tx).await;
            }
            let mut steps = Vec::new();
            while let Ok(update) = rx.try_recv() {
                if let Update::Step(step, status) = update {
                    view::set(&mut steps, step, status);
                }
            }
            print_summary(&plan, &found, &steps).await;
            return Ok(());
        }

        Box::pin(view::run(found, &plan, !self.no_import, store)).await
    }

    /// What's found here, less what the flags turn off: the first screen starts from this, and
    /// `--yes` takes it as it is.
    fn plan(&self, found: &Found, yes: bool) -> Plan {
        let agents = THREADS && !found.agents.is_empty();
        let sync = match (&self.sync, found.syncing()) {
            (Some(sync), _) => sync.clone(),
            // Left as it is, rather than signing them out.
            (None, Some(syncing)) => syncing,
            (None, None) if yes || !cfg!(feature = "sync") => Sync::Off,
            (None, None) => found.server.clone().map_or(Sync::Cloud, Sync::Server),
        };
        // Atuin for Teams uploads as the account sync signs in to, so it needs sync.
        let teams = self.teams.then_some(if sync == Sync::Off {
            Teams::Off
        } else {
            Teams::On { backfill: false }
        });
        Plan {
            shell: !self.no_shell_history,
            // With no agents to ask about, as it is.
            threads: if agents {
                !self.no_threads
            } else {
                found.capture
            },
            agent_history: agents && !self.no_agent_history,
            ctrl_r: found.ctrl_r && !self.no_ctrl_r,
            up_arrow: found.up_arrow && !self.no_up_arrow,
            mcp: agents && !self.no_mcp,
            sync,
            teams,
        }
    }
}

/// Agent threads need a daemon to capture them, and Atuin AI to read them.
const THREADS: bool = cfg!(all(feature = "ai", feature = "daemon"));

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
    /// Whether `atuin init` binds ctrl-r, and the up arrow, as configured now.
    ctrl_r: bool,
    up_arrow: bool,
    /// Whether the daemon captures agent threads, as configured now.
    capture: bool,
    signed_in: bool,
    /// A sync server that isn't Atuin Cloud, already configured.
    server: Option<String>,
}

impl Found {
    async fn detect(settings: &Settings, db: &Sqlite) -> Self {
        let hooked = var_nonempty("ATUIN_SESSION").is_some();
        Self {
            history: shell_history().await,
            // Less `atuin setup` itself, which a shell that runs Atuin has recorded already.
            imported: (db.history_count(false).await.unwrap_or(0) - i64::from(hooked)).max(0),
            threads_here: threads_here(settings).await,
            agents: detect_agents().await,
            hooked,
            ctrl_r: settings.bind_ctrl_r,
            up_arrow: settings.bind_up_arrow,
            capture: settings.threads.capture,
            signed_in: signed_in(settings).await,
            server: (!settings.is_hub_sync()).then(|| settings.sync_address.to_string()),
        }
    }

    fn agents(&self) -> Vec<AnyHarness> {
        self.agents.iter().map(|&(agent, _)| agent).collect()
    }

    fn threads(&self) -> u64 {
        self.agents.iter().map(|&(_, threads)| threads).sum()
    }

    /// Where this machine syncs now, if it's signed in.
    fn syncing(&self) -> Option<Sync> {
        self.signed_in.then(|| self.server.clone().map_or(Sync::Cloud, Sync::Server))
    }
}

/// What to set up, as chosen on the first screen.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "a choice each, on or off")]
pub struct Plan {
    /// What to capture.
    shell: bool,
    threads: bool,
    /// The commands agents run.
    agent_history: bool,
    /// How to search it.
    ctrl_r: bool,
    up_arrow: bool,
    mcp: bool,
    sync: Sync,
    /// Atuin for Teams, when setup asks about it.
    teams: Option<Teams>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sync {
    Cloud,
    Server(String),
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
    Shells,
    AgentHooks,
    Mcp,
    Sync,
    #[cfg(feature = "octavo")]
    Teams,
    History,
    Threads,
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

/// Do what `plan` says, telling the screen as it goes. Sign-in, if any, reads `answers`. Returns
/// what there is to import, now sign-in has said what the account holds.
async fn apply(
    plan: &Plan,
    found: &Found,
    store: &SqliteStore,
    tx: &Updates,
    answers: mpsc::UnboundedReceiver<Answer>,
) -> Result<Offer> {
    write_config(plan).await?;
    let settings = Arc::new(Settings::new()?);
    // Before anything writes records, so the daemon and sign-in agree on it.
    paseto_v4::Key::try_load_or_generate(&settings.key_path)?;
    let agents = found.agents();

    let hooks = async {
        if plan.shell {
            hook_shells(found, tx);
        }
        if plan.agent_history {
            hook_agents(&agents, tx).await;
        }
        if plan.mcp {
            connect_agents(&agents, tx).await;
        }
    };
    let sync = sync(plan, found, &settings, store, tx, answers);

    let ((), signed_in) = tokio::join!(hooks, sync);
    let signed_in = match signed_in {
        // That restarts the daemon.
        SignedIn::NeedsKey(key) => {
            let stored = use_account_key(&settings, store, &key, tx).await;
            if stored {
                sync_soon(&settings, tx).await;
            } else {
                sign_out().await;
            }
            stored
        }
        SignedIn::Yes => {
            sync_soon(&settings, tx).await;
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
    // Once sign-in has said who the account is, so the hub can say whether it has it.
    #[cfg(feature = "octavo")]
    if let Some(Teams::On { backfill }) = plan.teams {
        teams::enable(&settings, store, backfill, tx).await;
    }
    Ok(offer(plan, found, account, tx))
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
    if plan.shell {
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
    if THREADS && plan.threads {
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

/// Hand what they took up the offer of to the daemon, which imports it in the background.
#[cfg(feature = "daemon")]
async fn import(offer: &Offer, tx: &Updates) {
    async fn history(shell: &str) -> Result<()> {
        let settings = Settings::new()?;
        let var = |name| var_nonempty(name).map(|v| v.to_string_lossy().into_owned());
        let mut client = super::daemon::ready_client(&settings).await?;
        client.import_history(shell, var("HISTFILE"), var("fish_history")).await
    }

    if let Some((shell, commands)) = offer.history {
        let status = match history(shell).await {
            Ok(()) => {
                Status::Done(format!("Importing {} commands in the background", count(commands)))
            }
            Err(e) => Status::Failed(format!(
                "Couldn't import your shell history ({e:#}). Try `atuin import auto`"
            )),
        };
        send(tx, Step::History, status);
    }
    #[cfg(feature = "ai")]
    if let Some(threads) = offer.threads {
        let status = match import_threads().await {
            Ok(()) if threads == 1 => Status::Done("Importing 1 thread in the background".into()),
            Ok(()) => {
                Status::Done(format!("Importing {} threads in the background", count(threads)))
            }
            Err(e) => Status::Failed(format!(
                "Couldn't import your threads ({e:#}). Try `atuin threads import`"
            )),
        };
        send(tx, Step::Threads, status);
    }
}

#[cfg(all(feature = "ai", feature = "daemon"))]
async fn import_threads() -> Result<()> {
    let settings = Settings::new()?;
    // A daemon already running picks the new config up itself; this starts one that isn't.
    super::daemon::ready_client(&settings).await?;
    let mut client = atuin_daemon::AiClient::from_settings(&settings).await?;
    client.wait_for_sessions(|_| {}).await?;
    client.import_sessions_in_background().await
}

/// Import shell history here, with no daemon to hand it to.
#[cfg(not(feature = "daemon"))]
async fn import(offer: &Offer, tx: &Updates) {
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
/// does.
#[cfg(feature = "daemon")]
async fn sync_soon(settings: &Settings, tx: &Updates) {
    use atuin_daemon::service::ActiveService;

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
async fn sync_soon(_: &Settings, tx: &Updates) {
    send(tx, Step::Sync, Status::Done("Signed in, syncing in the background".into()));
}

async fn write_config(plan: &Plan) -> Result<()> {
    let settings = Settings::new()?;
    let mut changes = Vec::new();

    // A daemon already on may be managed externally (systemd, launchd), so leave its autostart
    // alone.
    if !settings.daemon.enabled {
        changes.push(("daemon.enabled", Value::from(true)));
        changes.push(("daemon.autostart", Value::from(true)));
    }
    if plan.shell {
        changes.push(("search_mode", Value::from("daemon-fuzzy")));
        if plan.ctrl_r != settings.bind_ctrl_r {
            changes.push(("bind_ctrl_r", Value::from(plan.ctrl_r)));
        }
        if plan.up_arrow != settings.bind_up_arrow {
            changes.push(("bind_up_arrow", Value::from(plan.up_arrow)));
        }
    }
    if plan.threads != settings.threads.capture {
        changes.push(("threads.capture", Value::from(plan.threads)));
    }
    match &plan.sync {
        Sync::Server(address) => changes.push(("sync_address", Value::from(address.as_str()))),
        Sync::Cloud if !settings.is_hub_sync() => {
            let cloud = atuin_client::settings::DEFAULT_SYNC_URL.as_str();
            changes.push(("sync_address", Value::from(cloud)));
        }
        Sync::Cloud | Sync::Off => {}
    }
    // Turned on after sign-in, once the hub has said the account has it.
    #[cfg(feature = "octavo")]
    if plan.teams == Some(Teams::Off) && settings.teams.enabled {
        changes.push(("teams.enabled", Value::from(false)));
    }
    update_config_file(changes).await
}

async fn update_config_file(changes: Vec<(&str, Value)>) -> Result<()> {
    let config_file = Settings::get_config_path()?;
    let config = tokio::fs::read_to_string(&config_file).await?;
    let updated = super::config::update_config(&config, changes)?;
    Settings::validate_str(&updated)
        .map_err(|e| eyre::eyre!("cannot update config: it would be invalid\n\n{e}"))?;
    tokio::fs::write(config_file, updated).await?;
    Ok(())
}

fn hook_shells(found: &Found, tx: &Updates) {
    if found.hooked {
        send(tx, Step::Shells, Status::Done("Your shell already runs Atuin".into()));
        return;
    }
    let home = home_dir();
    let zdotdir = var_nonempty("ZDOTDIR").map_or_else(|| home.clone(), PathBuf::from);
    let status = match install_shell_init(&home, &zdotdir) {
        Ok(shells) => Status::Done(format!("Hooked into {}", names(&shells))),
        Err(e) => Status::Failed(format!("Couldn't hook into your shell: {e}")),
    };
    send(tx, Step::Shells, status);
}

async fn hook_agents(agents: &[AnyHarness], tx: &Updates) {
    send(tx, Step::AgentHooks, Status::Running("Recording what your agents run".into()));
    let mut hooked = Vec::new();
    let mut failed = Vec::new();
    for agent in agents {
        match agent.install_hooks().await {
            Ok(_) | Err(InstallHookError::AlreadyInstalled) => hooked.push(agent.display_name()),
            Err(e) => failed.push(format!("{} ({e})", agent.display_name())),
        }
    }
    let done = (!hooked.is_empty()).then(|| format!("Recording commands from {}", names(&hooked)));
    send(tx, Step::AgentHooks, partial(done, &failed, "hook into"));
}

async fn connect_agents(agents: &[AnyHarness], tx: &Updates) {
    let now = SystemTime::now();
    let pool = BlockingPool::new(NonZeroUsize::MIN);
    let mut connected = Vec::new();
    let mut failed = Vec::new();
    let mut connect = Vec::new();
    for &agent in agents {
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
    if connect.is_empty() && failed.is_empty() {
        return;
    }
    send(tx, Step::Mcp, Status::Running("Letting your agents search Atuin".into()));
    for agent in &connect {
        // Its own process, as the agents' own `mcp add` commands print what they did.
        match quietly(&["mcp", "install", agent.name()]).await {
            Ok(()) => connected.push(agent.display_name()),
            Err(e) => failed.push(format!("{} ({e})", agent.display_name())),
        }
    }
    let done = (!connected.is_empty()).then(|| format!("{} can search Atuin", names(&connected)));
    send(tx, Step::Mcp, partial(done, &failed, "connect"));
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
const STILL_USED: Duration = Duration::from_hours(14 * 24);

/// What setup does about connecting an agent to Atuin's MCP server.
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

/// What worked, then what didn't.
fn partial(done: Option<String>, failed: &[String], verb: &str) -> Status {
    match (done, failed) {
        (Some(done), []) => Status::Done(done),
        (Some(done), failed) => {
            Status::Failed(format!("{done}; couldn't {verb} {}", failed.join(", ")))
        }
        (None, failed) => Status::Failed(format!("Couldn't {verb} {}", failed.join(", "))),
    }
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
    use atuin_client::import::zsh::Zsh;

    async fn entries<I: Importer>() -> usize {
        match I::new().await {
            Ok(mut importer) => importer.entries().await.unwrap_or(0),
            Err(_) => 0,
        }
    }

    let shell = var_nonempty("SHELL")?;
    let shell = shell.to_string_lossy();
    if shell.ends_with("/zsh") {
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
async fn threads_here(settings: &Settings) -> usize {
    use atuin_client::ai_session::{AiSessionDatabase, SessionFilter, sidecar_path};

    match AiSessionDatabase::open_read_only(sidecar_path(settings)).await {
        Ok(db) => db.list_sessions(&SessionFilter::default()).await.map_or(0, |s| s.len()),
        Err(_) => 0,
    }
}

/// The threads setup just imported, newest first, for the summary.
#[cfg(all(feature = "ai", feature = "daemon"))]
async fn latest_threads(settings: &Settings) -> Vec<atuin_ai::commands::Recent> {
    atuin_ai::commands::recent(settings, 3).await.unwrap_or_default()
}

/// What setup did, and what to try next, left in the scrollback.
async fn print_summary(plan: &Plan, found: &Found, steps: &[(Step, Status)]) {
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
    if plan.threads
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
    if plan.shell && plan.ctrl_r {
        tips.push(("ctrl-r", "search your shell history"));
    }
    if plan.shell && plan.up_arrow {
        tips.push(("up arrow", "search it from what you've typed"));
    }
    if plan.threads {
        tips.push(("atuin resume", "pick a thread back up, in any agent"));
    }
    if plan.shell {
        tips.push(("?", "ask Atuin AI for a command"));
    }
    if !tips.is_empty() {
        println!();
        // A shell picks up its key bindings as it starts.
        let keys_changed = plan.ctrl_r != found.ctrl_r || plan.up_arrow != found.up_arrow;
        if plan.shell && (!found.hooked || keys_changed) {
            println!("  {}", "Open a new terminal, then try".bold());
        } else {
            println!("  {}", "Try".bold());
        }
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

/// Add `atuin init` to the rc files of zsh and bash, and of fish where it's set up. Returns the
/// shells hooked.
fn install_shell_init(home: &Path, zdotdir: &Path) -> io::Result<Vec<&'static str>> {
    let mut shells = vec!["zsh", "bash"];

    append_once(&zdotdir.join(".zshrc"), "atuin init zsh", "\neval \"$(atuin init zsh)\"\n")?;

    // A `.bashrc` made here would be the only one bash reads, so it needs the installer's PATH.
    let bashrc = home.join(".bashrc");
    let env = home.join(".atuin").joined("bin").joined("env");
    let init = if !bashrc.exists() && env.exists() {
        ". \"$HOME/.atuin/bin/env\"\neval \"$(atuin init bash)\"\n"
    } else {
        "\neval \"$(atuin init bash)\"\n"
    };
    append_once(&bashrc, "atuin init bash", init)?;

    let fish = home.join(".config").joined("fish").joined("config.fish");
    if fish.exists() {
        append_once(
            &fish,
            "atuin init fish",
            "\nif status is-interactive\n    atuin init fish | source\nend\n",
        )?;
        shells.push("fish");
    }

    Ok(shells)
}

fn append_once(path: &Path, marker: &str, text: &str) -> io::Result<()> {
    match std::fs::read_to_string(path) {
        Ok(existing) if existing.contains(marker) => return Ok(()),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    OpenOptions::new().create(true).append(true).open(path)?.write_all(text.as_bytes())
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
    #[case(Some("did it"), &[], Status::Done("did it".into()))]
    #[case(Some("did it"), &["pi (no)"], Status::Failed("did it; couldn't hook pi (no)".into()))]
    #[case(None, &["pi (no)"], Status::Failed("Couldn't hook pi (no)".into()))]
    fn partial_says_what_failed(
        #[case] done: Option<&str>,
        #[case] failed: &[&str],
        #[case] expected: Status,
    ) {
        let failed: Vec<String> = failed.iter().map(ToString::to_string).collect();
        assert_eq!(partial(done.map(Into::into), &failed, "hook"), expected);
    }

    #[fixture]
    fn found() -> Found {
        Found {
            history: Some(("zsh", 48210)),
            imported: 0,
            threads_here: 0,
            agents: vec![(AnyHarness::ClaudeCode(atuin_common::harnesstools::ccode::Ccode), 353)],
            hooked: false,
            ctrl_r: true,
            up_arrow: true,
            capture: false,
            signed_in: false,
            server: None,
        }
    }

    /// What `offer` offers, and what it says about the rest.
    fn offered(found: &Found, account: Account) -> (Offer, Vec<(Step, Status)>) {
        let plan = Plan {
            shell: true,
            threads: true,
            agent_history: true,
            ctrl_r: true,
            up_arrow: true,
            mcp: true,
            sync: Sync::Off,
            teams: None,
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
        let flags = ["setup", "--no-agent-history", "--no-up-arrow", "--no-mcp", "--sync", "off"];
        assert_eq!(Cmd::parse_from(flags).plan(&found, false), Plan {
            shell: true,
            threads: THREADS,
            agent_history: false,
            ctrl_r: true,
            up_arrow: false,
            mcp: false,
            sync: Sync::Off,
            teams: None,
        });
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
        assert_eq!(Cmd::parse_from(["setup"]).plan(&found, yes).sync, expected);
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
        assert_eq!(Cmd::parse_from(["setup"]).plan(&found, yes).sync, expected);
    }

    #[rstest]
    #[case::found_and_kept(true, &[], true)]
    #[case::found_and_turned_off(true, &["--no-threads"], false)]
    #[case::none_found_and_left(false, &["--no-threads"], true)]
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
        let args = std::iter::once("setup").chain(flags.iter().copied());
        // Without them, there are no agents to ask about.
        assert_eq!(Cmd::parse_from(args).plan(&found, false).threads, expected || !THREADS);
    }

    #[rstest]
    #[case::not_asked(&[], None)]
    #[case::asked(&["--teams"], Some(Teams::On { backfill: false }))]
    #[case::without_sync(&["--teams", "--sync", "off"], Some(Teams::Off))]
    fn teams_is_asked_about_with_the_flag(
        mut found: Found,
        #[case] flags: &[&str],
        #[case] expected: Option<Teams>,
    ) {
        found.signed_in = true;
        let args = std::iter::once("setup").chain(flags.iter().copied());
        assert_eq!(Cmd::parse_from(args).plan(&found, false).teams, expected);
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

        install_shell_init(home.path(), home.path()).unwrap();
        install_shell_init(home.path(), home.path()).unwrap();

        let bashrc = std::fs::read_to_string(home.path().join(".bashrc")).unwrap();
        assert_eq!(bashrc, ". \"$HOME/.atuin/bin/env\"\neval \"$(atuin init bash)\"\n");
        let zshrc = std::fs::read_to_string(home.path().join(".zshrc")).unwrap();
        assert_eq!(zshrc.matches("atuin init zsh").count(), 1);
    }
}
