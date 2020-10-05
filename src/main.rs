use std::env;

use structopt::StructOpt;
use eyre::Result;

#[macro_use] extern crate log;
use pretty_env_logger;

mod local;

use local::history::History;
use local::database::{Database, SqliteDatabase};

#[derive(StructOpt)]
#[structopt(
    author="Ellie Huxtable <e@elm.sh>",
    version="0.1.0",
    about="Keep your shell history in sync"
)]
enum Shync {
    #[structopt(
        about="manipulate shell history",
        aliases=&["h", "hi", "his", "hist", "histo", "histor"],
    )]
    History(HistoryCmd),

    #[structopt(
        about="import shell history from file",
    )]
    Import,

    #[structopt(
        about="start a shync server",
    )]
    Server,
}

impl Shync {
    fn run(self, db: SqliteDatabase) -> Result<()> {
        match self {
            Shync::History(history) => history.run(db),
            _ => Ok(())
        }
    }
}

#[derive(StructOpt)]
enum HistoryCmd {
    #[structopt(
        about="add a new command to the history",
        aliases=&["a", "ad"],
    )]
    Add {
        command: Vec<String>,
    },

    #[structopt(
        about="list all items in history",
        aliases=&["l", "li", "lis"],
    )]
    List,
}

impl HistoryCmd {
    fn run(self, db: SqliteDatabase) -> Result<()> {
        match self {
            HistoryCmd::Add{command: words} => {
                let command = words.join(" ");

                let cwd = env::current_dir()?;
                let h = History::new(
                    command.as_str(),
                    cwd.display().to_string().as_str(),
                );

                debug!("adding history: {:?}", h);
                db.save(h)?;
                debug!("saved history to sqlite");
                Ok(())
            }

            HistoryCmd::List => db.list()
        }
    }
}

fn main() -> Result<()> {
    pretty_env_logger::init();

    let db = SqliteDatabase::new("~/.history.db")?;
    Shync::from_args().run(db)
}
