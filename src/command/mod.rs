use eyre::Result;
use structopt::StructOpt;
use uuid::Uuid;

use crate::local::database::Database;
use crate::settings::Settings;

mod event;
mod history;
mod import;
mod init;
mod login;
mod register;
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

    #[structopt(about = "sync with the configured server")]
    Sync,

    #[structopt(about = "login to the configured server")]
    Login(login::Cmd),

    #[structopt(about = "register with the configured server")]
    Register(register::Cmd),
}

pub fn uuid_v4() -> String {
    Uuid::new_v4().to_simple().to_string()
}

impl AtuinCmd {
    pub fn run(self, db: &mut impl Database, settings: &Settings) -> Result<()> {
        match self {
            Self::History(history) => history.run(db),
            Self::Import(import) => import.run(db),
            Self::Server(server) => server.run(settings),
            Self::Stats(stats) => stats.run(db, settings),
            Self::Init => init::init(),
            Self::Search { query } => search::run(&query, db),

            Self::Sync => Ok(()),
            Self::Login(l) => login::run(settings, l.username, l.password),
            Self::Register(r) => register::run(settings, r.username, r.email, r.password),

            Self::Uuid => {
                println!("{}", uuid_v4());
                Ok(())
            }
        }
    }
}
