//! Handles an `atuin` without desktop capture that runs the helper as its external subcommand.
//!
//! Such an `atuin` runs `atuin-desktop-capture` from `PATH` for `atuin desktop-capture ...`. For
//! example, a mainline `atuin` that comes first on `PATH` runs the helper of a build with desktop
//! capture that comes later on `PATH`.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use atuin_common::homebrew;
use atuin_common::os::process::current_exe;
use atuin_common::path::DisplayRichExt as _;
use atuin_desktop_capture::link::process::COMMAND_VARIABLE;

/// The helper's commands that a user runs, which `atuin desktop-capture` also has. Atuin runs
/// them with [`COMMAND_VARIABLE`] set.
const USER_COMMANDS: [&str; 2] = ["apps", "doctor"];

/// The helper's commands that Atuin runs for itself: the daemon runs `capture`, and setup runs
/// `access`.
const INTERNAL_COMMANDS: [&str; 3] = ["capture", "access", "help"];

/// The command that each login runs, which must never fail or print anything.
const LOGIN: &str = "login";

/// Takes over from the parser when an `atuin` without desktop capture ran the helper with `args`,
/// the arguments after the program name. Returns the exit code, or `None` to let the parser run.
///
/// `login` exits successfully at once and prints nothing. Another command that the helper doesn't
/// have, or a user command that an `atuin` ran without [`COMMAND_VARIABLE`], prints why it can't
/// run and how to fix `PATH`.
pub fn intercept(args: &[OsString]) -> Option<ExitCode> {
    let first = args.first()?;
    if first == LOGIN {
        return Some(ExitCode::SUCCESS);
    }
    let helpers_own = first.to_str().is_some_and(|first| {
        first.starts_with('-')
            || INTERNAL_COMMANDS.contains(&first)
            || USER_COMMANDS.contains(&first)
    });
    let run_by_bare_atuin = first.to_str().is_some_and(|first| USER_COMMANDS.contains(&first))
        && atuin_common::env::var_os(COMMAND_VARIABLE).is_none()
        && parent_atuin().is_some();
    if helpers_own && !run_by_bare_atuin {
        return None;
    }
    let caller = parent_atuin()
        .or_else(|| atuin_common::executable::resolve_in_path(Path::new("atuin")).map(Into::into));
    let explanation = Explanation {
        caller: caller.map(Caller::new),
        ours: sibling_atuin(),
        args,
    };
    eprintln!("{explanation}");
    Some(ExitCode::FAILURE)
}

/// The `atuin` that started this process, if an `atuin` did.
fn parent_atuin() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        let parent = rustix::process::getppid()?;
        let exe = atuin_common::os::unix::process::exe(parent)?;
        is_atuin(&exe).then_some(exe)
    }
    #[cfg(not(unix))]
    None
}

/// Whether `exe` is named like Atuin's executable.
fn is_atuin(exe: &Path) -> bool {
    let name = exe.file_name().map(OsStr::to_string_lossy);
    // On Linux, a replaced executable's path ends with " (deleted)".
    name.is_some_and(|name| {
        let name = name.strip_suffix(" (deleted)").unwrap_or(&name);
        name == "atuin" || name == "atuin.exe"
    })
}

/// The `atuin` next to this helper, which is the build that the helper belongs to.
fn sibling_atuin() -> Option<PathBuf> {
    let exe = current_exe().ok()?;
    let atuin = exe.parent()?.join(if cfg!(windows) {
        "atuin.exe"
    } else {
        "atuin"
    });
    atuin.is_file().then_some(atuin)
}

/// Why the helper can't run what an `atuin` without desktop capture asked for, and how to fix
/// `PATH`.
struct Explanation<'a> {
    /// The `atuin` that ran the helper, or else the `atuin` on `PATH`.
    caller: Option<Caller>,
    /// The `atuin` with desktop capture, next to this helper.
    ours: Option<PathBuf>,
    /// The arguments of `atuin desktop-capture`.
    args: &'a [OsString],
}

/// An `atuin` without desktop capture.
struct Caller {
    path: PathBuf,
    /// The Homebrew formula that links [`Self::path`] into Homebrew's `bin`, if Homebrew does.
    brew_link: Option<String>,
}

impl Caller {
    fn new(path: PathBuf) -> Self {
        Self {
            brew_link: homebrew::linked_formula(&path),
            path,
        }
    }
}

