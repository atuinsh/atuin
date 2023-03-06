use std::{
    env,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use clap::{Parser, ValueEnum, ValueHint};
use eyre::{bail, eyre, Result};
use indicatif::ProgressBar;

use atuin_client::{
    database::Database,
    history::History,
    import::{
        bash::Bash,
        fish::Fish,
        resh::Resh,
        zsh::Zsh,
        zsh_histdb::{can_connect_as_db, ZshHistDb},
        Importer, Loader, PathSource,
    },
};

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Shell {
    /// Import history for the current shell
    Auto,

    /// Import history from the zsh history file
    Zsh,
    /// Import history from the zsh history file
    ZshHistDb,
    /// Import history from the bash history file
    Bash,
    /// Import history from the resh history file
    Resh,
    /// Import history from the fish history file
    Fish,
}

const BATCH_SIZE: usize = 100;

#[derive(Clone, Debug, Parser)]
pub struct Cmd {
    #[arg(long = "from-file", value_name = "PATH", value_hint = ValueHint::FilePath)]
    custom_source: Option<PathBuf>,

    #[arg(index = 1)]
    shell: Shell,
}

impl Cmd {
    pub async fn run<DB: Database>(&self, db: &mut DB) -> Result<()> {
        println!("        Atuin         ");
        println!("======================");
        println!("          \u{1f30d}          ");
        println!("       \u{1f418}\u{1f418}\u{1f418}\u{1f418}       ");
        println!("          \u{1f422}          ");
        println!("======================");
        println!("Importing history...");

        let cli_custom_source = self.custom_source.as_deref();

        match self.shell {
            Shell::Auto if self.custom_source.is_some() => Err(eyre!(
                "You must explicitly specify a shell type when importing from a custom path."
            )),
            Shell::Auto => {
                let shell_path = {
                    let Ok(sh) = env::var("SHELL") else {
                        bail!("Cannot infer the current shell because $SHELL is unreadable.")
                    };
                    PathBuf::from(sh)
                };
                let shell = shell_path
                    .file_name()
                    .ok_or_else(|| eyre!("Unexpected value for $SHELL: {shell_path:?}."))?
                    .to_str()
                    .unwrap(); // infallible: env::var already guarantees UTF8
                match shell {
                    "bash" => {
                        println!("Detected Bash");
                        import::<Bash, DB>(db, None).await
                    }
                    "fish" => {
                        println!("Detected Fish");
                        import::<Fish, DB>(db, None).await
                    }
                    "zsh" => {
                        println!("Detected Zsh");
                        match ZshHistDb::final_source_path(Option::<&Path>::None) {
                            Ok(PathSource::Cli(_)) => unreachable!(), // already filtered
                            Ok(PathSource::Env(p)) if can_connect_as_db(&p).await => {
                                println!("{p:?} seems to be a Zsh history db file");
                                import::<ZshHistDb, DB>(db, None).await
                            }
                            Ok(PathSource::Default(p)) => {
                                println!("Found Zsh history db at {p:?}");
                                import::<ZshHistDb, DB>(db, None).await
                            }
                            Ok(PathSource::Env(p)) => {
                                println!("{p:?} seems to be a plain text Zsh history file");
                                import::<Zsh, DB>(db, None).await
                            }
                            Err(_) => {
                                println!(
                                    "No Zsh history db found; trying plain text Zsh history file"
                                );
                                let p = Zsh::final_source_path(Option::<&Path>::None)?;
                                println!("Found plain text Zsh history file at {p:?}");
                                import::<Zsh, DB>(db, None).await
                            }
                        }
                    }
                    other => {
                        println!("Unknown shell: {other}.");
                        Err(eyre!(
                            "Failed to import: inferred shell type is unsupported."
                        ))
                    }
                }
            }

            Shell::Zsh => import::<Zsh, DB>(db, cli_custom_source).await,
            Shell::ZshHistDb => import::<ZshHistDb, DB>(db, cli_custom_source).await,
            Shell::Bash => import::<Bash, DB>(db, cli_custom_source).await,
            Shell::Resh => import::<Resh, DB>(db, cli_custom_source).await,
            Shell::Fish => import::<Fish, DB>(db, cli_custom_source).await,
        }
    }
}

pub struct HistoryImporter<'db, DB: Database> {
    pb: ProgressBar,
    buf: Vec<History>,
    db: &'db mut DB,
}

impl<'db, DB: Database> HistoryImporter<'db, DB> {
    fn new(db: &'db mut DB, len: usize) -> Self {
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

async fn import<I: Importer + Send, DB: Database>(
    db: &mut DB,
    cli_custom_source: Option<&Path>,
) -> Result<()> {
    let final_source = I::final_source_path(cli_custom_source)?;
    let mut importer = I::new(final_source.path()).await?;
    let len = importer.entries().await.unwrap();
    let mut loader = HistoryImporter::new(db, len);
    importer.load(&mut loader).await?;
    loader.flush().await?;

    println!("Import from {p:?} complete!", p = final_source.path());
    Ok(())
}
