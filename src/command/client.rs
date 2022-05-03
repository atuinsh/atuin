use std::path::PathBuf;

use clap::{CommandFactory, Subcommand};
use clap_complete::{generate, generate_to, Shell};
use eyre::{Result, WrapErr};

use atuin_client::{database::Sqlite, settings::Settings};
use atuin_common::utils::uuid_v4;

#[cfg(feature = "sync")]
mod sync;

mod event;
mod history;
mod import;
mod init;
mod search;
mod stats;

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
    Stats(stats::Cmd),

    /// Output shell setup
    #[clap(subcommand)]
    Init(init::Cmd),

    /// Generate a UUID
    Uuid,

    /// Interactive history search
    Search(search::Cmd),

    /// Generate shell completions
    GenCompletions {
        /// Set the shell for generating completions
        #[clap(long, short)]
        shell: Shell,

        /// Set the output directory
        #[clap(long, short)]
        out_dir: Option<String>,
    },

    #[cfg(feature = "sync")]
    #[clap(flatten)]
    Sync(sync::Cmd),
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
            #[cfg(feature = "sync")]
            Self::Sync(sync) => sync.run(settings, &mut db).await,
        }
    }
}
