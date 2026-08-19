use std::env;

use async_trait::async_trait;
use atuin_client::database::Sqlite;
use atuin_client::history::History;
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
use clap::Parser;
use eyre::Result;
use indicatif::ProgressBar;

#[derive(Parser, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Import history for the current shell
    Auto,

    /// Import history from the zsh history file
    Zsh,
    /// Import history from the zsh history file
    ZshHistDb,
    /// Import history from the bash history file
    Bash,
    /// Import history from the replxx history file
    Replxx,
    /// Import history from the resh history file
    Resh,
    /// Import history from the fish history file
    Fish,
    /// Import history from the nu history file
    Nu,
    /// Import history from the nu history file
    NuHistDb,
    /// Import history from xonsh json files
    Xonsh,
    /// Import history from xonsh sqlite db
    XonshSqlite,
    /// Import history from the powershell history file
    Powershell,
}

const BATCH_SIZE: usize = 100;

impl Cmd {
    #[allow(clippy::cognitive_complexity)]
    pub async fn run(&self, db: &Sqlite) -> Result<()> {
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
                        import::<PowerShell>(db).await
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
                    import::<Xonsh>(db).await
                } else if xonsh_histfile.to_lowercase().ends_with(".sqlite") {
                    println!("Detected Xonsh (SQLite backend)");
                    import::<XonshSqlite>(db).await
                } else if shell.ends_with("/zsh") {
                    if let Ok(path) = ZshHistDb::histpath() {
                        println!("Detected Zsh-HistDb, using :{}", path.to_string_lossy());
                        import::<ZshHistDb>(db).await
                    } else {
                        println!("Detected ZSH");
                        import::<Zsh>(db).await
                    }
                } else if shell.ends_with("/fish") {
                    println!("Detected Fish");
                    import::<Fish>(db).await
                } else if shell.ends_with("/bash") {
                    println!("Detected Bash");
                    import::<Bash>(db).await
                } else if shell.ends_with("/nu") {
                    if let Ok(path) = NuHistDb::histpath() {
                        println!("Detected Nu-HistDb, using :{}", path.to_string_lossy());
                        import::<NuHistDb>(db).await
                    } else {
                        println!("Detected Nushell");
                        import::<Nu>(db).await
                    }
                } else if shell.ends_with("/pwsh") {
                    println!("Detected PowerShell");
                    import::<PowerShell>(db).await
                } else {
                    println!("cannot import {shell} history");
                    Ok(())
                }
            }

            Self::Zsh => import::<Zsh>(db).await,
            Self::ZshHistDb => import::<ZshHistDb>(db).await,
            Self::Bash => import::<Bash>(db).await,
            Self::Replxx => import::<Replxx>(db).await,
            Self::Resh => import::<Resh>(db).await,
            Self::Fish => import::<Fish>(db).await,
            Self::Nu => import::<Nu>(db).await,
            Self::NuHistDb => import::<NuHistDb>(db).await,
            Self::Xonsh => import::<Xonsh>(db).await,
            Self::XonshSqlite => import::<XonshSqlite>(db).await,
            Self::Powershell => import::<PowerShell>(db).await,
        }
    }
}

pub struct HistoryImporter<'db> {
    pb: ProgressBar,
    buf: Vec<History>,
    db: &'db Sqlite,
}

impl<'db> HistoryImporter<'db> {
    fn new(db: &'db Sqlite, len: usize) -> Self {
        Self {
            pb: ProgressBar::new(len as u64),
            buf: Vec::with_capacity(BATCH_SIZE),
            db,
        }
    }

    async fn flush(self) -> Result<()> {
        self.db.save_bulk(&self.buf).await?;
        self.pb.finish();
        Ok(())
    }
}

#[async_trait]
impl Loader for HistoryImporter<'_> {
    async fn push(&mut self, hist: History) -> Result<()> {
        self.pb.inc(1);
        self.buf.push(hist);
        if self.buf.len() == self.buf.capacity() {
            self.db.save_bulk(&self.buf).await?;
            self.buf.clear();
        }
        Ok(())
    }
}

async fn import<I: Importer + Send>(db: &Sqlite) -> Result<()> {
    println!("Importing history from {}", I::NAME);

    let mut importer = I::new().await?;
    let len = importer.entries().await?;
    let mut loader = HistoryImporter::new(db, len);
    importer.load(&mut loader).await?;
    loader.flush().await?;

    println!("Import complete!");
    Ok(())
}
