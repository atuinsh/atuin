use std::{env, path::PathBuf};

use eyre::{eyre, Result};
use structopt::StructOpt;

use atuin_client::import::{bash::Bash, zsh::Zsh};
use atuin_client::{database::Database, import::Importer};
use atuin_client::{history::History, import::resh::Resh};
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

const BATCH_SIZE: usize = 100;

impl Cmd {
    pub async fn run(&self, db: &mut (impl Database + Send + Sync)) -> Result<()> {
        println!("        Atuin         ");
        println!("======================");
        println!("          \u{1f30d}          ");
        println!("       \u{1f418}\u{1f418}\u{1f418}\u{1f418}       ");
        println!("          \u{1f422}          ");
        println!("======================");
        println!("Importing history...");

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

async fn import<I: Importer + Send, DB: Database + Send + Sync>(
    db: &mut DB,
    buf_size: usize,
) -> Result<()>
where
    I::IntoIter: Send,
{
    println!("Importing history from {}", I::NAME);

    let histpath = if let Ok(p) = env::var("HISTFILE") {
        let histpath = PathBuf::from(p);

        if !histpath.is_file() {
            return Err(eyre!(
                "Could not find history file {:?}. Try updating $HISTFILE",
                histpath
            ));
        }

        histpath
    } else {
        let histpath = I::histpath()?;

        if !histpath.is_file() {
            return Err(eyre!(
                "Could not find history file {:?}. Try setting $HISTFILE",
                histpath
            ));
        }

        histpath
    };

    let contents = I::parse(histpath)?;

    let iter = contents.into_iter();
    let progress = if let (_, Some(upper_bound)) = iter.size_hint() {
        ProgressBar::new(upper_bound as u64)
    } else {
        ProgressBar::new_spinner()
    };

    let mut buf = Vec::<History>::with_capacity(buf_size);
    let mut iter = progress.wrap_iter(iter);
    loop {
        // clear buffer
        buf.clear();

        // fill until either no more entries
        // or until the buffer is full
        let done = loop {
            match iter.next() {
                Some(Ok(hist)) => buf.push(hist),
                Some(Err(_)) => (),
                None => break true,
            }

            if buf.len() == buf_size {
                break false;
            }
        };

        // flush
        db.save_bulk(&buf).await?;

        if done {
            break;
        }
    }

    progress.finish();
    println!("Import complete!");

    Ok(())
}
