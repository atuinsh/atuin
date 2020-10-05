use std::env;
use std::path::PathBuf;

use directories::ProjectDirs;
use eyre::{eyre, Result};
use structopt::StructOpt;

#[macro_use]
extern crate log;
use pretty_env_logger;

mod local;

use local::database::{Database, SqliteDatabase};
use local::history::History;

#[derive(StructOpt)]
#[structopt(
    author = "Ellie Huxtable <e@elm.sh>",
    version = "0.1.0",
    about = "Keep your shell history in sync"
)]
struct Shync {
    #[structopt(long, parse(from_os_str), about = "db file path")]
    db: Option<PathBuf>,

    #[structopt(subcommand)]
    shync: ShyncCmd,
}

#[derive(StructOpt)]
enum ShyncCmd {
    #[structopt(
        about="manipulate shell history",
        aliases=&["h", "hi", "his", "hist", "histo", "histor"],
    )]
    History(HistoryCmd),

    #[structopt(about = "import shell history from file")]
    Import,

    #[structopt(about = "start a shync server")]
    Server,
}

impl Shync {
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
                let project_dirs = ProjectDirs::from("bike", "ellie", "shync").ok_or(eyre!(
                    "could not determine db file location\nspecify one using the --db flag"
                ))?;
                let root = project_dirs.data_dir();
                root.join("history.db")
            }
        };

        let db = SqliteDatabase::new(db_path)?;

        match self.shync {
            ShyncCmd::History(history) => history.run(db),
            _ => Ok(()),
        }
    }
}

#[derive(StructOpt)]
enum HistoryCmd {
    #[structopt(
        about="add a new command to the history",
        aliases=&["a", "ad"],
    )]
    Add { command: Vec<String> },

    #[structopt(
        about="list all items in history",
        aliases=&["l", "li", "lis"],
    )]
    List,
}

impl HistoryCmd {
    fn run(self, db: SqliteDatabase) -> Result<()> {
        match self {
            HistoryCmd::Add { command: words } => {
                let command = words.join(" ");
                let cwd = env::current_dir()?.display().to_string();
                let h = History::new(command, cwd);

                debug!("adding history: {:?}", h);
                db.save(h)?;
                debug!("saved history to sqlite");
                Ok(())
            }

            HistoryCmd::List => db.list(),
        }
    }
}

fn main() -> Result<()> {
    pretty_env_logger::init();

    Shync::from_args().run()
}
