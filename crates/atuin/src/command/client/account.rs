use clap::{Args, Subcommand};
use eyre::Result;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;

pub mod change_password;
pub mod delete;
pub mod login;
pub mod logout;
pub mod register;
pub mod verify;

#[derive(Args, Debug)]
pub struct Cmd {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Login to the configured server
    Login(login::Cmd),

    /// Register a new account
    Register(register::Cmd),

    /// Log out
    Logout,

    /// Delete your account, and all synced data
    Delete,

    /// Change your password
    ChangePassword(change_password::Cmd),

    /// Verify your account
    Verify(verify::Cmd),
}

impl Cmd {
    pub async fn run(self, settings: Settings, store: SqliteStore) -> Result<()> {
        match self.command {
            Commands::Login(l) => l.run(&settings, &store).await,
            Commands::Register(r) => r.run(&settings).await,
            Commands::Logout => logout::run(&settings),
            Commands::Delete => delete::run(&settings).await,
            Commands::ChangePassword(c) => c.run(&settings).await,
            Commands::Verify(c) => c.run(&settings).await,
        }
    }
}
