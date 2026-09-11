use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::logs::FromSettings;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_client::theme;
use atuin_common::logs::{self, LogConfig};
use clap::Subcommand;
use eyre::{Result, WrapErr};

use crate::logs::LogCtx;

#[cfg(feature = "sync")]
mod sync;

#[cfg(feature = "sync")]
mod account;

#[cfg(feature = "daemon")]
mod daemon;

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
mod scripts;
mod search;
mod setup;
mod stats;
mod store;
#[cfg(feature = "self-update")]
mod update;
mod wrapped;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    // Variant order sets the `--help` command list order, so keep the commands users reach for
    // most (search, sync, stats) at the top and the plumbing/config commands lower down.
    /// Interactive history search
    Search(search::Cmd),

    /// Calculate statistics for your history
    Stats(stats::Cmd),

    #[cfg(feature = "sync")]
    #[command(flatten)]
    Sync(sync::Cmd),

    /// Manage your sync account
    #[cfg(feature = "sync")]
    Account(account::Cmd),

    /// Manipulate shell history
    #[command(subcommand)]
    History(history::Cmd),

    /// Setup Atuin features
    #[command()]
    Setup,

    /// Print Atuin's shell init script
    #[command()]
    Init(init::Cmd),

    /// Import shell history from file
    #[command(subcommand)]
    Import(import::Cmd),

    /// Run the doctor to check for common issues
    #[command()]
    Doctor,

    /// Update atuin to the latest version on your release channel
    #[cfg(feature = "self-update")]
    #[command()]
    Update(update::Cmd),

    /// Get or set small key-value pairs
    #[command(subcommand)]
    Kv(kv::Cmd),

    /// Manage the atuin data store
    #[command(subcommand)]
    Store(store::Cmd),

    /// Manage your dotfiles with Atuin
    #[command(subcommand)]
    Dotfiles(dotfiles::Cmd),

    /// Manage your scripts with Atuin
    #[command(subcommand)]
    Scripts(scripts::Cmd),

    /// Manage AI-agent shell hooks
    Hook(hook::Cmd),

    /// Run the AI assistant
    #[cfg(feature = "ai")]
    #[command(subcommand)]
    Ai(atuin_ai::commands::Command),

    /// Start an MCP server exposing history search to AI tools (stdio)
    #[cfg(feature = "ai")]
    #[command()]
    Mcp,

    #[command()]
    Wrapped {
        year: Option<i32>,
    },

    /// Print the default atuin configuration (config.toml)
    #[command()]
    DefaultConfig,

    #[command(subcommand)]
    Config(config::Cmd),

    /// Information about dotfiles locations and ENV vars
    #[command()]
    Info,

    /// *Experimental* Manage the background daemon
    #[cfg(feature = "daemon")]
    #[command()]
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

    /// We want to exclude the `__internal` subcommand from Clap's `infer_subcommands`; otherwise,
    /// a user could access it simply by typing `atuin _`. However, Clap has no way to disable
    /// `infer_subcommands` for a single command. As a workaround, we define a dummy command with
    /// the same name but with an extra understore, which forces `__internal` to be typed out in
    /// entirety, since any prefix of the name would be ambiguous.
    #[command(
        hide = true,
        name = "__internal_",
        disable_help_flag = true,
        disable_help_subcommand = true
    )]
    InternalDecoy,
}

impl Cmd {
    pub fn run(self) -> Result<()> {
        // Daemonize before creating the async runtime – fork() inside a live
        // tokio runtime corrupts its internal state.
        #[cfg(all(unix, feature = "daemon"))]
        if let Self::Daemon(ref cmd) = self
            && cmd.should_daemonize()
        {
            daemon::daemonize_current_process()?;
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

        let settings = Settings::new().wrap_err("could not load client settings")?;
        let _logging = self
            .log_config(&settings)
            .map(|c| LogCtx::try_enable("atuin", &c))
            .transpose()
            .wrap_err("failed to enable logging")?;

        Ok(Some(async {
            if let Some(func) = run_internal {
                func(settings).await
            } else {
                Box::pin(self.run_async(settings)).await
            }
        }))
    }

    #[allow(clippy::too_many_lines)]
    // `atuin_ai::commands::run` is not `Send` because `eye_declare` holds a `StdoutLock` across
    // await points.
    #[allow(clippy::future_not_send)]
    async fn run_async(self, mut settings: Settings) -> Result<()> {
        tracing::trace!(command = ?self, "client command");
        let mut theme_manager = theme::ThemeManager::new(settings.theme.debug, None);

        // Skip initializing any databases for history
        // This is a pretty hot path, as it runs before and after every single command the user
        // runs
        match self {
            Self::History(history) => return history.run(&settings).await,
            Self::Hook(hook) => return hook.run(&settings).await,
            Self::Init(init) => return init.run(&settings).await,
            Self::Doctor => return doctor::run(&settings).await,
            #[cfg(feature = "self-update")]
            Self::Update(update) => return update.run(&settings).await,
            Self::Config(config) => return config.run(&settings).await,
            Self::InternalDecoy => {
                eprintln!("error: this command is not meant to be accessed directly");
                std::process::exit(1);
            }
            _ => {}
        }

        let db_path = &settings.db_path;
        let record_store_path = &settings.record_store_path;

        let db = Sqlite::new(db_path, Duration::try_from_secs_f64(settings.local_timeout)?).await?;
        let sqlite_store = SqliteStore::new(
            record_store_path,
            Duration::try_from_secs_f64(settings.local_timeout)?,
        )
        .await?;

        let theme_name = settings.theme.name.clone();
        let theme = theme_manager.load_theme(theme_name.as_str(), settings.theme.max_depth);

        match self {
            Self::Setup => setup::run(&settings).await,
            Self::Import(import) => import.run(&db).await,
            Self::Stats(stats) => stats.run(&db, &settings, theme).await,
            Self::Search(search) => search.run(db, &mut settings, sqlite_store, theme).await,

            #[cfg(feature = "sync")]
            Self::Sync(sync) => sync.run(settings, &db, sqlite_store).await,

            #[cfg(feature = "sync")]
            Self::Account(account) => account.run(settings, sqlite_store).await,

            Self::Kv(kv) => kv.run(&settings, &sqlite_store).await,

            Self::Store(store) => store.run(&settings, &db, sqlite_store).await,

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

            #[cfg(feature = "daemon")]
            Self::Daemon(cmd) => cmd.run(settings, sqlite_store, db).await,

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

            #[cfg(feature = "ai")]
            Self::Ai(cli) => atuin_ai::commands::run(cli, &settings).await,

            #[cfg(feature = "ai")]
            Self::Mcp => atuin_ai::mcp::run(&db).await,
        }
    }

    fn log_config(&self, settings: &Settings) -> Option<LogConfig> {
        match self {
            Self::History(cmd) => cmd.log_config(),

            Self::Search(cmd) if cmd.is_interactive() => {
                Some(LogConfig::from_settings(&settings.logs, &settings.logs.search))
            }

            #[cfg(feature = "daemon")]
            Self::Daemon(cmd) => Some(LogConfig {
                file: logs::FileConfig::from_settings(&settings.logs, &settings.logs.daemon),
                stderr: cmd.show_logs().then(logs::StderrConfig::verbose),
            }),

            #[cfg(feature = "ai")]
            Self::Ai(cmd) => cmd.log_config(settings),

            Self::Internal(cmd) => cmd.log_config(),

            _ => Some(LogConfig::stderr_only()),
        }
    }
}
