use std::path::PathBuf;

use directories::ProjectDirs;
use eyre::{eyre, Result};
use structopt::StructOpt;
use uuid::Uuid;

#[macro_use]
extern crate log;
use pretty_env_logger;

use command::{history::HistoryCmd, import::ImportCmd};
use local::database::{Database, SqliteDatabase};
use local::history::History;

mod command;
mod local;

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

#[derive(StructOpt)]
enum AtuinCmd {
    #[structopt(
        about="manipulate shell history",
        aliases=&["h", "hi", "his", "hist", "histo", "histor"],
    )]
    History(HistoryCmd),

    #[structopt(about = "import shell history from file")]
    Import(ImportCmd),

    #[structopt(about = "start an atuin server")]
    Server,

    #[structopt(about = "generates a UUID")]
    Uuid,
}

impl Atuin {
    fn run(self) -> Result<()> {
        let db_path = match self.db {
            Some(db_path) => {
                let path = db_path
                    .to_str()
                    .ok_or(eyre!("path {:?} was not valid UTF-8", db_path))?;
                let path = shellexpand::full(path)?;
                PathBuf::from(path.as_ref())
            }
            None => {
                let project_dirs = ProjectDirs::from("com", "elliehuxtable", "atuin").ok_or(
                    eyre!("could not determine db file location\nspecify one using the --db flag"),
                )?;
                let root = project_dirs.data_dir();
                root.join("history.db")
            }
        };

        let mut db = SqliteDatabase::new(db_path)?;

        match self.atuin {
            AtuinCmd::History(history) => history.run(&mut db),
            AtuinCmd::Import(import) => import.run(&mut db),
            AtuinCmd::Uuid => {
                println!("{}", Uuid::new_v4().to_simple().to_string());
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn main() -> Result<()> {
    pretty_env_logger::init();

    Atuin::from_args().run()
}
