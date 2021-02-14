#![feature(str_split_once)]
#![feature(proc_macro_hygiene)]
#![feature(decl_macro)]
#![warn(clippy::pedantic, clippy::nursery)]

use std::path::PathBuf;

use directories::ProjectDirs;
use eyre::{eyre, Result};
use structopt::StructOpt;
use uuid::Uuid;

#[macro_use]
extern crate log;

#[macro_use]
extern crate rocket;

use command::AtuinCmd;
use local::database::Sqlite;

mod command;
mod local;
mod remote;

#[derive(StructOpt)]
#[structopt(
    author = "Ellie Huxtable <e@elm.sh>",
    version = "0.1.0",
    about = "Keep your shell history in sync"
)]
struct Atuin {
    #[structopt(long, parse(from_os_str), help = "db file path")]
    db: Option<PathBuf>,

    #[structopt(subcommand)]
    atuin: AtuinCmd,
}

impl Atuin {
    fn run(self) -> Result<()> {
        let db_path = if let Some(db_path) = self.db {
            let path = db_path
                .to_str()
                .ok_or_else(|| eyre!("path {:?} was not valid UTF-8", db_path))?;
            let path = shellexpand::full(path)?;
            PathBuf::from(path.as_ref())
        } else {
            let project_dirs =
                ProjectDirs::from("com", "elliehuxtable", "atuin").ok_or_else(|| {
                    eyre!("could not determine db file location\nspecify one using the --db flag")
                })?;
            let root = project_dirs.data_dir();
            root.join("history.db")
        };

        let mut db = Sqlite::new(db_path)?;

        match self.atuin {
            AtuinCmd::History(history) => history.run(&mut db),
            AtuinCmd::Import(import) => import.run(&mut db),
            AtuinCmd::Server(server) => server.run(),

            AtuinCmd::Uuid => {
                println!("{}", Uuid::new_v4().to_simple().to_string());
                Ok(())
            }
        }
    }
}

fn main() -> Result<()> {
    pretty_env_logger::init();

    Atuin::from_args().run()
}
