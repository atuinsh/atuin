use std::path::PathBuf;

use eyre::{Result, WrapErr};
use structopt::clap::Shell;
use structopt::StructOpt;

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings as ClientSettings;
use atuin_common::utils::uuid_v4;
use atuin_server::settings::Settings as ServerSettings;

mod event;
mod history;
mod import;
mod init;
mod login;
mod logout;
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
    Init(init::Cmd),

    #[structopt(about = "generates a UUID")]
    Uuid,

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

    #[structopt(about = "sync with the configured server")]
    Sync {
        #[structopt(long, short, about = "force re-download everything")]
        force: bool,
    },

    #[structopt(about = "login to the configured server")]
    Login(login::Cmd),

    #[structopt(about = "log out")]
    Logout,

    #[structopt(about = "register with the configured server")]
    Register(register::Cmd),

    #[structopt(about = "print the encryption key for transfer to another machine")]
    Key,

    #[structopt(about = "generate shell completions")]
    GenCompletions {
        #[structopt(long, short, help = "set the shell for generating completions")]
        shell: Shell,

        #[structopt(long, short, help = "set the output directory")]
        out_dir: String,
    },
}

impl AtuinCmd {
    pub async fn run(self) -> Result<()> {
        let client_settings = ClientSettings::new().wrap_err("could not load client settings")?;
        let server_settings = ServerSettings::new().wrap_err("could not load server settings")?;

        let db_path = PathBuf::from(client_settings.db_path.as_str());

        let mut db = Sqlite::new(db_path).await?;

        match self {
            Self::History(history) => history.run(&client_settings, &mut db).await,
            Self::Import(import) => import.run(&mut db).await,
            Self::Server(server) => server.run(&server_settings).await,
            Self::Stats(stats) => stats.run(&mut db, &client_settings).await,
            Self::Init(init) => {
                init.run();
                Ok(())
            }
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

            Self::Sync { force } => sync::run(&client_settings, force, &mut db).await,
            Self::Login(l) => l.run(&client_settings).await,
            Self::Logout => {
                logout::run();
                Ok(())
            }
            Self::Register(r) => {
                register::run(&client_settings, &r.username, &r.email, &r.password).await
            }
            Self::Key => {
                use atuin_client::encryption::{encode_key, load_key};
                let key = load_key(&client_settings).wrap_err("could not load encryption key")?;
                let encode = encode_key(key).wrap_err("could not encode encryption key")?;
                println!("{}", encode);
                Ok(())
            }
            Self::Uuid => {
                println!("{}", uuid_v4());
                Ok(())
            }
            Self::GenCompletions { shell, out_dir } => {
                AtuinCmd::clap().gen_completions(env!("CARGO_PKG_NAME"), shell, &out_dir);
                println!(
                    "Shell completion for {} is generated in {:?}",
                    shell, out_dir
                );
                Ok(())
            }
        }
    }
}
