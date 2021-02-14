use std::env;
use std::path::PathBuf;

use eyre::{eyre, Result};
use home::home_dir;
use structopt::StructOpt;

use crate::local::database::{Database, Sqlite};
use crate::local::history::History;
use crate::local::import::Zsh;
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
}

impl Cmd {
    pub fn run(&self, db: &mut Sqlite) -> Result<()> {
        println!("        A'Tuin       ");
        println!("=====================");
        println!("          \u{1f30d}         ");
        println!("       \u{1f418}\u{1f418}\u{1f418}\u{1f418}      ");
        println!("          \u{1f422}         ");
        println!("=====================");
        println!("Importing history...");

        match self {
            Self::Auto => {
                let shell = env::var("SHELL").unwrap_or_else(|_| String::from("NO_SHELL"));

                if shell.as_str() == "/bin/zsh" {
                    println!("Detected ZSH");
                    import_zsh(db)
                } else {
                    println!("cannot import {} history", shell);
                    Ok(())
                }
            }

            Self::Zsh => import_zsh(db),
        }
    }
}

fn import_zsh(db: &mut Sqlite) -> Result<()> {
    // oh-my-zsh sets HISTFILE=~/.zhistory
    // zsh has no default value for this var, but uses ~/.zhistory.
    // we could maybe be smarter about this in the future :)

    let histpath = env::var("HISTFILE");

    let histpath = if let Ok(p) = histpath {
        PathBuf::from(p)
    } else {
        let mut home = home_dir().unwrap();
        home.push(".zhistory");

        home
    };

    if !histpath.exists() {
        return Err(eyre!(
            "Could not find history file at {}, try setting $HISTFILE",
            histpath.to_str().unwrap()
        ));
    }

    let zsh = Zsh::new(histpath.to_str().unwrap())?;

    let progress = ProgressBar::new(zsh.loc);

    let buf_size = 100;
    let mut buf = Vec::<History>::with_capacity(buf_size);

    for i in zsh {
        match i {
            Ok(h) => {
                buf.push(h);
            }
            Err(e) => {
                error!("{}", e);
                continue;
            }
        }

        if buf.len() == buf_size {
            db.save_bulk(&buf)?;
            progress.inc(buf.len() as u64);

            buf = Vec::<History>::with_capacity(buf_size);
        }
    }

    if !buf.is_empty() {
        db.save_bulk(&buf)?;
        progress.inc(buf.len() as u64);
    }

    progress.finish_with_message("Imported history!");

    Ok(())
}
