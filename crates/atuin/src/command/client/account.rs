use std::convert::Infallible;
use std::io::Read;
use std::str::FromStr;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::utils::env_nonempty;
use clap::{Args, Subcommand};
use eyre::{Context, Result};
use tracing::instrument;

pub mod change_password;
pub mod delete;
pub mod link;
pub mod login;
pub mod logout;
pub mod register;

const PASSWORD_ENV: &str = "ATUIN_PASSWORD";

/// A `--password` value: the password itself, or `-` to read it from stdin.
#[derive(Clone, Debug)]
pub enum PasswordArg {
    Stdin,
    Value(String),
}

impl FromStr for PasswordArg {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "-" => Self::Stdin,
            password => Self::Value(password.to_owned()),
        })
    }
}

impl PasswordArg {
    /// Resolve `arg`, falling back to `ATUIN_PASSWORD`, if set.
    ///
    /// [`Self::Stdin`] reads `stdin` to its end and drops the trailing newline.
    pub fn resolve(arg: Option<&Self>, mut stdin: impl Read) -> Result<Option<String>> {
        let Some(arg) = arg else {
            return Ok(env_nonempty(PASSWORD_ENV).and_then(|password| password.into_string().ok()));
        };

        match arg {
            Self::Value(password) => Ok(Some(password.clone())),
            Self::Stdin => {
                let mut buf = String::new();
                stdin.read_to_string(&mut buf).context("failed to read password from stdin")?;
                Ok(Some(buf.trim_end_matches(['\r', '\n']).to_owned()))
            }
        }
    }
}

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
    Delete(delete::Cmd),

    /// Change your password
    ChangePassword(change_password::Cmd),

    /// Link your CLI sync account to your Hub account
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::value("hunter2", "stdin", "hunter2")]
    #[case::stdin("-", "hunter2\n", "hunter2")]
    #[case::stdin_crlf("-", "hunter2\r\n", "hunter2")]
    #[case::stdin_keeps_inner_whitespace("-", " hunter 2\n", " hunter 2")]
    fn resolve_reads_value_or_stdin(
        #[case] arg: &str,
        #[case] stdin: &str,
        #[case] expected: &str,
    ) {
        let arg: PasswordArg = arg.parse().unwrap();
        let password = PasswordArg::resolve(Some(&arg), stdin.as_bytes()).unwrap();
        assert_eq!(password.as_deref(), Some(expected));
    }
}
