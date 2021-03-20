use eyre::Result;
use structopt::StructOpt;
use uuid::Uuid;

use crate::local::database::Database;
use crate::settings::Settings;

mod event;
mod history;
mod import;
mod init;
mod search;
mod server;
mod stats;

#[derive(StructOpt)]
pub enum AtuinCmd {
    #[structopt(
        about="manipulate shell history",
        aliases=&["h", "hi", "his", "hist", "histo", "histor"],
    )]
    History(history::Cmd),

    #[structopt(about = "import shell history from file")]
    Import(import::Cmd),

    #[structopt(about = "start an atuin server")]
    Server(server::Cmd),

    #[structopt(about = "calculate statistics for your history")]
    Stats(stats::Cmd),

    #[structopt(about = "output shell setup")]
    Init,

    #[structopt(about = "generates a UUID")]
    Uuid,

    #[structopt(about = "interactive history search")]
    Search { query: Vec<String> },
}

pub fn uuid_v4() -> String {
    Uuid::new_v4().to_simple().to_string()
}

impl AtuinCmd {
    pub fn run(self, db: &mut impl Database, settings: &Settings) -> Result<()> {
        match self {
            Self::History(history) => history.run(db),
            Self::Import(import) => import.run(db),
            Self::Server(server) => server.run(),
            Self::Stats(stats) => stats.run(db, settings),
            Self::Init => init::init(),
            Self::Search { query } => search::run(&query, db),

            Self::Uuid => {
                println!("{}", uuid_v4());
                Ok(())
            }
        }
    }
}
