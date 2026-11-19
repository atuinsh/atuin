//! What setup sets up: the choices on its first screen and their flags, and how they depend on
//! each other.

use atuin_client::settings::ThreadUploads;
use clap::{Arg, ArgAction, ArgMatches, Command, FromArgMatches};

use super::super::init::Shell;
use super::agents::{AnyHarness, Harness};
use super::form::{Choice, Flag, Form, Needs, Visit, toggles};
use super::{AGENTS, Found, SYNC, Sync, TEAMS, THREADS, Teams, parse_sync};

/// What setup asks about here, besides what this atuin was built with. What it doesn't ask
/// about, it leaves as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "what setup asks about, each yes or no")]
pub(super) struct Asks {
    /// Agents' threads, commands and search, when there are agents here to ask about or the
    /// flags name some.
    pub(super) agents: bool,
    /// Atuin for Teams, with `--hub` or `--no-hub`.
    pub(super) teams: bool,
    /// Whether Atuin holds commands or threads here already, which agents can search without
    /// capturing more.
    pub(super) searchable: bool,
    /// Whether `--only --no-shell-history` takes shell history out, of the shells named beside it.
    pub(super) unhooks: bool,
}

/// What to set up, as chosen on the first screen and by the flags. Setup sets up its
/// [`Form::effective`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Plan {
    pub(super) asks: Asks,
    pub(super) shell: ShellHistory,
    pub(super) shells: Shells,
    pub(super) output: OutputCapture,
    pub(super) threads: Threads,
    pub(super) agent_history: AgentHistory,
    pub(super) agents: Agents,
    pub(super) ctrl_r: CtrlR,
    pub(super) up_arrow: UpArrow,
    pub(super) mcp: Mcp,
    pub(super) daemon: Daemon,
    pub(super) sync: Sync,
    pub(super) atuin_for_teams: AtuinForTeams,
    pub(super) upload_threads: UploadThreads,
    pub(super) backfill: Backfill,
}

toggles! {
    /// Capturing shell history, which hooks the shells setup knows.
    ShellHistory("shell-history", "Capture") {
        on: "Capture your shell history",
        off: "Don't capture your shell history",
    }
    /// Capturing what commands print, through the pty proxy that `atuin init` starts.
    OutputCapture("output-capture", "Capture") {
        on: "Capture what your commands print",
        off: "Don't capture what your commands print",
    }
    /// Capturing the agents' threads.
    Threads("threads", "Capture") {
        on: "Capture your conversations with your agents",
        off: "Don't capture your conversations with your agents",
    }
    /// Recording the commands the agents run, through their hooks.
    AgentHistory("agent-history", "Capture") {
        on: "Capture the commands your agents run",
        off: "Don't capture the commands your agents run",
    }
    CtrlR("ctrl-r", "Search") {
        on: "Search with ctrl-r",
        off: "Leave ctrl-r to your shell",
    }
    UpArrow("up-arrow", "Search") {
        on: "Search with the up arrow",
        off: "Leave the up arrow to your shell",
    }
    /// Letting the agents search Atuin over MCP.
    Mcp("mcp", "Search") {
        on: "Let your agents search Atuin over MCP",
        off: "Don't let your agents search Atuin over MCP",
    }
    /// Running the daemon, which searches and captures in the background.
    Daemon("daemon") {
        on: "Run the daemon, which thread and output capture need",
        off: "Stop the daemon, and keep it off",
    }
    // Hidden until Atuin for Teams ships. 18.24.8 shipped it as `--teams`.
    AtuinForTeams("hub", "Sync"; hidden = true; alias = "teams") {
        on: "Turn on Atuin for Teams",
        off: "Turn off Atuin for Teams",
    }
    /// Atuin for Teams uploading what Atuin has already, as well as what it records from now on.
    Backfill("backfill", "Sync"; hidden = true) {
        on: "Upload what Atuin has already to Atuin for Teams",
        off: "Upload only what Atuin records from now on",
    }
}

impl Flag for Sync {
    fn args(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("sync")
                .long("sync")
                .value_name("cloud|off|URL")
                .value_parser(parse_sync)
                .overrides_with("sync")
                .help("Where to sync: `cloud`, `off`, or your own server's address")
                .help_heading("Sync"),
        )
    }

    fn from_args(matches: &ArgMatches) -> Option<Self> {
        matches.get_one::<Self>("sync").cloned()
    }

    fn flag(&self) -> String {
        match self {
            Self::Cloud => "--sync cloud".to_owned(),
            Self::Server(address) => format!("--sync {address}"),
            Self::Off => "--sync off".to_owned(),
        }
    }
}

