use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_client::theme;
use atuin_common::logs::{self, LogMode};
use clap::Subcommand;
use clap_couture::Couture;
use eyre::{Result, WrapErr};

use crate::i18n::fl;
use crate::logs::LogCtx;

#[cfg(feature = "sync")]
mod sync;

#[cfg(feature = "sync")]
mod account;

#[cfg(feature = "daemon")]
mod daemon;

#[cfg(all(feature = "ai", feature = "daemon"))]
mod ai_sessions;

mod config;
mod default_config;
mod doctor;
mod dotfiles;
mod history;
mod hook;
mod import;
mod info;
mod init;
mod internal;
mod kv;
#[cfg(feature = "ai")]
mod mcp;
#[cfg(feature = "daemon")]
mod output;
mod scripts;
mod search;
mod setup;
mod stats;
mod store;
#[cfg(feature = "self-update")]
mod update;
mod wrapped;

#[derive(Subcommand, Debug, Couture)]
#[command(infer_subcommands = true)]
#[couture(inherit = ["ai", "sync", "data", "setup", "advanced"])]
pub enum Cmd {
    // Variant order sets the `--help` command list order, so keep the commands users reach for
    // most (search, sync, stats) at the top and the plumbing/config commands lower down.
    #[command(about = fl!("cmd-search"))]
    Search(search::Cmd),

    /// Work with captured command output
    #[cfg(feature = "daemon")]
    #[command(subcommand)]
    Output(output::Cmd),

    #[command(about = fl!("cmd-stats"))]
    Stats(stats::Cmd),

    #[cfg(feature = "sync")]
    #[command(flatten)]
    Sync(sync::Cmd),

    #[command(about = fl!("cmd-account"))]
    #[cfg(feature = "sync")]
    #[couture(category = "sync")]
    Account(account::Cmd),

    /// Manipulate shell history
    #[command(subcommand)]
    History(history::Cmd),

    #[command(about = fl!("cmd-setup"))]
    #[couture(category = "setup")]
    Setup,

    #[command(about = fl!("cmd-init"))]
    #[couture(category = "setup")]
    Init(init::Cmd),

    /// Import shell history from file
    #[command(subcommand)]
    #[couture(category = "setup")]
    Import(import::Cmd),

    #[command(about = fl!("cmd-doctor"))]
    #[couture(category = "setup")]
    Doctor,

    #[cfg(feature = "self-update")]
    #[command(about = fl!("cmd-update"))]
    #[couture(category = "setup")]
    Update(update::Cmd),

    /// Get or set small key-value pairs
    #[command(subcommand)]
    #[couture(category = "data")]
    Kv(kv::Cmd),

    /// Manage the atuin data store
    #[command(subcommand)]
    #[couture(category = "data")]
    Store(store::Cmd),

    /// List legacy synced dotfiles data
    #[command(subcommand)]
    #[couture(category = "data")]
    Dotfiles(dotfiles::Cmd),

    /// Manage your scripts with Atuin
    #[command(subcommand)]
    #[couture(category = "data")]
    Scripts(scripts::Cmd),

    #[command(about = fl!("cmd-hook"))]
    #[couture(category = "advanced")]
    Hook(hook::Cmd),

    /// Run the AI assistant
    #[cfg(feature = "ai")]
    #[command(subcommand)]
    #[couture(category = "ai")]
    Ai(atuin_ai::commands::Command),

    /// Search, read and resume your coding agents' captured threads
    #[cfg(feature = "ai")]
    #[command(subcommand)]
    #[couture(category = "ai")]
    Threads(atuin_ai::commands::ThreadsCmd),

    /// Pick a captured coding-agent thread and resume it (short for `atuin threads resume`)
    #[cfg(feature = "ai")]
    #[couture(category = "ai")]
    Resume(atuin_ai::commands::ResumeCmd),

    #[cfg(feature = "ai")]
    #[command(about = fl!("cmd-mcp"))]
    #[couture(category = "ai")]
    Mcp(mcp::Cmd),

