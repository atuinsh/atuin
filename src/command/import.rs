use std::env;

use eyre::Result;
use itertools::Itertools;
use structopt::StructOpt;

use atuin_client::{history::History, import::resh::Resh};
use atuin_client::import::{bash::Bash, zsh::Zsh};
use atuin_client::{database::Database, import::Importer};
use indicatif::ProgressBar;

#[derive(StructOpt)]
pub enum Cmd {
    #[structopt(
        about="import history for the current shell",
        aliases=&["a", "au", "aut"],
    )]
    Auto,

    #[structopt(
        about="import history from the zsh history file",
        aliases=&["z", "zs"],
    )]
    Zsh,

    #[structopt(
        about="import history from the bash history file",
        aliases=&["b", "ba", "bas"],
    )]
    Bash,

    #[structopt(
        about="import history from the resh history file",
        aliases=&["r", "re", "res"],
    )]
    Resh,
}

impl Cmd {
    pub async fn run(&self, db: &mut (impl Database + Send + Sync)) -> Result<()> {
        println!("        Atuin         ");
        println!("======================");
        println!("          \u{1f30d}          ");
        println!("       \u{1f418}\u{1f418}\u{1f418}\u{1f418}       ");
        println!("          \u{1f422}          ");
        println!("======================");
        println!("Importing history...");

        const BATCH_SIZE: usize = 100;

        match self {
            Self::Auto => {
                let shell = env::var("SHELL").unwrap_or_else(|_| String::from("NO_SHELL"));

                if shell.ends_with("/zsh") {
                    println!("Detected ZSH");
                    import::<Zsh, _>(db, BATCH_SIZE).await
                } else {
                    println!("cannot import {} history", shell);
                    Ok(())
                }
            }

            Self::Zsh => import::<Zsh, _>(db, BATCH_SIZE).await,
            Self::Bash => import::<Bash, _>(db, BATCH_SIZE).await,
            Self::Resh => import::<Resh, _>(db, BATCH_SIZE).await,
        }
    }
}

async fn import<I: Importer, DB: Database + Send + Sync>(
    db: &mut DB,
    buf_size: usize,
) -> Result<()> {
    let histpath = I::histpath()?;
    let contents = I::parse(histpath)?;

    let progress = ProgressBar::new(contents.len());

    let mut buf = Vec::<History>::with_capacity(buf_size);
    for chunk in contents
        .into_iter()
        .filter_map(Result::ok)
        .chunks(buf_size)
        .into_iter()
    {
        buf.clear();
        buf.extend(chunk);

        db.save_bulk(&buf).await?;
        progress.inc(buf.len() as u64);
    }

    progress.finish();
    println!("Import complete!");

    Ok(())
}
