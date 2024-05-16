use std::env;

use async_trait::async_trait;
use clap::Parser;
use eyre::Result;
use indicatif::ProgressBar;

use atuin_client::{
    database::Database,
    history::History,
    import::{
        bash::Bash, fish::Fish, nu::Nu, nu_histdb::NuHistDb, replxx::Replxx, resh::Resh,
        xonsh::Xonsh, xonsh_sqlite::XonshSqlite, zsh::Zsh, zsh_histdb::ZshHistDb, Importer, Loader,
    },
};

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
}

const BATCH_SIZE: usize = 100;

impl Cmd {
    pub async fn run<DB: Database>(&self, db: &DB) -> Result<()> {
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
                    println!("This feature does not work on windows. Please run atuin import <SHELL>. To view a list of shells, run atuin import.");
                    return Ok(());
                }

                // $XONSH_HISTORY_BACKEND isn't always set, but $XONSH_HISTORY_FILE is
                let xonsh_histfile =
                    env::var("XONSH_HISTORY_FILE").unwrap_or_else(|_| String::new());
                let shell = env::var("SHELL").unwrap_or_else(|_| String::from("NO_SHELL"));

                if xonsh_histfile.to_lowercase().ends_with(".json") {
                    println!("Detected Xonsh",);
                    import::<Xonsh, DB>(db).await
                } else if xonsh_histfile.to_lowercase().ends_with(".sqlite") {
                    println!("Detected Xonsh (SQLite backend)");
                    import::<XonshSqlite, DB>(db).await
                } else if shell.ends_with("/zsh") {
                    if ZshHistDb::histpath().is_ok() {
                        println!(
                            "Detected Zsh-HistDb, using :{}",
                            ZshHistDb::histpath().unwrap().to_str().unwrap()
                        );
                        import::<ZshHistDb, DB>(db).await
                    } else {
                        println!("Detected ZSH");
                        import::<Zsh, DB>(db).await
                    }
                } else if shell.ends_with("/fish") {
                    println!("Detected Fish");
                    import::<Fish, DB>(db).await
                } else if shell.ends_with("/bash") {
                    println!("Detected Bash");
                    import::<Bash, DB>(db).await
                } else if shell.ends_with("/nu") {
                    if NuHistDb::histpath().is_ok() {
                        println!(
                            "Detected Nu-HistDb, using :{}",
                            NuHistDb::histpath().unwrap().to_str().unwrap()
                        );
                        import::<NuHistDb, DB>(db).await
                    } else {
                        println!("Detected Nushell");
                        import::<Nu, DB>(db).await
                    }
                } else {
                    println!("cannot import {shell} history");
                    Ok(())
                }
            }

            Self::Zsh => import::<Zsh, DB>(db).await,
            Self::ZshHistDb => import::<ZshHistDb, DB>(db).await,
            Self::Bash => import::<Bash, DB>(db).await,
            Self::Replxx => import::<Replxx, DB>(db).await,
            Self::Resh => import::<Resh, DB>(db).await,
            Self::Fish => import::<Fish, DB>(db).await,
            Self::Nu => import::<Nu, DB>(db).await,
            Self::NuHistDb => import::<NuHistDb, DB>(db).await,
            Self::Xonsh => import::<Xonsh, DB>(db).await,
            Self::XonshSqlite => import::<XonshSqlite, DB>(db).await,
        }
    }
}

pub struct HistoryImporter<'db, DB: Database> {
    pb: ProgressBar,
    buf: Vec<History>,
    db: &'db DB,
}

impl<'db, DB: Database> HistoryImporter<'db, DB> {
    fn new(db: &'db DB, len: usize) -> Self {
        Self {
            pb: ProgressBar::new(len as u64),
            buf: Vec::with_capacity(BATCH_SIZE),
            db,
        }
    }

    async fn flush(self) -> Result<()> {
        if !self.buf.is_empty() {
            self.db.save_bulk(&self.buf).await?;
        }
        self.pb.finish();
        Ok(())
    }
}

#[async_trait]
impl<'db, DB: Database> Loader for HistoryImporter<'db, DB> {
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

async fn import<I: Importer + Send, DB: Database>(db: &DB) -> Result<()> {
    println!("Importing history from {}", I::NAME);

    let mut importer = I::new().await?;
    let len = importer.entries().await.unwrap();
    let mut loader = HistoryImporter::new(db, len);
    importer.load(&mut loader).await?;
    loader.flush().await?;

    println!("Import complete!");
    Ok(())
}
