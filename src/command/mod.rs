use std::path::PathBuf;

use clap::{IntoApp, Subcommand};
use eyre::{Result, WrapErr};

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings as ClientSettings;
use atuin_common::utils::uuid_v4;
use atuin_server::settings::Settings as ServerSettings;
use clap_generate::{generate, generate_to, Generator, Shell};

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

#[derive(Subcommand)]
pub enum AtuinCmd {
    /// manipulate shell history
    #[clap(subcommand)]
    History(history::Cmd),

    /// import shell history from file
    #[clap(subcommand)]
    Import(import::Cmd),

    /// start an atuin server
    #[clap(subcommand)]
    Server(server::Cmd),

    /// calculate statistics for your history
    #[clap(subcommand)]
    Stats(stats::Cmd),

    /// output shell setup
    #[clap(subcommand)]
    Init(init::Cmd),

    /// generates a UUID
    Uuid,

    /// interactive history search
    Search {
        ///filter search result by directory
        #[clap(long, short)]
        cwd: Option<String>,

        ///exclude directory from results
        #[clap(long = "exclude-cwd")]
        exclude_cwd: Option<String>,

        ///filter search result by exit code
        #[clap(long, short)]
        exit: Option<i64>,

        ///exclude results with this exit code
        #[clap(long = "exclude-exit")]
        exclude_exit: Option<i64>,

        ///only include results added before this date
        #[clap(long, short)]
        before: Option<String>,

        ///only include results after this date
        #[clap(long)]
        after: Option<String>,

        ///open interactive search UI
        #[clap(long, short)]
        interactive: bool,

        ///use human-readable formatting for time
        #[clap(long)]
        human: bool,

        query: Vec<String>,

        ///Show only the text of the command
        #[clap(long)]
        cmd_only: bool,
    },

    /// sync with the configured server
    Sync {
        ///force re-download everything
        #[clap(long, short)]
        force: bool,
    },

    /// login to the configured server
    Login(login::Cmd),

    /// log out
    Logout,

    /// register with the configured server
    Register(register::Cmd),

    /// print the encryption key for transfer to another machine
    Key,

    /// generate shell completions
    GenCompletions {
        /// set the shell for generating completions
        #[clap(long, short)]
        shell: Shell,

        /// set the output directory
        #[clap(long, short)]
        out_dir: Option<String>,
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
                let mut cli = crate::Atuin::into_app();
                match out_dir {
                    Some(out_dir) => {
                        generate_to(shell, &mut cli, env!("CARGO_PKG_NAME"), &out_dir)?;

                        println!(
                            "Shell completion for {} is generated in {:?}",
                            shell, out_dir
                        );
                    }
                    None => {
                        generate(
                            shell,
                            &mut cli,
                            env!("CARGO_PKG_NAME"),
                            &mut std::io::stdout(),
                        );
                    }
                }

                Ok(())
            }
        }
    }
}
