#[cfg(feature = "client")]
use std::path::PathBuf;

use eyre::Result;
use structopt::StructOpt;

#[cfg(feature = "client")]
use atuin_client::database::Sqlite;
#[cfg(feature = "client")]
use atuin_client::settings::Settings as ClientSettings;
use atuin_common::utils::uuid_v4;
#[cfg(feature = "server")]
use atuin_server::settings::Settings as ServerSettings;

#[cfg(feature = "client")]
mod event;
#[cfg(feature = "client")]
mod history;
#[cfg(feature = "client")]
mod import;
#[cfg(feature = "client")]
mod init;
#[cfg(feature = "sync")]
mod login;
#[cfg(feature = "sync")]
mod logout;
#[cfg(feature = "sync")]
mod register;
#[cfg(feature = "client")]
mod search;
#[cfg(feature = "server")]
mod server;
#[cfg(feature = "client")]
mod stats;
#[cfg(feature = "sync")]
mod sync;

#[derive(StructOpt)]
pub enum AtuinCmd {
    #[cfg(feature = "client")]
    #[structopt(
        about="manipulate shell history",
        aliases=&["h", "hi", "his", "hist", "histo", "histor"],
    )]
    History(history::Cmd),

    #[cfg(feature = "client")]
    #[structopt(about = "import shell history from file")]
    Import(import::Cmd),

    #[cfg(feature = "server")]
    #[structopt(about = "start an atuin server")]
    Server(server::Cmd),

    #[cfg(feature = "client")]
    #[structopt(about = "calculate statistics for your history")]
    Stats(stats::Cmd),

    #[cfg(feature = "client")]
    #[structopt(about = "output shell setup")]
    Init(init::Cmd),

    #[structopt(about = "generates a UUID")]
    Uuid,

    #[cfg(feature = "client")]
    #[structopt(about = "interactive history search")]
    Search {
        #[structopt(long, short, about = "filter search result by directory")]
        cwd: Option<String>,

        #[structopt(long = "exclude-cwd", about = "exclude directory from results")]
        exclude_cwd: Option<String>,

        #[structopt(long, short, about = "filter search result by exit code")]
        exit: Option<i64>,

        #[structopt(long = "exclude-exit", about = "exclude results with this exit code")]
        exclude_exit: Option<i64>,

        #[structopt(long, short, about = "only include results added before this date")]
        before: Option<String>,

        #[structopt(long, about = "only include results after this date")]
        after: Option<String>,

        #[structopt(long, short, about = "open interactive search UI")]
        interactive: bool,

        #[structopt(long, short, about = "use human-readable formatting for time")]
        human: bool,

        query: Vec<String>,

        #[structopt(long, about = "Show only the text of the command")]
        cmd_only: bool,
    },

    #[cfg(feature = "sync")]
    #[structopt(about = "sync with the configured server")]
    Sync {
        #[structopt(long, short, about = "force re-download everything")]
        force: bool,
    },

    #[cfg(feature = "sync")]
    #[structopt(about = "login to the configured server")]
    Login(login::Cmd),

    #[cfg(feature = "sync")]
    #[structopt(about = "log out")]
    Logout,

    #[cfg(feature = "sync")]
    #[structopt(about = "register with the configured server")]
    Register(register::Cmd),

    #[cfg(feature = "sync")]
    #[structopt(about = "print the encryption key for transfer to another machine")]
    Key,
}

impl AtuinCmd {
    pub async fn run(self) -> Result<()> {
        #[cfg(feature = "client")]
        let client_settings = ClientSettings::new()?;
        #[cfg(feature = "server")]
        let server_settings = ServerSettings::new()?;

        #[cfg(feature = "client")]
        let db_path = PathBuf::from(client_settings.db_path.as_str());

        #[cfg(feature = "client")]
        let mut db = Sqlite::new(db_path).await?;

        match self {
            #[cfg(feature = "sync")]
            Self::History(history) => history.run(&client_settings, &mut db).await,
            #[cfg(all(feature = "client", not(feature = "sync")))]
            Self::History(history) => history.run(&mut db).await,
            #[cfg(feature = "client")]
            Self::Import(import) => import.run(&mut db).await,
            #[cfg(feature = "server")]
            Self::Server(server) => server.run(&server_settings).await,
            #[cfg(feature = "client")]
            Self::Stats(stats) => stats.run(&mut db, &client_settings).await,
            #[cfg(feature = "client")]
            Self::Init(init) => {
                init.run();
                Ok(())
            }
            #[cfg(feature = "client")]
            Self::Search {
                cwd,
                exit,
                interactive,
                human,
                exclude_exit,
                exclude_cwd,
                before,
                after,
                query,
                cmd_only,
            } => {
                search::run(
                    &client_settings,
                    cwd,
                    exit,
                    interactive,
                    human,
                    exclude_exit,
                    exclude_cwd,
                    before,
                    after,
                    cmd_only,
                    &query,
                    &mut db,
                )
                .await
            }

            #[cfg(feature = "sync")]
            Self::Sync { force } => sync::run(&client_settings, force, &mut db).await,
            #[cfg(feature = "sync")]
            Self::Login(l) => l.run(&client_settings),
            #[cfg(feature = "sync")]
            Self::Logout => {
                logout::run();
                Ok(())
            }
            #[cfg(feature = "sync")]
            Self::Register(r) => {
                register::run(&client_settings, &r.username, &r.email, &r.password)
            }
            #[cfg(feature = "sync")]
            Self::Key => {
                let key = atuin_client::encryption::load_key(&client_settings)?;
                println!("{}", atuin_client::encryption::encode_key(key)?);
                Ok(())
            }

            Self::Uuid => {
                println!("{}", uuid_v4());
                Ok(())
            }
        }
    }
}
