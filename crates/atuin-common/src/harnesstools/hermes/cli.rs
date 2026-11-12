//! Running the `hermes` CLI for what should go through Hermes's own writers. `plugins enable` also
//! loads the plugin into a running gateway. `config set` keeps the comments in `config.yaml`. The
//! plugin's `atuin import` is how atuin writes a session, which `rehydrate` describes.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use super::home::InstallHome;
use crate::executable::resolve_in_path;

/// How long a run may take: an import may first start a gateway, which loads every plugin and
/// opens the store.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// What a run of `hermes` printed, and how it exited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliOutput {
    /// The exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    #[must_use]
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// The last line it printed, for an error: stderr first, where Hermes says what went wrong.
    #[must_use]
    pub fn last_line(&self) -> String {
        [&self.stderr, &self.stdout]
            .iter()
            .flat_map(|text| text.lines().rev())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(
                || self.code.map_or_else(|| "killed".to_owned(), |code| format!("exit {code}")),
                str::to_owned,
            )
    }
}

/// Runs `hermes` with arguments, against one Hermes home. A trait so that tests can stand in
/// for Hermes.
pub trait HermesCli {
    /// Run `hermes` with `args`, stdin closed, and hand back what it printed.
    #[allow(async_fn_in_trait)]
    async fn output(&self, args: &[&str]) -> Result<CliOutput, HermesCliError>;

    /// [`Self::output`], for a command whose output is kept back, since it would land in the
    /// middle of `atuin setup`'s; a failure carries its last line.
    #[allow(async_fn_in_trait)]
    async fn run(&self, args: &[&str]) -> Result<(), HermesCliError> {
        let output = self.output(args).await?;
        if output.success() {
            Ok(())
        } else {
            Err(HermesCliError::Failed(output.last_line()))
        }
    }

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

    #[error("`hermes` took longer than {} seconds", RUN_TIMEOUT.as_secs())]
    TimedOut,

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
    async fn output(&self, args: &[&str]) -> Result<CliOutput, HermesCliError> {
        let hermes = resolve_in_path(Path::new("hermes")).ok_or(HermesCliError::NotFound)?;
        let mut command = tokio::process::Command::new(&*hermes);
        command
            .args(self.profile_args())
            .args(args)
            .env("HERMES_HOME", self.home.path())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(RUN_TIMEOUT, command.output())
            .await
            .map_err(|_| HermesCliError::TimedOut)??;
        Ok(CliOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
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
    use std::collections::VecDeque;

    use parking_lot::Mutex;

    use super::{CliOutput, HermesCli, HermesCliError};

    /// What a [`FakeHermes`] does when run.
    pub type Reply = Box<dyn Fn(&[&str]) -> Result<CliOutput, HermesCliError> + Send + Sync>;

    /// Records what it is asked to run, and answers each run with the next of the replies it was
    /// built with by [`Self::replying`]. Without one, a run prints nothing and succeeds. Built with
    /// [`Self::missing`], it says `hermes` is missing.
    #[derive(Default)]
    pub struct FakeHermes {
        calls: Mutex<Vec<Vec<String>>>,
        replies: Mutex<VecDeque<Reply>>,
        missing: bool,
    }

    impl std::fmt::Debug for FakeHermes {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FakeHermes").field("calls", &self.calls).finish_non_exhaustive()
        }
    }

    impl FakeHermes {
        pub fn missing() -> Self {
            Self {
                missing: true,
                ..Self::default()
            }
        }

        /// A Hermes answering its runs, in order, with `replies`.
        pub fn replying(replies: impl IntoIterator<Item = Reply>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().collect()),
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
        async fn output(&self, args: &[&str]) -> Result<CliOutput, HermesCliError> {
            self.calls.lock().push(args.iter().map(|&arg| arg.to_owned()).collect());
            if self.missing {
                return Err(HermesCliError::NotFound);
            }
            let reply = self.replies.lock().pop_front();
            match reply {
                Some(reply) => reply(args),
                None => Ok(CliOutput {
                    code: Some(0),
                    ..CliOutput::default()
                }),
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
