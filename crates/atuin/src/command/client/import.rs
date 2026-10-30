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
use atuin_common::{env, userio};
use clap::Parser;
use easy_cast::Conv;
use eyre::Result;
use indicatif::ProgressBar;
use tracing::instrument;

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
        userio::log::info!("import-banner");
        userio::log::info!("import-start");

        match self {
            Self::Auto => {
                if cfg!(windows) {
                    return if env::var("PSModulePath").is_ok() {
                        userio::log::info!("import-detected-powershell");
                        import::<PowerShell>(db).await
                    } else {
                        userio::log::error!("import-detect-shell-failed");
                        userio::log::info!("import-run-with-shell");
                        userio::log::info!("import-list-shells");
                        Ok(())
                    };
                }

                // $XONSH_HISTORY_BACKEND isn't always set, but $XONSH_HISTORY_FILE is
                let xonsh_histfile =
                    env::var("XONSH_HISTORY_FILE").unwrap_or_else(|_| String::new());
                let shell = env::var("SHELL").unwrap_or_else(|_| String::from("NO_SHELL"));

                if xonsh_histfile.to_lowercase().ends_with(".json") {
                    userio::log::info!("import-detected-xonsh");
                    import::<Xonsh>(db).await
                } else if xonsh_histfile.to_lowercase().ends_with(".sqlite") {
                    userio::log::info!("import-detected-xonsh-sqlite");
                    import::<XonshSqlite>(db).await
                } else if shell.ends_with("/zsh") {
                    if let Ok(path) = ZshHistDb::histpath() {
                        userio::log::info!(
                            "import-detected-zsh-histdb",
                            path = path.display().to_string()
                        );
                        import::<ZshHistDb>(db).await
                    } else {
                        userio::log::info!("import-detected-zsh");
                        import::<Zsh>(db).await
                    }
                } else if shell.ends_with("/fish") {
                    userio::log::info!("import-detected-fish");
                    import::<Fish>(db).await
                } else if shell.ends_with("/bash") {
                    userio::log::info!("import-detected-bash");
                    import::<Bash>(db).await
                } else if shell.ends_with("/nu") {
                    if let Ok(path) = NuHistDb::histpath() {
                        userio::log::info!(
                            "import-detected-nu-histdb",
                            path = path.display().to_string()
                        );
                        import::<NuHistDb>(db).await
                    } else {
                        userio::log::info!("import-detected-nushell");
                        import::<Nu>(db).await
                    }
                } else if shell.ends_with("/pwsh") {
                    userio::log::info!("import-detected-powershell");
                    import::<PowerShell>(db).await
                } else {
                    userio::log::error!("import-shell-unsupported", shell = shell.as_str());
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
    userio::log::info!("import-from", name = I::NAME);

    let mut importer = I::new().await?;
    let len = importer.entries().await?;
    let mut loader = HistoryImporter::new(db, len);
    importer.load(&mut loader).await?;
    loader.flush().await?;

    userio::log::info!("import-complete");
    Ok(())
}
