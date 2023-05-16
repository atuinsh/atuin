use clap::{Args, Subcommand};
use eyre::Result;

use atuin_client::settings::Settings;

pub mod delete;
pub mod login;
pub mod logout;
pub mod register;

#[derive(Args)]
pub struct Cmd {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Login to the configured server
    Login(login::Cmd),

    // Register a new account
    Register(register::Cmd),

    /// Log out
    Logout,

    // Delete your account, and all synced data
    Delete,
}

impl Cmd {
    pub async fn run(self, settings: Settings) -> Result<()> {
        match self.command {
            Commands::Login(l) => l.run(&settings).await,
            Commands::Register(r) => r.run(&settings).await,
            Commands::Logout => logout::run(&settings),
            Commands::Delete => delete::run(&settings).await,
        }
    }
}
