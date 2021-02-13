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
    Import,

    #[structopt(about = "start a atuin server")]
    Server,
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

        let db = SqliteDatabase::new(db_path)?;

        match self.atuin {
            AtuinCmd::History(history) => history.run(db),
            _ => Ok(()),
        }
    }
}

#[derive(StructOpt)]
enum HistoryCmd {
    #[structopt(
        about="begins a new command in the history",
        aliases=&["s", "st", "sta", "star"],
    )]
    Start { command: Vec<String> },

    #[structopt(
        about="finishes a new command in the history (adds time, exit code)",
        aliases=&["e", "en"],
    )]
    End {
        id: String,
        #[structopt(long, short)]
        exit: i64,
    },

    #[structopt(
        about="list all items in history",
        aliases=&["l", "li", "lis"],
    )]
    List,
}

impl HistoryCmd {
    fn run(&self, db: SqliteDatabase) -> Result<()> {
        match self {
            HistoryCmd::Start { command: words } => {
                let command = words.join(" ");
                let cwd = env::current_dir()?.display().to_string();

                let h = History::new(command, cwd, -1, -1);

                // print the ID
                // we use this as the key for calling end
                println!("{}", h.id);
                db.save(h)?;
                Ok(())
            }

            HistoryCmd::End { id, exit } => {
                let mut h = db.load(id)?;
                h.exit = *exit;
                h.duration = chrono::Utc::now().timestamp_millis() - h.timestamp;

                db.update(h)?;

                Ok(())
            }

            HistoryCmd::List => db.list(),
        }
    }
}

fn main() -> Result<()> {
    pretty_env_logger::init();

    Atuin::from_args().run()
}
