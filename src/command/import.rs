use std::env;
use std::path::PathBuf;

use eyre::{eyre, Result};
use home::home_dir;
use structopt::StructOpt;

use crate::local::database::{Database, SqliteDatabase};
use crate::local::history::History;
use crate::local::import::ImportZsh;
use indicatif::ProgressBar;

#[derive(StructOpt)]
pub enum ImportCmd {
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

impl ImportCmd {
    fn import_zsh(&self, db: &mut SqliteDatabase) -> Result<()> {
        // oh-my-zsh sets HISTFILE=~/.zhistory
        // zsh has no default value for this var, but uses ~/.zhistory.
        // we could maybe be smarter about this in the future :)

        let histpath = env::var("HISTFILE");

        let histpath = match histpath {
            Ok(p) => PathBuf::from(p),
            Err(_) => {
                let mut home = home_dir().unwrap();
                home.push(".zhistory");

                home
            }
        };

        if !histpath.exists() {
            return Err(eyre!(
                "Could not find history file at {}, try setting $HISTFILE",
                histpath.to_str().unwrap()
            ));
        }

        let zsh = ImportZsh::new(histpath.to_str().unwrap())?;

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

        if buf.len() > 0 {
            db.save_bulk(&buf)?;
            progress.inc(buf.len() as u64);
        }

        progress.finish_with_message("Imported history!");

        Ok(())
    }

    pub fn run(&self, db: &mut SqliteDatabase) -> Result<()> {
        match self {
            ImportCmd::Auto => {
                let shell = env::var("SHELL").unwrap_or(String::from("NO_SHELL"));

                match shell.as_str() {
                    "/bin/zsh" => self.import_zsh(db),

                    _ => {
                        println!("cannot import {} history", shell);
                        Ok(())
                    }
                }
            }

            ImportCmd::Zsh => Ok(()),
        }
    }
}
