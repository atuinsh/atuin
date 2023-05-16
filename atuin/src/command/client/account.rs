use clap::Subcommand;
use eyre::{Result, WrapErr};

use atuin_client::{database::Database, settings::Settings};

pub mod login;
pub mod logout;
pub mod register;

#[derive(Subcommand)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Login to the configured server
    Login(login::Cmd),

    Register(register::Cmd),

    /// Log out
    Logout,
}

impl Cmd {
    pub async fn run(self, settings: Settings, db: &mut impl Database) -> Result<()> {
        match self {
            Self::Login(l) => l.run(&settings).await,
            Self::Register(r) => r.run(&settings).await,
            Self::Logout => logout::run(&settings),
        }
    }
}