impl fmt::Display for Explanation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let command = |program: String| {
            fmt::from_fn(move |f| {
                write!(f, "{program} desktop-capture")?;
                for arg in self.args {
                    let arg = arg.to_string_lossy();
                    write!(f, " {}", shlex::try_quote(&arg).unwrap_or_default())?;
                }
                Ok(())
            })
        };
        write!(
            f,
            "error: `{}` needs an `atuin` with desktop capture, but ",
            command("atuin".to_owned())
        )?;
        match &self.caller {
            Some(caller) => {
                writeln!(f, "`atuin` runs {}, which doesn't have it.", shown(&caller.path))?;
            }
            None => writeln!(f, "the `atuin` that ran it doesn't have it.")?,
        }
        let Some(ours) = &self.ours else {
            return write!(
                f,
                "Put the directory of the `atuin` with desktop capture first on PATH, and then \
                 open a new terminal."
            );
        };
        write!(f, "The `atuin` with desktop capture is {}. To use it, ", shown(ours))?;
        match &self.caller {
            Some(Caller {
                brew_link: Some(formula),
                ..
            }) => write!(f, "run `brew unlink {formula}`, or ")?,
            Some(caller) => write!(f, "rename {}, or ", shown(&caller.path))?,
            None => {}
        }
        let dir = ours.parent().unwrap_or(ours);
        write!(
            f,
            "put {} first on PATH. Then open a new terminal, because a terminal that is already \
             open keeps its old PATH.",
            shown(dir)
        )?;
        if let Some(Caller {
            path,
            brew_link: Some(_),
        }) = &self.caller
        {
            write!(
                f,
                " `brew unlink` removes only the link {}, and Homebrew keeps the package \
                 installed.",
                shown(path)
            )?;
        }
        writeln!(f)?;
        let ours = shlex::try_quote(&ours.to_string_lossy()).unwrap_or_default().into_owned();
        write!(f, "Until then, run `{}`.", command(ours))
    }
}

/// `path` as the user would type it, with `~` for the home directory.
fn shown(path: &Path) -> String {
    path.display_rich().tilde_me().to_string()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    /// The tests run without an `atuin` parent, so only commands that the helper doesn't have are
    /// taken over.
    #[rstest]
    #[case::login(&["login", "--shell", "zsh"], Some(ExitCode::SUCCESS))]
    #[case::status(&["status"], Some(ExitCode::FAILURE))]
    #[case::apps(&["apps"], None)]
    #[case::doctor(&["doctor"], None)]
    #[case::capture(&["capture"], None)]
    #[case::access(&["access", "check"], None)]
    #[case::help(&["--help"], None)]
    #[case::nothing(&[], None)]
    fn takes_over_what_the_helper_does_not_have(
        #[case] given: &[&str],
        #[case] expected: Option<ExitCode>,
    ) {
        assert_eq!(intercept(&args(given)), expected);
    }

    #[rstest]
    #[case::atuin("/home/u/.atuin/bin/atuin", true)]
    #[case::replaced("/home/u/.atuin/bin/atuin (deleted)", true)]
    #[case::shell("/bin/zsh", false)]
    #[case::helper("/home/u/.cargo/bin/atuin-desktop-capture", false)]
    fn knows_an_atuin_parent(#[case] exe: &str, #[case] expected: bool) {
        assert_eq!(is_atuin(Path::new(exe)), expected);
    }

    /// The fix for Homebrew's `atuin` is `brew unlink`, which keeps the package.
    #[rstest]
    #[case::released(
        "/opt/released/atuin",
        None,
        "error: `atuin desktop-capture log --limit 5` needs an `atuin` with desktop capture, but \
         `atuin` runs /opt/released/atuin, which doesn't have it.\nThe `atuin` with desktop \
         capture is /opt/preview/atuin. To use it, rename /opt/released/atuin, or put \
         /opt/preview first on PATH. Then open a new terminal, because a terminal that is already \
         open keeps its old PATH.\nUntil then, run `/opt/preview/atuin desktop-capture log \
         --limit 5`."
    )]
    #[case::homebrew(
        "/opt/homebrew/bin/atuin",
        Some("atuin"),
        "error: `atuin desktop-capture log --limit 5` needs an `atuin` with desktop capture, but \
         `atuin` runs /opt/homebrew/bin/atuin, which doesn't have it.\nThe `atuin` with desktop \
         capture is /opt/preview/atuin. To use it, run `brew unlink atuin`, or put /opt/preview \
         first on PATH. Then open a new terminal, because a terminal that is already open keeps \
         its old PATH. `brew unlink` removes only the link /opt/homebrew/bin/atuin, and Homebrew \
         keeps the package installed.\nUntil then, run `/opt/preview/atuin desktop-capture log \
         --limit 5`."
    )]
    fn says_which_atuin_to_run_and_how_to_fix_path(
        #[case] caller: &str,
        #[case] brew_link: Option<&str>,
        #[case] expected: &str,
    ) {
        let given = args(&["log", "--limit", "5"]);
        let explanation = Explanation {
            caller: Some(Caller {
                path: PathBuf::from(caller),
                brew_link: brew_link.map(str::to_owned),
            }),
            ours: Some(PathBuf::from("/opt/preview/atuin")),
            args: &given,
        };
        assert_eq!(explanation.to_string(), expected);
    }
}
