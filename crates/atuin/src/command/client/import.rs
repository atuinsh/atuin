use std::env;

use async_trait::async_trait;
use atuin_client::database::Sqlite;
use atuin_client::history::History;
use atuin_client::history::store::HistoryStore;
use atuin_client::import::bash::Bash;
use atuin_client::import::fish::Fish;
use atuin_client::import::nu::Nu;
use atuin_client::import::nu_histdb::NuHistDb;
use atuin_client::import::powershell::PowerShell;
use atuin_client::import::replxx::Replxx;
use atuin_client::import::resh::Resh;
use atuin_client::import::xonsh::Xonsh;
use atuin_client::import::xonsh_sqlite::XonshSqlite;
use atuin_client::import::zsh::Zsh;
use atuin_client::import::zsh_histdb::ZshHistDb;
use atuin_client::import::{Importer, Loader};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use clap::Parser;
use easy_cast::Conv;
use eyre::{Context, Result};
use indicatif::ProgressBar;
use tracing::instrument;

#[cfg(feature = "daemon")]
use super::daemon;
use crate::i18n::fl;

#[derive(Parser, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(about = fl!("cmd-import-auto"))]
    Auto,

    #[command(about = fl!("cmd-import-shell", shell = "zsh"))]
    Zsh,
    #[command(about = fl!("cmd-import-zsh-hist-db"))]
    ZshHistDb,
    #[command(about = fl!("cmd-import-shell", shell = "bash"))]
    Bash,
    #[command(about = fl!("cmd-import-shell", shell = "replxx"))]
    Replxx,
    #[command(about = fl!("cmd-import-shell", shell = "resh"))]
    Resh,
    #[command(about = fl!("cmd-import-shell", shell = "fish"))]
    Fish,
    #[command(about = fl!("cmd-import-shell", shell = "nu"))]
    Nu,
    #[command(about = fl!("cmd-import-nu-hist-db"))]
    NuHistDb,
    #[command(about = fl!("cmd-import-xonsh"))]
    Xonsh,
    #[command(about = fl!("cmd-import-xonsh-sqlite"))]
    XonshSqlite,
    #[command(about = fl!("cmd-import-shell", shell = "powershell"))]
    Powershell,
}

const BATCH_SIZE: usize = 100;

impl Cmd {
    #[allow(clippy::cognitive_complexity)]
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(&self, settings: &Settings, db: &Sqlite, store: SqliteStore) -> Result<()> {
        let target = Target::new(settings, db, store).await?;
        let target = &target;

        println!("        Atuin         ");
        println!("======================");
        println!("          \u{1f30d}          ");
        println!("       \u{1f418}\u{1f418}\u{1f418}\u{1f418}       ");
        println!("          \u{1f422}          ");
        println!("======================");
        println!("Importing history...");

        match self {
            Self::Auto => {
                if cfg!(windows) {
                    return if env::var("PSModulePath").is_ok() {
                        println!("Detected PowerShell");
                        import::<PowerShell>(target).await
                    } else {
                        println!("Could not detect the current shell.");
                        println!("Please run atuin import <SHELL>.");
                        println!("To view a list of shells, run atuin import.");
                        Ok(())
                    };
                }

                // $XONSH_HISTORY_BACKEND isn't always set, but $XONSH_HISTORY_FILE is
                let xonsh_histfile =
                    env::var("XONSH_HISTORY_FILE").unwrap_or_else(|_| String::new());
                let shell = env::var("SHELL").unwrap_or_else(|_| String::from("NO_SHELL"));

                if xonsh_histfile.to_lowercase().ends_with(".json") {
                    println!("Detected Xonsh");
                    import::<Xonsh>(target).await
                } else if xonsh_histfile.to_lowercase().ends_with(".sqlite") {
                    println!("Detected Xonsh (SQLite backend)");
                    import::<XonshSqlite>(target).await
                } else if shell.ends_with("/zsh") {
                    if let Ok(path) = ZshHistDb::histpath() {
                        println!("Detected Zsh-HistDb, using :{}", path.to_string_lossy());
                        import::<ZshHistDb>(target).await
                    } else {
                        println!("Detected ZSH");
                        import::<Zsh>(target).await
                    }
                } else if shell.ends_with("/fish") {
                    println!("Detected Fish");
                    import::<Fish>(target).await
                } else if shell.ends_with("/bash") {
                    println!("Detected Bash");
                    import::<Bash>(target).await
                } else if shell.ends_with("/nu") {
                    if let Ok(path) = NuHistDb::histpath() {
                        println!("Detected Nu-HistDb, using :{}", path.to_string_lossy());
                        import::<NuHistDb>(target).await
                    } else {
                        println!("Detected Nushell");
                        import::<Nu>(target).await
                    }
                } else if shell.ends_with("/pwsh") {
                    println!("Detected PowerShell");
                    import::<PowerShell>(target).await
                } else {
                    println!("cannot import {shell} history");
                    Ok(())
                }
            }

            Self::Zsh => import::<Zsh>(target).await,
            Self::ZshHistDb => import::<ZshHistDb>(target).await,
            Self::Bash => import::<Bash>(target).await,
            Self::Replxx => import::<Replxx>(target).await,
            Self::Resh => import::<Resh>(target).await,
            Self::Fish => import::<Fish>(target).await,
            Self::Nu => import::<Nu>(target).await,
            Self::NuHistDb => import::<NuHistDb>(target).await,
            Self::Xonsh => import::<Xonsh>(target).await,
            Self::XonshSqlite => import::<XonshSqlite>(target).await,
            Self::Powershell => import::<PowerShell>(target).await,
        }
    }
}

