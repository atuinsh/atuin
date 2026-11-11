//! Running the `hermes` CLI for the setup steps that should go through Hermes's own writers:
//! `plugins enable` (which also loads the plugin into a running gateway) and `config set`
//! (which keeps the comments in `config.yaml`).

use std::path::Path;
use std::process::Stdio;

use super::home::InstallHome;
use crate::executable::resolve_in_path;

/// Runs `hermes` with arguments, against one Hermes home. A trait so that tests can stand in
/// for Hermes.
pub trait HermesCli {
    #[allow(async_fn_in_trait)]
    async fn run(&self, args: &[&str]) -> Result<(), HermesCliError>;

    /// What a user would type to run the same: for when [`Self::run`] fails.
    fn command_line(&self, args: &[&str]) -> String;
}

#[derive(Debug, thiserror::Error)]
pub enum HermesCliError {
    #[error("`hermes` is not on your PATH")]
    NotFound,

    /// The command exited non-zero, with the last line it printed.
    #[error("`hermes` failed: {0}")]
    Failed(String),

    #[error("could not run `hermes`")]
    Io(
        #[from]
        #[source]
        std::io::Error,
    ),
}

/// The `hermes` on `PATH`, aimed at the home atuin wrote to: `HERMES_HOME` set to it, and `-p`
/// naming its profile (`default` for a root). Without `-p`, Hermes would follow a root's sticky
/// `active_profile` instead, and from a root that isn't `~/.hermes` too.
#[derive(Debug, Clone)]
pub struct HermesCommand {
    home: InstallHome,
}

impl HermesCommand {
    #[must_use]
    pub fn new(home: &InstallHome) -> Self {
        Self { home: home.clone() }
    }

    fn profile_args(&self) -> [&str; 2] {
        ["-p", self.home.profile()]
    }
}

impl HermesCli for HermesCommand {
    /// Hermes's output is kept back, since it would land in the middle of `atuin setup`'s; a
    /// failure carries its last line.
    async fn run(&self, args: &[&str]) -> Result<(), HermesCliError> {
        let hermes = resolve_in_path(Path::new("hermes")).ok_or(HermesCliError::NotFound)?;
        let output = tokio::process::Command::new(&*hermes)
            .args(self.profile_args())
            .args(args)
            .env("HERMES_HOME", self.home.path())
            .stdin(Stdio::null())
            .output()
            .await?;
        if output.status.success() {
            return Ok(());
        }
        let printed = [&output.stderr, &output.stdout].map(|bytes| String::from_utf8_lossy(bytes));
        let last_line = printed
            .iter()
            .flat_map(|text| text.lines().rev())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(|| output.status.to_string(), str::to_owned);
        Err(HermesCliError::Failed(last_line))
    }

    fn command_line(&self, args: &[&str]) -> String {
        let home = self.home.path().to_string_lossy();
        let home = shlex::try_quote(&home).unwrap_or(home.as_ref().into());
        let words: Vec<&str> = self.profile_args().iter().chain(args).copied().collect();
        let words: Vec<String> = words
            .iter()
            .map(|word| shlex::try_quote(word).map_or_else(|_| (*word).to_owned(), Into::into))
            .collect();
        format!("HERMES_HOME={home} hermes {}", words.join(" "))
    }
}

/// A stand-in for `hermes`, which tests never run.
#[cfg(test)]
pub(super) mod fake {
    use parking_lot::Mutex;

    use super::{HermesCli, HermesCliError};

    /// Records what it is asked to run, and says `hermes` is missing when built with
    /// [`Self::missing`].
    #[derive(Debug, Default)]
    pub struct FakeHermes {
        calls: Mutex<Vec<Vec<String>>>,
        missing: bool,
    }

    impl FakeHermes {
        pub fn missing() -> Self {
            Self {
                missing: true,
                ..Self::default()
            }
        }

        /// Each run's arguments.
        pub fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().clone()
        }

        /// Each run's arguments, joined by spaces.
        pub fn ran(&self) -> Vec<String> {
            self.calls().iter().map(|args| args.join(" ")).collect()
        }
    }

    impl HermesCli for FakeHermes {
        async fn run(&self, args: &[&str]) -> Result<(), HermesCliError> {
            self.calls.lock().push(args.iter().map(|&arg| arg.to_owned()).collect());
            if self.missing {
                Err(HermesCliError::NotFound)
            } else {
                Ok(())
            }
        }

        fn command_line(&self, args: &[&str]) -> String {
            format!("hermes {}", args.join(" "))
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[cfg(unix)]
    #[rstest]
    #[case::root(
        "/home/me/.hermes",
        "HERMES_HOME=/home/me/.hermes hermes -p default plugins enable atuin"
    )]
    #[case::profile(
        "/home/me/.hermes/profiles/work",
        "HERMES_HOME=/home/me/.hermes/profiles/work hermes -p work plugins enable atuin"
    )]
    #[case::spaces(
        "/home/me/My Hermes",
        "HERMES_HOME='/home/me/My Hermes' hermes -p default plugins enable atuin"
    )]
    fn the_command_line_runs_against_the_same_home(#[case] home: &str, #[case] want: &str) {
        let hermes = HermesCommand::new(&InstallHome::resolve(Path::new(home)));
        assert_eq!(hermes.command_line(&["plugins", "enable", "atuin"]), want);
    }
}
