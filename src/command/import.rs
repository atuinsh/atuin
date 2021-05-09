use std::env;
use std::path::PathBuf;

use atuin_common::utils::uuid_v4;
use chrono::{TimeZone, Utc};
use directories::UserDirs;
use eyre::{eyre, Result};
use structopt::StructOpt;

use atuin_client::history::History;
use atuin_client::import::{bash::Bash, zsh::Zsh};
use atuin_client::{database::Database, import::resh::ReshEntry};
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

        match self {
            Self::Auto => {
                let shell = env::var("SHELL").unwrap_or_else(|_| String::from("NO_SHELL"));

                if shell.ends_with("/zsh") {
                    println!("Detected ZSH");
                    import_zsh(db).await
                } else {
                    println!("cannot import {} history", shell);
                    Ok(())
                }
            }

            Self::Zsh => import_zsh(db).await,
            Self::Bash => import_bash(db).await,
            Self::Resh => import_resh(db).await,
        }
    }
}

async fn import_resh(db: &mut (impl Database + Send + Sync)) -> Result<()> {
    let histpath = std::path::Path::new(std::env::var("HOME")?.as_str()).join(".resh_history.json");

    println!("Parsing .resh_history.json...");
    #[allow(clippy::filter_map)]
    let history = std::fs::read_to_string(histpath)?
        .split('\n')
        .map(str::trim)
        .map(|x| serde_json::from_str::<ReshEntry>(x))
        .filter_map(|x| match x {
            Ok(x) => Some(x),
            Err(e) => {
                if e.is_eof() {
                    None
                } else {
                    warn!("Invalid entry found in resh_history file: {}", e);
                    None
                }
            }
        })
        .map(|x| {
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            let timestamp = {
                let secs = x.realtime_before.floor() as i64;
                let nanosecs = (x.realtime_before.fract() * 1_000_000_000_f64).round() as u32;
                Utc.timestamp(secs, nanosecs)
            };
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            let duration = {
                let secs = x.realtime_after.floor() as i64;
                let nanosecs = (x.realtime_after.fract() * 1_000_000_000_f64).round() as u32;
                let difference = Utc.timestamp(secs, nanosecs) - timestamp;
                difference.num_nanoseconds().unwrap_or(0)
            };

            History {
                id: uuid_v4(),
                timestamp,
                duration,
                exit: x.exit_code,
                command: x.cmd_line,
                cwd: x.pwd,
                session: uuid_v4(),
                hostname: x.host,
            }
        })
        .collect::<Vec<_>>();
    println!("Updating database...");

    let progress = ProgressBar::new(history.len() as u64);

    let buf_size = 100;
    let mut buf = Vec::<_>::with_capacity(buf_size);

    for i in history {
        buf.push(i);

        if buf.len() == buf_size {
            db.save_bulk(&buf).await?;
            progress.inc(buf.len() as u64);

            buf.clear();
        }
    }

    if !buf.is_empty() {
        db.save_bulk(&buf).await?;
        progress.inc(buf.len() as u64);
    }
    Ok(())
}

async fn import_zsh(db: &mut (impl Database + Send + Sync)) -> Result<()> {
    // oh-my-zsh sets HISTFILE=~/.zhistory
    // zsh has no default value for this var, but uses ~/.zhistory.
    // we could maybe be smarter about this in the future :)

    let histpath = env::var("HISTFILE");

    let histpath = if let Ok(p) = histpath {
        let histpath = PathBuf::from(p);

        if !histpath.exists() {
            return Err(eyre!(
                "Could not find history file {:?}. try updating $HISTFILE",
                histpath
            ));
        }

        histpath
    } else {
        let user_dirs = UserDirs::new().unwrap();
        let home_dir = user_dirs.home_dir();

        let mut candidates = [".zhistory", ".zsh_history"].iter();
        loop {
            match candidates.next() {
                Some(candidate) => {
                    let histpath = home_dir.join(candidate);
                    if histpath.exists() {
                        break histpath;
                    }
                }
                None => return Err(eyre!("Could not find history file. try setting $HISTFILE")),
            }
        }
    };

    let zsh = Zsh::new(histpath)?;

    let progress = ProgressBar::new(zsh.loc);

    let buf_size = 100;
    let mut buf = Vec::<History>::with_capacity(buf_size);

    for i in zsh
        .filter_map(Result::ok)
        .filter(|x| !x.command.trim().is_empty())
    {
        buf.push(i);

        if buf.len() == buf_size {
            db.save_bulk(&buf).await?;
            progress.inc(buf.len() as u64);

            buf.clear();
        }
    }

    if !buf.is_empty() {
        db.save_bulk(&buf).await?;
        progress.inc(buf.len() as u64);
    }

    progress.finish();
    println!("Import complete!");

    Ok(())
}

// TODO: don't just copy paste this lol
async fn import_bash(db: &mut (impl Database + Send + Sync)) -> Result<()> {
    // oh-my-zsh sets HISTFILE=~/.zhistory
    // zsh has no default value for this var, but uses ~/.zhistory.
    // we could maybe be smarter about this in the future :)

    let histpath = env::var("HISTFILE");

    let histpath = if let Ok(p) = histpath {
        let histpath = PathBuf::from(p);

        if !histpath.exists() {
            return Err(eyre!(
                "Could not find history file {:?}. try updating $HISTFILE",
                histpath
            ));
        }

        histpath
    } else {
        let user_dirs = UserDirs::new().unwrap();
        let home_dir = user_dirs.home_dir();

        home_dir.join(".bash_history")
    };

    let bash = Bash::new(histpath)?;

    let progress = ProgressBar::new(bash.loc);

    let buf_size = 100;
    let mut buf = Vec::<History>::with_capacity(buf_size);

    for i in bash
        .filter_map(Result::ok)
        .filter(|x| !x.command.trim().is_empty())
    {
        buf.push(i);

        if buf.len() == buf_size {
            db.save_bulk(&buf).await?;
            progress.inc(buf.len() as u64);

            buf.clear();
        }
    }

    if !buf.is_empty() {
        db.save_bulk(&buf).await?;
        progress.inc(buf.len() as u64);
    }

    progress.finish();
    println!("Import complete!");

    Ok(())
}