/// Where imported history goes: through the daemon when it's enabled, so its search index sees it
/// too; otherwise into the history db and record store directly.
enum Target<'a> {
    /// The daemon holds the key and writes the store itself.
    #[cfg(feature = "daemon")]
    Daemon(&'a Settings),
    Local {
        db: &'a Sqlite,
        history_store: HistoryStore,
    },
}

impl<'a> Target<'a> {
    async fn new(settings: &'a Settings, db: &'a Sqlite, store: SqliteStore) -> Result<Self> {
        // An enabled daemon owns the store: if it isn't running and won't start, fail up front
        // rather than write behind it.
        #[cfg(feature = "daemon")]
        if settings.daemon.enabled {
            daemon::ready_client(settings).await?;
            return Ok(Self::Daemon(settings));
        }

        // Generated if missing: the installer runs `atuin import` before anything has made one.
        let encryption_key = paseto_v4::Key::try_load_or_generate(&settings.key_path)
            .context("could not load or generate encryption key")?;
        let host_id = Settings::host_id().await?;
        let history_store = HistoryStore::new(store, host_id, encryption_key);
        Ok(Self::Local { db, history_store })
    }

    async fn import(&self, histories: Vec<History>) -> Result<()> {
        match self {
            #[cfg(feature = "daemon")]
            Self::Daemon(settings) => {
                daemon::import_history(settings, histories).await?;
            }
            Self::Local { db, history_store } => {
                history_store.import(db, histories).await?;
            }
        }
        Ok(())
    }
}

pub struct HistoryImporter<'t> {
    pb: ProgressBar,
    buf: Vec<History>,
    target: &'t Target<'t>,
}

impl<'t> HistoryImporter<'t> {
    fn new(target: &'t Target<'t>, len: usize) -> Self {
        Self {
            pb: ProgressBar::new(u64::conv(len)),
            buf: Vec::with_capacity(BATCH_SIZE),
            target,
        }
    }

    async fn flush(self) -> Result<()> {
        self.target.import(self.buf).await?;
        self.pb.finish();
        Ok(())
    }
}

#[async_trait]
impl Loader for HistoryImporter<'_> {
    async fn push(&mut self, hist: History) -> Result<()> {
        self.pb.inc(1);
        self.buf.push(hist);
        // Each batch is one import: one db transaction, one store push and, through the daemon,
        // one RPC.
        if self.buf.len() == BATCH_SIZE {
            self.target.import(std::mem::take(&mut self.buf)).await?;
        }
        Ok(())
    }
}

async fn import<I: Importer + Send>(target: &Target<'_>) -> Result<()> {
    println!("Importing history from {}", I::NAME);

    let mut importer = I::new().await?;
    let len = importer.entries().await?;
    let mut loader = HistoryImporter::new(target, len);
    importer.load(&mut loader).await?;
    loader.flush().await?;

    println!("Import complete!");
    Ok(())
}
