use std::{env, path::PathBuf};

use atuin_client::import::fish::Fish;
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

    #[structopt(
        about="import history from the fish history file",
        aliases=&["f", "fi", "fis"],
    )]
    Fish,
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
                    import::<Zsh<_>, _>(db, BATCH_SIZE).await
                } else if shell.ends_with("/fish") {
                    println!("Detected Fish");
                    import::<Fish<_>, _>(db, BATCH_SIZE).await
                } else {
                    println!("cannot import {} history", shell);
                    Ok(())
                }
            }

            Self::Zsh => import::<Zsh<_>, _>(db, BATCH_SIZE).await,
            Self::Bash => import::<Bash<_>, _>(db, BATCH_SIZE).await,
            Self::Resh => import::<Resh, _>(db, BATCH_SIZE).await,
            Self::Fish => import::<Fish<_>, _>(db, BATCH_SIZE).await,
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

    let histpath = get_histpath::<I>()?;
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
        // fill until either no more entries
        // or until the buffer is full
        let done = fill_buf(&mut buf, &mut iter);

        // flush
        db.save_bulk(&buf).await?;

        if done {
            break;
        }
    }

    println!("Import complete!");

    Ok(())
}

fn get_histpath<I: Importer>() -> Result<PathBuf> {
    if let Ok(p) = env::var("HISTFILE") {
        is_file(PathBuf::from(p))
    } else {
        is_file(I::histpath()?)
    }
}

fn is_file(p: PathBuf) -> Result<PathBuf> {
    if p.is_file() {
        Ok(p)
    } else {
        Err(eyre!(
            "Could not find history file {:?}. Try setting $HISTFILE",
            p
        ))
    }
}

fn fill_buf<T, E>(buf: &mut Vec<T>, iter: &mut impl Iterator<Item = Result<T, E>>) -> bool {
    buf.clear();
    loop {
        match iter.next() {
            Some(Ok(t)) => buf.push(t),
            Some(Err(_)) => (),
            None => break true,
        }

        if buf.len() == buf.capacity() {
            break false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fill_buf;

    #[test]
    fn test_fill_buf() {
        let mut buf = Vec::with_capacity(4);
        let mut iter = vec![
            Ok(1),
            Err(2),
            Ok(3),
            Ok(4),
            Err(5),
            Ok(6),
            Ok(7),
            Err(8),
            Ok(9),
        ]
        .into_iter();

        assert!(!fill_buf(&mut buf, &mut iter));
        assert_eq!(buf, vec![1, 3, 4, 6]);

        assert!(fill_buf(&mut buf, &mut iter));
        assert_eq!(buf, vec![7, 9]);
    }
}
