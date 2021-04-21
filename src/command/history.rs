use std::env;

use eyre::Result;
use structopt::StructOpt;

use atuin_client::database::Database;
use atuin_client::history::History;
use atuin_client::settings::Settings;
use atuin_client::sync;

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

    #[structopt(
        about="get the last command ran",
        aliases=&["la", "las"],
    )]
    Last {},
}

fn print_list(h: &[History]) {
    for i in h {
        println!("{}", i.command);
    }
}

impl Cmd {
    pub async fn run(&self, settings: &Settings, db: &mut (impl Database + Send)) -> Result<()> {
        match self {
            Self::Start { command: words } => {
                let command = words.join(" ");
                let cwd = env::current_dir()?.display().to_string();

                let h = History::new(chrono::Utc::now(), command, cwd, -1, -1, None, None);

                // print the ID
                // we use this as the key for calling end
                println!("{}", h.id);
                db.save(&h)?;
                Ok(())
            }

            Self::End { id, exit } => {
                if id.trim() == "" {
                    return Ok(());
                }

                let mut h = db.load(id)?;

                if h.duration > 0 {
                    debug!("cannot end history - already has duration");

                    // returning OK as this can occur if someone Ctrl-c a prompt
                    return Ok(());
                }

                h.exit = *exit;
                h.duration = chrono::Utc::now().timestamp_nanos() - h.timestamp.timestamp_nanos();

                db.update(&h)?;

                if settings.should_sync()? {
                    debug!("running periodic background sync");
                    sync::sync(settings, false, db).await?;
                } else {
                    debug!("sync disabled! not syncing");
                }

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
                    (false, false) => db.list(None, false)?,
                    (true, false) => db.query(QUERY_SESSION, &[session.as_str()])?,
                    (false, true) => db.query(QUERY_DIR, &[cwd.as_str()])?,
                    (true, true) => {
                        db.query(QUERY_SESSION_DIR, &[cwd.as_str(), session.as_str()])?
                    }
                };

                print_list(&history);

                Ok(())
            }

            Self::Search { query } => {
                let history = db.prefix_search(&query.join(""))?;
                print_list(&history);

                Ok(())
            }

            Self::Last {} => {
                let last = db.last()?;
                print_list(&[last]);

                Ok(())
            }
        }
    }
}
