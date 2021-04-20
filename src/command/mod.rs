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
mod sync;

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
    Sync {
        #[structopt(long, short, about = "force re-download everything")]
        force: bool,
    },

    #[structopt(about = "login to the configured server")]
    Login(login::Cmd),

    #[structopt(about = "register with the configured server")]
    Register(register::Cmd),

    #[structopt(about = "print the encryption key for transfer to another machine")]
    Key,
}

pub fn uuid_v4() -> String {
    Uuid::new_v4().to_simple().to_string()
}

impl AtuinCmd {
    pub async fn run<T: Database + Send>(self, db: &mut T, settings: &Settings) -> Result<()> {
        match self {
            Self::History(history) => history.run(settings, db).await,
            Self::Import(import) => import.run(db),
            Self::Server(server) => server.run(settings).await,
            Self::Stats(stats) => stats.run(db, settings),
            Self::Init => init::init(),
            Self::Search { query } => search::run(&query, db),

            Self::Sync { force } => sync::run(settings, force, db).await,
            Self::Login(l) => l.run(settings),
            Self::Register(r) => register::run(
                settings,
                r.username.as_str(),
                r.email.as_str(),
                r.password.as_str(),
            ),
            Self::Key => {
                let key = std::fs::read(settings.local.key_path.as_str())?;
                println!("{}", base64::encode(key));
                Ok(())
            }

            Self::Uuid => {
                println!("{}", uuid_v4());
                Ok(())
            }
        }
    }
}
