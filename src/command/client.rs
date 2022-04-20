use clap::CommandFactory;
use clap::Subcommand;
use clap_complete::Shell;
use clap_complete::{generate, generate_to};
use eyre::{Result, WrapErr};

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_common::utils::uuid_v4;

mod event;
mod history;
mod import;
mod init;
mod login;
mod logout;
mod register;
mod search;
mod stats;
mod sync;
use std::path::PathBuf;

#[derive(Subcommand)]
#[clap(infer_subcommands = true)]
pub enum Cmd {
    /// Manipulate shell history
    #[clap(subcommand)]
    History(history::Cmd),

    /// Import shell history from file
    #[clap(subcommand)]
    Import(import::Cmd),

    /// Calculate statistics for your history
    #[clap(subcommand)]
    Stats(stats::Cmd),

    /// Output shell setup
    #[clap(subcommand)]
    Init(init::Cmd),

    /// Generate a UUID
    Uuid,

    /// Interactive history search
    Search(search::Cmd),

    /// Sync with the configured server
    Sync {
        /// Force re-download everything
        #[clap(long, short)]
        force: bool,
    },

    /// Login to the configured server
    Login(login::Cmd),

    /// Log out
    Logout,

    /// Register with the configured server
    Register(register::Cmd),

    /// Print the encryption key for transfer to another machine
    Key,

    /// Generate shell completions
    GenCompletions {
        /// Set the shell for generating completions
        #[clap(long, short)]
        shell: Shell,

        /// Set the output directory
        #[clap(long, short)]
        out_dir: Option<String>,
    },
}

impl Cmd {
    pub async fn run(self) -> Result<()> {
        pretty_env_logger::init();
    
        let settings = Settings::new().wrap_err("could not load client settings")?;

        let db_path = PathBuf::from(settings.db_path.as_str());
        let mut db = Sqlite::new(db_path).await?;

        match self {
            Self::History(history) => history.run(&settings, &mut db).await,
            Self::Import(import) => import.run(&mut db).await,
            Self::Stats(stats) => stats.run(&mut db, &settings).await,
            Self::Init(init) => {
                init.run();
                Ok(())
            }
            Self::Search(search) => search.run(&mut db, &settings).await,
            Self::Sync { force } => sync::run(&settings, force, &mut db).await,
            Self::Login(l) => l.run(&settings).await,
            Self::Logout => logout::run(),
            Self::Register(r) => r.run(&settings).await,
            Self::Key => {
                use atuin_client::encryption::{encode_key, load_key};
                let key = load_key(&settings).wrap_err("could not load encryption key")?;
                let encode = encode_key(key).wrap_err("could not encode encryption key")?;
                println!("{}", encode);
                Ok(())
            }
            Self::Uuid => {
                println!("{}", uuid_v4());
                Ok(())
            }
            Self::GenCompletions { shell, out_dir } => {
                let mut cli = crate::Atuin::command();

                match out_dir {
                    Some(out_dir) => {
                        generate_to(shell, &mut cli, env!("CARGO_PKG_NAME"), &out_dir)?;
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