    #[command(about = fl!("cmd-wrapped"))]
    Wrapped {
        #[arg(help = fl!("arg-wrapped-year"))]
        year: Option<i32>,
    },

    #[command(about = fl!("cmd-default-config"))]
    #[couture(category = "advanced")]
    DefaultConfig,

    /// Get, set, or print values in your atuin config file
    #[command(subcommand)]
    #[couture(category = "setup")]
    Config(config::Cmd),

    #[command(about = fl!("cmd-info"))]
    #[couture(category = "advanced")]
    Info,

    #[cfg(feature = "daemon")]
    #[command(about = fl!("cmd-daemon"))]
    #[couture(category = "advanced")]
    Daemon(daemon::Cmd),

    /// Internal subcommands, not for direct use by users.
    #[command(
        subcommand,
        hide = true,
        name = "__internal",
        help_template = "error: this command is not meant to be accessed directly",
        disable_help_flag = true,
        disable_help_subcommand = true
    )]
    Internal(internal::Cmd),

    #[command(
        hide = true,
        name = "__internal_",
        disable_help_flag = true,
        disable_help_subcommand = true,
        about = fl!("cmd-__internal_")
    )]
    InternalDecoy,
}

impl Cmd {
    pub fn run(self) -> Result<()> {
        // `atuin threads` and `atuin resume` are `atuin ai`'s, so they run exactly as those do, and
        // `atuin threads enable` is `atuin config enable threads`.
        #[cfg(feature = "ai")]
        match self {
            #[cfg(feature = "daemon")]
            Self::Threads(atuin_ai::commands::ThreadsCmd::Enable)
            | Self::Ai(atuin_ai::commands::Command::Threads(
                atuin_ai::commands::ThreadsCmd::Enable,
            )) => {
                let feature = config::Feature::Threads;
                return Self::Config(config::Cmd::Enable(config::EnableCmd { feature })).run();
            }
            Self::Threads(cmd) => return Self::Ai(cmd.into()).run(),
            Self::Resume(cmd) => return Self::Ai(atuin_ai::commands::Command::Resume(cmd)).run(),
            _ => {}
        }

        // Daemonize before creating the async runtime – fork() inside a live
        // tokio runtime corrupts its internal state.
        #[cfg(feature = "daemon")]
        if let Self::Daemon(ref cmd) = self
            && cmd.should_daemonize()
        {
            atuin_daemon::service::Subproc::daemonize()?;
        }

        #[cfg(feature = "ai")]
        let use_multi_thread_runtime = matches!(&self, Self::Ai(_));
        #[cfg(not(feature = "ai"))]
        let use_multi_thread_runtime = false;

        let Some(future) = self.run_inner()? else {
            // The command was handled synchronously; return.
            return Ok(());
        };

        let runtime = if use_multi_thread_runtime {
            tokio::runtime::Builder::new_multi_thread()
        } else {
            tokio::runtime::Builder::new_current_thread()
        }
        .enable_all()
        .build()
        .unwrap();

        let res = runtime.block_on(future);
        runtime.shutdown_timeout(std::time::Duration::from_millis(50));
        res
    }

