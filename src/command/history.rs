use std::env;

use eyre::Result;
use structopt::StructOpt;

use crate::local::database::{Database, QueryParam};
use crate::local::history::History;

#[derive(StructOpt)]
pub enum Cmd {
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
    List {
        #[structopt(long, short)]
        cwd: bool,

        #[structopt(long, short)]
        session: bool,
    },

    #[structopt(
        about="search for a command",
        aliases=&["se", "sea", "sear", "searc"],
    )]
    Search { query: Vec<String> },
}

fn print_list(h: &[History]) {
    for i in h {
        println!("{}", i.command);
    }
}

impl Cmd {
    pub fn run(&self, db: &mut impl Database) -> Result<()> {
        match self {
            Self::Start { command: words } => {
                let command = words.join(" ");
                let cwd = env::current_dir()?.display().to_string();

                let h = History::new(
                    chrono::Utc::now().timestamp_nanos(),
                    command,
                    cwd,
                    -1,
                    -1,
                    None,
                    None,
                );

                // print the ID
                // we use this as the key for calling end
                println!("{}", h.id);
                db.save(&h)?;
                Ok(())
            }

            Self::End { id, exit } => {
                let mut h = db.load(id)?;
                h.exit = *exit;
                h.duration = chrono::Utc::now().timestamp_nanos() - h.timestamp;

                db.update(&h)?;

                Ok(())
            }

            Self::List { session, cwd, .. } => {
                const QUERY_SESSION: &str = "select * from history where session = ?;";
                const QUERY_DIR: &str = "select * from history where cwd = ?;";
                const QUERY_SESSION_DIR: &str =
                    "select * from history where cwd = ?1 and session = ?2;";

                let params = (session, cwd);

                let cwd = env::current_dir()?.display().to_string();
                let session = env::var("ATUIN_SESSION")?;

                let history = match params {
                    (false, false) => db.list()?,
                    (true, false) => db.query(QUERY_SESSION, &[QueryParam::Text(session)])?,
                    (false, true) => db.query(QUERY_DIR, &[QueryParam::Text(cwd)])?,
                    (true, true) => db.query(
                        QUERY_SESSION_DIR,
                        &[QueryParam::Text(cwd), QueryParam::Text(session)],
                    )?,
                };

                print_list(&history);

                Ok(())
            }

            Self::Search { query } => {
                let history = db.prefix_search(&query.join(""))?;
                print_list(&history);

                Ok(())
            }
        }
    }
}
