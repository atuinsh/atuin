//! The `atuin-server admin` commands, for operators managing the server by hand.

mod user;

use clap::Subcommand;
use eyre::{Context as _, Result};
pub use user::UserCmd;

use crate::db::DbSettings;

#[derive(Subcommand, Debug)]
pub enum AdminCmd {
    /// Manage user accounts
    #[clap(subcommand)]
    User(UserCmd),
}

impl AdminCmd {
    /// Run the command against the database in `db_settings`.
    ///
    /// # Errors
    /// If the database is unreachable or rejects the change, the user is missing, or the
    /// confirmation or the new password is rejected.
    pub async fn run(self, db_settings: &DbSettings) -> Result<()> {
        let db =
            crate::connect(db_settings.connect_url()?).await.wrap_err("failed to connect to db")?;

        match self {
            Self::User(cmd) => cmd.run(db.as_ref()).await,
        }
    }
}