/// Which agent threads Atuin for Teams uploads while it's on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct UploadThreads(pub(super) ThreadUploads);

fn parse_thread_uploads(value: &str) -> Result<ThreadUploads, String> {
    [ThreadUploads::All, ThreadUploads::Workspaces, ThreadUploads::Off]
        .into_iter()
        .find(|uploads| uploads.as_str() == value)
        .ok_or_else(|| "expected all, workspaces or off".to_owned())
}

impl Flag for UploadThreads {
    fn args(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("upload-threads")
                .long("upload-threads")
                .value_name("all|workspaces|off")
                .value_parser(parse_thread_uploads)
                .overrides_with("upload-threads")
                // Hidden until Atuin for Teams ships.
                .hide(true)
                .help(
                    "Which agent threads Atuin for Teams uploads: all, those started in a \
                     workspace, or none",
                )
                .help_heading("Sync"),
        )
    }

    fn from_args(matches: &ArgMatches) -> Option<Self> {
        matches.get_one::<ThreadUploads>("upload-threads").copied().map(Self)
    }

    fn flag(&self) -> String {
        format!("--upload-threads {}", self.0.as_str())
    }
}

/// The shells setup hooks into: those `atuin init` knows that setup can write the rc file of.
pub(super) const HOOKABLE: [Shell; 4] = [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::PowerShell];

/// The shells setup hooks into, of [`HOOKABLE`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Shells(pub(super) Vec<Shell>);

/// The agents setup hooks into and connects to Atuin's MCP server.
#[derive(Clone, Debug, Default)]
pub(super) struct Agents(pub(super) Vec<AnyHarness>);

impl PartialEq for Agents {
    fn eq(&self, other: &Self) -> bool {
        self.0.iter().map(Harness::name).eq(other.0.iter().map(Harness::name))
    }
}

impl Eq for Agents {}

fn parse_shell(name: &str) -> Result<Shell, String> {
    HOOKABLE.into_iter().find(|shell| shell.name() == name).ok_or_else(|| {
        let names: Vec<_> = HOOKABLE.iter().map(|shell| shell.name()).collect();
        format!("setup hooks into {}", names.join(", "))
    })
}

fn parse_agent(name: &str) -> Result<AnyHarness, String> {
    AnyHarness::from_name(name).map_err(|e| e.to_string())
}

impl Flag for Shells {
    fn args(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("shells")
                .long("shells")
                .value_name("SHELL")
                .value_delimiter(',')
                .action(ArgAction::Append)
                .value_parser(parse_shell)
                .help("The shells to hook into: zsh, bash, fish and powershell")
                .help_heading("Capture"),
        )
    }

    fn from_args(matches: &ArgMatches) -> Option<Self> {
        matches.get_many::<Shell>("shells").map(|shells| Self(shells.copied().collect()))
    }

    fn flag(&self) -> String {
        let names: Vec<_> = self.0.iter().map(|shell| shell.name()).collect();
        format!("--shells {}", names.join(","))
    }
}

impl Flag for Agents {
    fn args(cmd: Command) -> Command {
        let names: Vec<_> = AnyHarness::all().iter().map(Harness::name).collect();
        cmd.arg(
            Arg::new("agents")
                .long("agents")
                .value_name("AGENT")
                .value_delimiter(',')
                .action(ArgAction::Append)
                .value_parser(parse_agent)
                .help(format!("The agents to hook into and connect, of {}", names.join(", ")))
                .help_heading("Capture"),
        )
    }

    fn from_args(matches: &ArgMatches) -> Option<Self> {
        matches.get_many::<AnyHarness>("agents").map(|agents| Self(agents.copied().collect()))
    }

    fn flag(&self) -> String {
        let names: Vec<_> = self.0.iter().map(Harness::name).collect();
        format!("--agents {}", names.join(","))
    }
}

impl Form for Plan {
    fn visit(&mut self, visit: &mut impl Visit<Self>) {
        visit.field(&mut self.shell);
        visit.field(&mut self.shells);
        visit.field(&mut self.output);
        visit.field(&mut self.threads);
        visit.field(&mut self.agent_history);
        visit.field(&mut self.agents);
        visit.field(&mut self.ctrl_r);
        visit.field(&mut self.up_arrow);
        visit.field(&mut self.mcp);
        visit.field(&mut self.daemon);
        visit.field(&mut self.sync);
        visit.field(&mut self.atuin_for_teams);
        visit.field(&mut self.upload_threads);
        visit.field(&mut self.backfill);
    }
}

