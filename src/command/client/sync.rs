use clap::Subcommand;
use eyre::{Result, WrapErr};

use atuin_client::{database::Database, settings::Settings};

mod login;
mod logout;
mod register;

#[derive(Subcommand)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Sync with the configured server
    Sync {
        /// Force re-download everything
        #[arg(long, short)]
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
}

impl Cmd {
    pub async fn run(self, settings: Settings, db: &mut impl Database) -> Result<()> {
        match self {
            Self::Sync { force } => run(&settings, force, db).await,
            Self::Login(l) => l.run(&settings).await,
            Self::Logout => logout::run(),
            Self::Register(r) => r.run(&settings).await,
            Self::Key => {
                use atuin_client::encryption::{encode_key, load_key};
                let key = load_key(&settings).wrap_err("could not load encryption key")?;
                let encode = encode_key(key).wrap_err("could not encode encryption key")?;
                println!("{encode}");
                Ok(())
            }
        }
    }
}

async fn run(settings: &Settings, force: bool, db: &mut impl Database) -> Result<()> {
    atuin_client::sync::sync(settings, force, db).await?;
    println!(
        "Sync complete! {} items in database, force: {}",
        db.history_count().await?,
        force
    );
    Ok(())
}
