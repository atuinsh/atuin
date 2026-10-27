//! The `atuin-server admin user` commands.

use std::io::{self, IsTerminal as _};

use atuin_common::userio;
use atuin_common::userio::{Confirm, DefaultAnswer, Secret};
use atuin_common::utils::crypto_random_string;
use clap::Subcommand;
use eyre::{Context as _, Result, bail};
use secrecy::{ExposeSecret as _, SecretString};

use crate::db::models::{HashedPassword, NewUser, NewUsername, User};
use crate::db::{DbError, DynDatabase};

#[derive(Subcommand, Debug)]
pub enum UserCmd {
    /// Create a user, with the password read from a prompt or from stdin
    ///
    /// Works even when open_registration is false.
    Create {
        /// The new user's name, of letters, digits and hyphens (-)
        username: NewUsername,

        /// The new user's email
        #[clap(long)]
        email: String,
    },

    /// Set a user's password, read from a prompt or from stdin
    ///
    /// The user's machines stay logged in.
    SetPassword {
        /// The user to change
        username: String,
    },

    /// Delete a user with their history and synced records
    Delete {
        /// The user to delete
        username: String,

        /// Delete without asking for confirmation
        #[clap(long)]
        yes: bool,
    },
}

impl UserCmd {
    pub(super) async fn run(self, db: &dyn DynDatabase) -> Result<()> {
        match self {
            Self::Create { username, email } => create(db, username, email).await,
            Self::SetPassword { username } => set_password(db, &username).await,
            Self::Delete { username, yes } => delete(db, &username, yes).await,
        }
    }
}

async fn create(db: &dyn DynDatabase, username: NewUsername, email: String) -> Result<()> {
    let user = NewUser {
        password: HashedPassword::new(read_password()?).await,
        username,
        email,
    };
    // Login hands out the user's existing session rather than minting one, so the user needs one
    // from the start, as on registration.
    db.add_user_with_session(&user, &crypto_random_string::<24>())
        .await
        .wrap_err_with(|| format!("failed to create {}", user.username))?;

    userio::log::info!("created {}", user.username);
    Ok(())
}

async fn set_password(db: &dyn DynDatabase, username: &str) -> Result<()> {
    let mut user = get_user(db, username).await?;
    user.password = HashedPassword::new(read_password()?).await;
    db.update_user_password(&user).await.wrap_err("failed to update the password")?;

    userio::log::info!("changed the password of {username}");
    Ok(())
}

async fn delete(db: &dyn DynDatabase, username: &str, yes: bool) -> Result<()> {
    let user = get_user(db, username).await?;
    if !yes {
        confirm_delete(username)?;
    }
    db.delete_user(&user).await.wrap_err("failed to delete the user")?;

    userio::log::info!("deleted {username}");
    Ok(())
}

async fn get_user(db: &dyn DynDatabase, username: &str) -> Result<User> {
    match db.get_user(username).await {
        Ok(user) => Ok(user),
        Err(DbError::NotFound) => bail!("no user named {username}"),
        Err(DbError::Other(err)) => Err(err.wrap_err(format!("failed to look up {username}"))),
    }
}

/// Read a password from a no-echo prompt, typed twice, or from the first line of stdin when it is
/// not a terminal.
fn read_password() -> Result<SecretString> {
    let password = Secret::new("New password:")
        .ask()
        .wrap_err("failed to read the password")?
        .unwrap_or_default();

    if io::stdin().is_terminal() {
        let repeated = Secret::new("Repeat the new password:")
            .ask()
            .wrap_err("failed to read the password")?
            .unwrap_or_default();
        if password.expose_secret() != repeated.expose_secret() {
            bail!("the passwords do not match");
        }
    }

    if password.expose_secret().is_empty() {
        bail!("the password is empty");
    }
    Ok(password)
}

fn confirm_delete(username: &str) -> Result<()> {
    if !io::stdin().is_terminal() {
        bail!(
            "not deleting {username} without confirmation; pass --yes when stdin is not a terminal"
        );
    }

    let question = format!(
        "Delete {username} with all their history and synced records? This cannot be undone."
    );
    if !Confirm::new(&question, DefaultAnswer::No)
        .ask()
        .wrap_err("failed to read the confirmation")?
    {
        bail!("not deleting {username}");
    }
    Ok(())
}