/// Whether setup asks about the agents here.
const fn agents(plan: &Plan) -> Result<(), &'static str> {
    if !AGENTS {
        Err("this atuin was built without agent support")
    } else if plan.asks.agents {
        Ok(())
    } else {
        Err("there's no coding agent on this machine")
    }
}

const fn needs(on: bool, needs: Needs) -> Result<(), Needs> {
    if on {
        Ok(())
    } else {
        Err(needs)
    }
}

const SHELL_HISTORY: Needs = Needs {
    what: "shell history",
    flags: "`--shell-history`",
};

const DAEMON: Needs = Needs {
    what: "the daemon",
    flags: "`--daemon`",
};

const SYNCING: Needs = Needs {
    what: "sync",
    flags: "`--sync cloud`, or your own server's address",
};

const CONNECTED_AGENT: Needs = Needs {
    what: "a connected agent",
    flags: "`--agents`",
};

const SOMETHING_TO_SEARCH: Needs = Needs {
    what: "history or threads to search",
    flags: "`--shell-history`, `--agent-history` or `--threads`",
};

const TEAMS_ON: Needs = Needs {
    what: "Atuin for Teams",
    flags: "`--hub`",
};

const BEYOND_WORKSPACES: Needs = Needs {
    what: "threads from outside workspaces",
    flags: "`--upload-threads all`",
};

impl Choice<Plan> for ShellHistory {}

impl Choice<Plan> for OutputCapture {
    fn asked(_: &Plan) -> Result<(), &'static str> {
        if cfg!(all(feature = "pty-proxy", feature = "daemon", unix)) {
            Ok(())
        } else {
            Err("this atuin was built without output capture")
        }
    }

    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(plan.shell.0, SHELL_HISTORY)?;
        needs(plan.daemon.0, DAEMON)
    }
}

impl Choice<Plan> for Threads {
    fn asked(plan: &Plan) -> Result<(), &'static str> {
        if !THREADS {
            return Err("this atuin was built without the daemon that captures threads");
        }
        agents(plan)
    }

    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(plan.daemon.0, DAEMON)
    }
}

/// The agents' commands come from the hooks of those connected.
impl Choice<Plan> for AgentHistory {
    fn asked(plan: &Plan) -> Result<(), &'static str> {
        agents(plan)
    }

    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(!plan.agents.0.is_empty(), CONNECTED_AGENT)
    }
}

/// The keys search shell history.
impl Choice<Plan> for CtrlR {
    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(plan.shell.0, SHELL_HISTORY)
    }
}

impl Choice<Plan> for UpArrow {
    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(plan.shell.0, SHELL_HISTORY)
    }
}

/// The connected agents search what Atuin holds or captures, so there has to be some.
impl Choice<Plan> for Mcp {
    fn asked(plan: &Plan) -> Result<(), &'static str> {
        agents(plan)
    }

    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(!plan.agents.0.is_empty(), CONNECTED_AGENT)?;
        let captures = plan.shell.0 || plan.agent_history.0 || plan.threads.0;
        needs(captures || plan.asks.searchable, SOMETHING_TO_SEARCH)
    }
}

impl Choice<Plan> for Shells {
    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(plan.shell.0 || plan.asks.unhooks, SHELL_HISTORY)
    }
}

/// The agents setup connects: it hooks into each and connects it to Atuin's MCP server. Thread
/// capture is one for every agent.
impl Choice<Plan> for Agents {
    fn asked(plan: &Plan) -> Result<(), &'static str> {
        agents(plan)
    }
}

impl Choice<Plan> for Daemon {
    fn asked(_: &Plan) -> Result<(), &'static str> {
        if cfg!(feature = "daemon") {
            Ok(())
        } else {
            Err("this atuin was built without the daemon")
        }
    }
}

impl Choice<Plan> for Sync {
    fn asked(_: &Plan) -> Result<(), &'static str> {
        if SYNC {
            Ok(())
        } else {
            Err("this atuin was built without sync")
        }
    }
}

/// Atuin for Teams uploads as the account sync signs in to, from the daemon.
impl Choice<Plan> for AtuinForTeams {
    fn asked(plan: &Plan) -> Result<(), &'static str> {
        if !TEAMS {
            Err("this atuin was built without Atuin for Teams")
        } else if plan.asks.teams {
            Ok(())
        } else {
            Err("setup asks about Atuin for Teams only with `--hub`")
        }
    }

    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(plan.sync != Sync::Off, SYNCING)?;
        needs(plan.daemon.0, DAEMON)
    }
}

