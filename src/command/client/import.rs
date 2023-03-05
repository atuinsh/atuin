use std::{env, path::PathBuf};

use async_trait::async_trait;
use clap::Parser;
use eyre::{eyre, Result};
use indicatif::ProgressBar;

use atuin_client::{
    database::Database,
    history::History,
    import::{
        bash::Bash, fish::Fish, resh::Resh, zsh::Zsh, zsh_histdb::ZshHistDb, Importer, Loader,
    },
};

#[derive(Parser)]
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
    /// Import history from the resh history file
    Resh,
    /// Import history from the fish history file
    Fish,
}

const BATCH_SIZE: usize = 100;

impl Cmd {
    pub async fn run<DB: Database>(&self, db: &mut DB) -> Result<()> {
        println!("        Atuin         ");
        println!("======================");
        println!("          \u{1f30d}          ");
        println!("       \u{1f418}\u{1f418}\u{1f418}\u{1f418}       ");
        println!("          \u{1f422}          ");
        println!("======================");
        println!("Importing history...");

        match self {
            Self::Auto => {
                let shell_path = {
                    let sh = env::var("SHELL").map_err(|_| {
                        eyre!("Cannot infer the current shell because $SHELL is unreadable.")
                    })?;
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
                        import::<Bash, DB>(db).await
                    }
                    "fish" => {
                        println!("Detected Fish");
                        import::<Fish, DB>(db).await
                    }
                    "zsh" => {
                        if let Ok(path) = ZshHistDb::histpath() {
                            println!("Detected Zsh-HistDb, using {path:?}",);
                            import::<ZshHistDb, DB>(db).await
                        } else {
                            println!("Detected ZSH");
                            import::<Zsh, DB>(db).await
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

            Self::Zsh => import::<Zsh, DB>(db).await,
            Self::ZshHistDb => import::<ZshHistDb, DB>(db).await,
            Self::Bash => import::<Bash, DB>(db).await,
            Self::Resh => import::<Resh, DB>(db).await,
            Self::Fish => import::<Fish, DB>(db).await,
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

async fn import<I: Importer + Send, DB: Database>(db: &mut DB) -> Result<()> {
    println!("Importing history from {}", I::NAME);

    let mut importer = I::new().await?;
    let len = importer.entries().await.unwrap();
    let mut loader = HistoryImporter::new(db, len);
    importer.load(&mut loader).await?;
    loader.flush().await?;

    println!("Import complete!");
    Ok(())
}