    /// Run the command, returning a future for commands that require async.
    ///
    /// If the command was able to be handled synchronously, returns `Ok(None)`. Otherwise, returns
    /// `Ok(Some(future))` where `future` will run the command when awaited. Returns `Err` on error.
    fn run_inner(self) -> Result<Option<impl Future<Output = Result<()>>>> {
        let run_internal = if let Self::Internal(cmd) = &self {
            match cmd.run() {
                Some(func) => Some(func),
                None => return Ok(None),
            }
        } else {
            None
        };

        // Logging doesn't read the settings, so it is up before loading them can warn.
        let logging = self
            .log_mode()
            .map(|mode| LogCtx::try_enable("atuin", mode))
            .transpose()
            .wrap_err("failed to enable logging")?;
        let settings = Settings::new().wrap_err("could not load client settings")?;

        Ok(Some(async move {
            let result = if let Some(func) = run_internal {
                func(settings).await
            } else {
                Box::pin(self.run_async(settings)).await
            };
            // Dropping the guard flushes and shuts down the OTEL exporter, so it must outlive the
            // command rather than this function.
            #[allow(
                clippy::drop_non_drop,
                reason = "LogCtx has drop glue only with OTEL compiled in"
            )]
            drop(logging);
            result
        }))
    }

    #[allow(clippy::too_many_lines)]
    // `atuin_ai::commands::run` is not `Send` because `eye_declare` holds a `StdoutLock` across
    // await points.
    #[allow(clippy::future_not_send)]
    async fn run_async(self, mut settings: Settings) -> Result<()> {
        tracing::trace!(command = ?self, "client command");
        let mut theme_manager = theme::ThemeManager::from_settings(&settings.theme);

        // Skip initializing any databases for history
        // This is a pretty hot path, as it runs before and after every single command the user
        // runs
        match self {
            Self::History(history) => return history.run(&Arc::new(settings)).await,
            Self::Hook(hook) => return hook.run(&Arc::new(settings)).await,
            Self::Init(init) => return init.run(&settings).await,
            Self::Doctor => return doctor::run(&settings).await,
            #[cfg(feature = "self-update")]
            Self::Update(update) => return update.run(&settings).await,
            Self::Config(config) => return config.run(&Arc::new(settings)).await,
            // Opens the databases itself, once no other daemon can be using them.
            #[cfg(feature = "daemon")]
            Self::Daemon(cmd) => return cmd.run(Arc::new(settings)).await,
            Self::InternalDecoy => {
                tracing::error!(
                    target: logs::USER,
                    "this command is not meant to be accessed directly"
                );
                std::process::exit(1);
            }
            _ => {}
        }

        let db_path = &settings.db_path;
        let record_store_path = &settings.record_store_path;

        let db = Sqlite::new(db_path, settings.local_timeout).await?;
        let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

        let theme_name = settings.theme.name.clone();
        let theme = theme_manager.load_theme(theme_name.as_str(), settings.theme.max_depth);

        match self {
            Self::Setup => setup::run(&settings).await,
            Self::Import(import) => import.run(&db).await,
            Self::Stats(stats) => stats.run(&db, &settings, theme).await,
            Self::Search(search) => search.run(db, settings, sqlite_store, theme).await,

            #[cfg(feature = "daemon")]
            Self::Output(cmd) => cmd.run(&db, &settings, theme).await,

            #[cfg(feature = "sync")]
            Self::Sync(sync) => sync.run(&Arc::new(settings), &db, sqlite_store).await,

            #[cfg(feature = "sync")]
            Self::Account(account) => account.run(&Arc::new(settings), sqlite_store).await,

            Self::Kv(kv) => kv.run(&settings, &sqlite_store).await,

            Self::Store(store) => store.run(&Arc::new(settings), &db, sqlite_store).await,

            Self::Dotfiles(dotfiles) => dotfiles.run(&settings, sqlite_store).await,

            Self::Scripts(scripts) => scripts.run(&settings, sqlite_store, &db).await,

            Self::Info => {
                info::run(&settings);
                Ok(())
            }

            Self::DefaultConfig => {
                default_config::run();
                Ok(())
            }

            Self::Wrapped { year } => wrapped::run(year, &db, &settings, sqlite_store, theme).await,

            Self::History(_)
            | Self::Hook(_)
            | Self::Init(_)
            | Self::Doctor
            | Self::Config(_)
            | Self::Internal(_)
            | Self::InternalDecoy => {
                unreachable!()
            }

            #[cfg(feature = "self-update")]
            Self::Update(_) => unreachable!(),

            #[cfg(feature = "daemon")]
            Self::Daemon(_) => unreachable!(),

            #[cfg(feature = "ai")]
            Self::Threads(_) | Self::Resume(_) => unreachable!(),

            #[cfg(feature = "ai")]
            Self::Ai(cli) => {
                // Resume is what threads are for, so while capture is off it offers to turn it
                // on, every time.
                #[cfg(feature = "daemon")]
                if let atuin_ai::commands::Command::Resume(cmd) = &cli
                    && !settings.threads.capture
                    && cmd.is_interactive()
                    && let Some(mut term) = ai_sessions::Terminal::tty()
                    && let Some(enabled) = ai_sessions::offer(&settings, &mut term).await
                {
                    settings = enabled;
                }

                // Resume reads the threads database itself, which a daemon of an older build
                // keeps at an older schema: replace one, as history does, when Atuin manages it.
                #[cfg(feature = "daemon")]
                if matches!(cli, atuin_ai::commands::Command::Resume(_))
                    && settings.daemon.enabled
                    && settings.daemon.autostart
                    && let Err(e) = daemon::ready_client(&settings).await
                {
                    tracing::debug!(?e, "couldn't make the daemon ready before resuming");
                }

                // Threads commands talk to the daemon: start or replace it as history does,
                // instead of failing against a stopped or stale one.
                #[cfg(feature = "daemon")]
                if matches!(cli, atuin_ai::commands::Command::Threads(_)) {
                    daemon::ready_client(&settings).await?;
                }

                atuin_ai::commands::run(cli, settings).await
            }

            #[cfg(feature = "ai")]
            Self::Mcp(cmd) => cmd.run(&db, settings).await,
        }
    }

    fn log_mode(&self) -> Option<LogMode> {
        match self {
            Self::History(cmd) => cmd.log_mode(),
            Self::Internal(cmd) => cmd.log_mode(),

            // The TUI owns the terminal.
            Self::Search(cmd) if cmd.is_interactive() => Some(LogMode::File),

            #[cfg(feature = "daemon")]
            Self::Daemon(cmd) if cmd.runs_daemon() => Some(LogMode::Daemon {
                show_logs: cmd.show_logs(),
            }),

            #[cfg(feature = "ai")]
            Self::Ai(cmd) => cmd.log_mode(),
            #[cfg(feature = "ai")]
            Self::Threads(cmd) => Some(cmd.log_mode()),
            #[cfg(feature = "ai")]
            Self::Resume(_) => Some(LogMode::File),

            // Shell startup and prompt hooks: a log file costs time on every prompt.
            Self::Init(_) | Self::Hook(_) => Some(LogMode::Stderr),

            Self::Search(_)
            | Self::Stats(_)
            | Self::Setup
            | Self::Import(_)
            | Self::Doctor
            | Self::Kv(_)
            | Self::Store(_)
            | Self::Dotfiles(_)
            | Self::Scripts(_)
            | Self::Wrapped { .. }
            | Self::DefaultConfig
            | Self::Config(_)
            | Self::Info
            | Self::InternalDecoy => Some(LogMode::FileAndStderr),
            #[cfg(feature = "daemon")]
            Self::Output(_) | Self::Daemon(_) => Some(LogMode::FileAndStderr),
            #[cfg(feature = "sync")]
            Self::Sync(_) | Self::Account(_) => Some(LogMode::FileAndStderr),
            #[cfg(feature = "self-update")]
            Self::Update(_) => Some(LogMode::FileAndStderr),
            #[cfg(feature = "ai")]
            Self::Mcp(_) => Some(LogMode::FileAndStderr),
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::logs::LogMode;
    use clap::Parser;
    use rstest::rstest;

    use super::Cmd;

    const SECRET: &str = "hunter2-s3cr3t";

    #[derive(Parser, Debug)]
    struct Cli {
        #[command(subcommand)]
        cmd: Cmd,
    }

    #[rstest]
    #[cfg_attr(feature = "sync", case::login(&["login", "-u", "u", "--password", SECRET]))]
    #[cfg_attr(feature = "sync", case::login_key(&["login", "-u", "u", "--key", SECRET]))]
    #[cfg_attr(feature = "sync", case::login_totp(&["login", "-u", "u", "--totp-code", SECRET]))]
    #[cfg_attr(feature = "sync", case::register(&["register", "-u", "u", "--password", SECRET]))]
    #[cfg_attr(feature = "sync", case::account_login(&["account", "login", "--password", SECRET]))]
    #[cfg_attr(feature = "sync", case::delete(&["account", "delete", "--password", SECRET]))]
    #[cfg_attr(
        feature = "sync",
        case::change_current(&["account", "change-password", "--current-password", SECRET])
    )]
    #[cfg_attr(
        feature = "sync",
        case::change_new(&["account", "change-password", "--new-password", SECRET])
    )]
    #[case::rekey(&["store", "rekey", SECRET])]
    #[cfg_attr(feature = "ai", case::ai_token(&["ai", "inline", "--api-token", SECRET]))]
    fn debug_redacts_argv_secrets(#[case] args: &[&str]) {
        let cli = Cli::try_parse_from(std::iter::once("atuin").chain(args.iter().copied()))
            .expect("the argv parses");
        assert!(!format!("{:?}", cli.cmd).contains(SECRET));
    }

    #[rstest]
    #[case::search_interactive(&["search", "-i"], Some(LogMode::File))]
    #[case::search(&["search", "foo"], Some(LogMode::FileAndStderr))]
    #[case::history_hook(&["history", "start", "--hook", "--", "ls"], None)]
    #[case::history_list(&["history", "list"], Some(LogMode::FileAndStderr))]
    #[case::init(&["init", "zsh"], Some(LogMode::Stderr))]
    #[case::doctor(&["doctor"], Some(LogMode::FileAndStderr))]
    #[cfg_attr(
        feature = "daemon",
        case::daemon(&["daemon"], Some(LogMode::Daemon { show_logs: false }))
    )]
    #[cfg_attr(
        feature = "daemon",
        case::daemon_show_logs(
            &["daemon", "start", "--show-logs"],
            Some(LogMode::Daemon { show_logs: true })
        )
    )]
    #[cfg_attr(
        feature = "daemon",
        case::daemon_status(&["daemon", "status"], Some(LogMode::FileAndStderr))
    )]
    #[cfg_attr(
        feature = "daemon",
        case::daemon_restart(&["daemon", "restart"], Some(LogMode::FileAndStderr))
    )]
    #[cfg_attr(feature = "ai", case::ai_inline(&["ai", "inline"], Some(LogMode::File)))]
    #[cfg_attr(feature = "ai", case::resume(&["resume"], Some(LogMode::File)))]
    #[cfg_attr(
        feature = "ai",
        case::threads_resume(&["threads", "resume"], Some(LogMode::File))
    )]
    #[cfg_attr(
        feature = "ai",
        case::threads_list(&["threads", "list"], Some(LogMode::FileAndStderr))
    )]
    fn log_mode_per_command(#[case] args: &[&str], #[case] expected: Option<LogMode>) {
        let cli = Cli::try_parse_from(std::iter::once("atuin").chain(args.iter().copied()))
            .expect("the argv parses");
        assert_eq!(cli.cmd.log_mode(), expected);
    }

    /// `atuin resume`, `atuin threads resume` and (as it was) `atuin ai resume` take the same.
    #[cfg(feature = "ai")]
    #[rstest]
    #[case::short(&["resume"])]
    #[case::threads(&["threads", "resume"])]
    #[case::ai(&["ai", "resume"])]
    fn resume_parses_everywhere(#[case] command: &[&str]) {
        let args = std::iter::once(&"atuin").chain(command).chain(&["0199abcdef", "--in", "codex"]);
        Cli::try_parse_from(args).expect("the argv parses");
    }

    /// `atuin ai session` is still `atuin threads`.
    #[cfg(feature = "ai")]
    #[rstest]
    fn ai_session_is_still_threads() {
        let cli = Cli::try_parse_from(["atuin", "ai", "session", "list", "--style", "json"])
            .expect("the argv parses");
        assert!(
            matches!(cli.cmd, Cmd::Ai(atuin_ai::commands::Command::Threads(_))),
            "{:?}",
            cli.cmd
        );
    }
}