impl Choice<Plan> for UploadThreads {
    fn asked(plan: &Plan) -> Result<(), &'static str> {
        AtuinForTeams::asked(plan)
    }

    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(plan.atuin_for_teams.0, TEAMS_ON)
    }
}

/// Workspace-only uploads take nothing from before, which can't be told to come from a workspace.
impl Choice<Plan> for Backfill {
    fn asked(plan: &Plan) -> Result<(), &'static str> {
        AtuinForTeams::asked(plan)
    }

    fn allowed(plan: &Plan) -> Result<(), Needs> {
        needs(plan.atuin_for_teams.0, TEAMS_ON)?;
        needs(plan.upload_threads.0 != ThreadUploads::Workspaces, BEYOND_WORKSPACES)
    }
}

impl Plan {
    /// What's set up here now, as far as `found` says, asking what `asks` says: the shells that
    /// load Atuin, and the agents it's in. Where none does, or it's in none, those here.
    pub(super) fn from_current_state(found: &Found, asks: Asks) -> Self {
        let shells = if found.inits.is_empty() {
            found.shells.clone()
        } else {
            found.inits.clone()
        };
        let connected = found.connected();
        let agents = if connected.is_empty() {
            found.agents()
        } else {
            connected
        };
        Self {
            asks,
            shell: ShellHistory(found.hooked || !found.inits.is_empty()),
            shells: Shells(shells),
            output: OutputCapture(found.output),
            threads: Threads(found.capture),
            agent_history: AgentHistory(!found.hooked_agents().is_empty()),
            agents: Agents(agents),
            ctrl_r: CtrlR(found.ctrl_r),
            up_arrow: UpArrow(found.up_arrow),
            mcp: Mcp(!found.mcp_agents().is_empty()),
            daemon: Daemon(found.daemon),
            sync: found.syncing().unwrap_or_default(),
            atuin_for_teams: AtuinForTeams(found.teams),
            upload_threads: UploadThreads(found.upload_threads),
            backfill: Backfill::default(),
        }
    }

    /// What setup sets up where no flag says otherwise, from what's set up here: all it asks
    /// about but output capture, the daemon unless they turned it off, the keys as they're bound,
    /// and sync where it's signed in. Once
    /// Atuin is in an agent, the agents' choices stay as they are, but for its MCP server, which
    /// comes with every agent connected. `yes` when nobody's there to sign in.
    #[must_use]
    pub(super) fn proposal(self, found: &Found, yes: bool) -> Self {
        let agents = agents(&self).is_ok();
        let fresh = agents && found.connections.is_empty();
        let sync = match found.syncing() {
            // Left as it is, rather than signing them out.
            Some(syncing) => syncing,
            None if yes || !SYNC => Sync::Off,
            None => found.server.clone().map_or(Sync::Cloud, Sync::Server),
        };
        Self {
            shell: ShellHistory(true),
            threads: if fresh {
                Threads(true)
            } else {
                self.threads
            },
            agent_history: if fresh {
                AgentHistory(true)
            } else {
                self.agent_history
            },
            mcp: Mcp(agents),
            daemon: Daemon(!found.daemon_off),
            sync,
            atuin_for_teams: AtuinForTeams(self.asks.teams),
            ..self
        }
    }

    /// What setup does about Atuin for Teams, `None` when it leaves it as it is.
    pub(super) fn teams(&self) -> Option<Teams> {
        AtuinForTeams::asked(self).ok()?;
        Some(if self.atuin_for_teams.0 {
            Teams::On {
                backfill: self.backfill.0,
            }
        } else {
            Teams::Off
        })
    }

    /// Whether setup waits for its imports: turning Atuin for Teams on restarts the daemon, which
    /// would stop one going in the background, and its upload takes in only what's imported.
    pub(super) fn waits_for_imports(&self) -> bool {
        matches!(self.teams(), Some(Teams::On { .. }))
    }
}

/// The flags of the plan's choices, which [`Form::set_from`] reads.
#[derive(Clone, Debug)]
pub(super) struct Choices(ArgMatches);

impl Choices {
    pub(super) const fn matches(&self) -> &ArgMatches {
        &self.0
    }
}

impl FromArgMatches for Choices {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self(matches.clone()))
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        self.0 = matches.clone();
        Ok(())
    }
}

impl clap::Args for Choices {
    fn augment_args(cmd: Command) -> Command {
        Plan::args(cmd)
    }

    fn augment_args_for_update(cmd: Command) -> Command {
        Plan::args(cmd)
    }
}
