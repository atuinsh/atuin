use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use clap::{Args, Subcommand};
use eyre::Result;
use tracing::instrument;

use crate::i18n::fl;

pub mod change_password;
pub mod delete;
pub mod link;
pub mod login;
pub mod logout;
pub mod register;

#[derive(Args, Debug)]
pub struct Cmd {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    #[command(about = fl!("cmd-login"))]
    Login(login::Cmd),

    #[command(about = fl!("cmd-account-register"))]
    Register(register::Cmd),

    #[command(about = fl!("cmd-logout"))]
    Logout,

    #[command(about = fl!("cmd-account-delete"))]
    Delete(delete::Cmd),

    #[command(about = fl!("cmd-account-change-password"))]
    ChangePassword(change_password::Cmd),

    #[command(about = fl!("cmd-account-link"))]
    Link,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: Settings, store: SqliteStore) -> Result<()> {
        match self.command {
            Commands::Login(l) => l.run(&settings, &store).await,
            Commands::Register(r) => r.run(&settings, &store).await,
            Commands::Logout => logout::run().await,
            Commands::Delete(d) => d.run(&settings).await,
            Commands::ChangePassword(c) => c.run(&settings).await,
            Commands::Link => link::run(&settings).await,
        }
    }
}
