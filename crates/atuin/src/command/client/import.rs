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
use atuin_common::env;
use atuin_common::logs::USER;
use clap::Parser;
use easy_cast::Conv;
use eyre::Result;
use indicatif::ProgressBar;
use tracing::{error, info, instrument};

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
    pub async fn run(&self, db: &Sqlite) -> Result<()> {
        const BANNER: &str = concat!(
            "        Atuin         \n",
            "======================\n",
            "          \u{1f30d}          \n",
            "       \u{1f418}\u{1f418}\u{1f418}\u{1f418}       \n",
            "          \u{1f422}          \n",
            "======================",
        );
        info!(target: USER, "{BANNER}");
        info!(target: USER, "Importing history...");

        match self {
            Self::Auto => {
                if cfg!(windows) {
                    return if env::var("PSModulePath").is_ok() {
                        info!(target: USER, "Detected PowerShell");
                        import::<PowerShell>(db).await
                    } else {
                        error!(target: USER, "could not detect the current shell");
                        info!(target: USER, "Please run atuin import <SHELL>.");
                        info!(target: USER, "To view a list of shells, run atuin import.");
                        Ok(())
                    };
                }

                // $XONSH_HISTORY_BACKEND isn't always set, but $XONSH_HISTORY_FILE is
                let xonsh_histfile =
                    env::var("XONSH_HISTORY_FILE").unwrap_or_else(|_| String::new());
                let shell = env::var("SHELL").unwrap_or_else(|_| String::from("NO_SHELL"));

                if xonsh_histfile.to_lowercase().ends_with(".json") {
                    info!(target: USER, "Detected Xonsh");
                    import::<Xonsh>(db).await
                } else if xonsh_histfile.to_lowercase().ends_with(".sqlite") {
                    info!(target: USER, "Detected Xonsh (SQLite backend)");
                    import::<XonshSqlite>(db).await
                } else if shell.ends_with("/zsh") {
                    if let Ok(path) = ZshHistDb::histpath() {
                        info!(
                            target: USER,
                            "Detected Zsh-HistDb, using :{}",
                            path.to_string_lossy()
                        );
                        import::<ZshHistDb>(db).await
                    } else {
                        info!(target: USER, "Detected ZSH");
                        import::<Zsh>(db).await
                    }
                } else if shell.ends_with("/fish") {
                    info!(target: USER, "Detected Fish");
                    import::<Fish>(db).await
                } else if shell.ends_with("/bash") {
                    info!(target: USER, "Detected Bash");
                    import::<Bash>(db).await
                } else if shell.ends_with("/nu") {
                    if let Ok(path) = NuHistDb::histpath() {
                        info!(
                            target: USER,
                            "Detected Nu-HistDb, using :{}",
                            path.to_string_lossy()
                        );
                        import::<NuHistDb>(db).await
                    } else {
                        info!(target: USER, "Detected Nushell");
                        import::<Nu>(db).await
                    }
                } else if shell.ends_with("/pwsh") {
                    info!(target: USER, "Detected PowerShell");
                    import::<PowerShell>(db).await
                } else {
                    error!(target: USER, "cannot import {shell} history");
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
            pb: ProgressBar::new(u64::conv(len)),
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
    info!(target: USER, "Importing history from {}", I::NAME);

    let mut importer = I::new().await?;
    let len = importer.entries().await?;
    let mut loader = HistoryImporter::new(db, len);
    importer.load(&mut loader).await?;
    loader.flush().await?;

    info!(target: USER, "Import complete!");
    Ok(())
}
