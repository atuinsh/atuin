//! Finds another `atuin` that takes the daemon from this one, and says how to stop it.
//!
//! A client of another build replaces the daemon with a daemon of its own build. Two atuins that
//! a user runs in turn take the daemon from each other, and a build without desktop capture stops
//! capture each time that it takes the daemon. A `brew services` job of Homebrew's `atuin` also
//! takes the daemon, at each login and whenever the daemon stops.

use std::path::{Path, PathBuf};

use atuin_client::settings::Settings;
use atuin_common::atuin_exe::{PathAtuin, path_atuin};
use atuin_common::homebrew;
use atuin_common::os::process::current_exe;
use atuin_common::path::{AbsPathBuf, DisplayRichExt as _};
use atuin_daemon::grpc::versioning::{VersionMatchError, has_desktop_capture};
use atuin_daemon::service::pidfile::{Pidfile, PidfileInfo};
use atuin_daemon::service::{ActiveService, RunMode};
use serde::Serialize;

use crate::i18n::fl;

/// The Homebrew formula of atuin.
const FORMULA: &str = "atuin";

/// Everything that takes the daemon from this `atuin`.
#[derive(Debug, Serialize)]
pub struct Conflict {
    /// The running daemon, when it is another build.
    other_daemon: Option<OtherDaemon>,
    /// The `atuin` that `PATH` runs, and whether it is this `atuin`. `null` when `PATH` doesn't
    /// hold any `atuin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    path_atuin: Option<PathCheck>,
    /// The `brew services` job of Homebrew's `atuin`, when this `atuin` is another one.
    #[serde(skip_serializing_if = "Option::is_none")]
    brew_service: Option<BrewService>,
}

impl Conflict {
    /// Finds everything that takes the daemon from this `atuin`. Doesn't start or replace a daemon.
    pub async fn find(settings: &Settings) -> Self {
        Self::besides(OtherDaemon::find(settings).await).await
    }

    /// Finds what will take the daemon from this `atuin` later: `PATH` and `brew services`.
    pub async fn later() -> Self {
        Self::besides(None).await
    }

    /// Finds what takes the daemon from this `atuin`, besides `daemon`.
    pub async fn besides(daemon: Option<OtherDaemon>) -> Self {
        Self {
            other_daemon: daemon,
            path_atuin: PathCheck::run().await,
            brew_service: BrewService::find(),
        }
    }

    /// Explains which `atuin` started the daemon, what will start that `atuin` again, and how to
    /// keep that `atuin` from starting again. `None` when there is nothing to explain.
    pub fn explain(&self) -> Option<String> {
        let service = self
            .brew_service_formula()
            .map(|formula| fl!("desktop-capture-brew-service", formula = formula));
        let path = self.path_atuin.as_ref();
        let Some(daemon) = &self.other_daemon else {
            return join([service, path.and_then(PathCheck::explain)]);
        };
        let path_sentence = path.and_then(|path| match &path.atuin {
            PathAtuin::Other { first, .. } if daemon.runs(first) => {
                join([Some(fl!("desktop-capture-path-runs-daemon")), path.fix()])
            }
            PathAtuin::Other { .. } => path.explain(),
            // A daemon that is running an earlier build at this executable's path came from this
            // atuin.
            PathAtuin::This(_) if service.is_none() && !daemon.runs_this_path() => daemon
                .executable
                .as_deref()
                .map(|program| fl!("desktop-capture-path-runs-this", program = shown(program))),
            PathAtuin::This(_) | PathAtuin::Absent => None,
        });
        join([Some(daemon.sentence()), service, path_sentence])
    }

    /// The formula of the `brew services` job that takes the daemon from this `atuin`, if one does.
    fn brew_service_formula(&self) -> Option<&str> {
        let running = self.other_daemon.as_ref().and_then(|daemon| daemon.brew_service.as_deref());
        running.or_else(|| self.brew_service.as_ref().map(|_| FORMULA))
    }
}

/// A running daemon of another build, which a client of this build replaces.
#[derive(Debug, Serialize)]
pub struct OtherDaemon {
    build: String,
    /// The executable that the daemon runs, when the OS or the daemon's pidfile says.
    executable: Option<PathBuf>,
    has_desktop_capture: bool,
    /// Whether the daemon's version is newer than this client's, so that a client of this build
    /// leaves the daemon running.
    newer: bool,
    /// The Homebrew formula whose `brew services` job runs the daemon, when one does.
    #[serde(skip_serializing_if = "Option::is_none")]
    brew_service: Option<String>,
}

impl OtherDaemon {
    /// Finds the running daemon when it is another build. Doesn't start or replace a daemon.
    pub async fn find(settings: &Settings) -> Option<Self> {
        let service = ActiveService::find(settings).await.ok()??;
        let mismatch = service.ensure_our_build().err()?;
        let newer = mismatch.peer_is_newer();
        let VersionMatchError::Mismatched { theirs, .. } = mismatch else {
            return None;
        };
        let info = Pidfile::new(&settings.daemon.pidfile_path).info();
        let brew_service = info.as_ref().and_then(|info| match &info.run_mode {
            Some(RunMode::External(Some(manager))) => manager.brew_formula().map(str::to_owned),
            _ => None,
        });
        Some(Self {
            has_desktop_capture: has_desktop_capture(&theirs),
            build: theirs,
            executable: info.and_then(daemon_executable),
            newer,
            brew_service,
        })
    }

    /// Whether a client of this build replaces the daemon.
    pub const fn is_replaced(&self) -> bool {
        !self.newer
    }

    /// Whether a `brew services` job runs the daemon.
    pub const fn is_run_by_brew_services(&self) -> bool {
        self.brew_service.is_some()
    }

    /// The sentence that says which `atuin` started the daemon.
    fn sentence(&self) -> String {
        let build = self.build.clone();
        let capture = if self.has_desktop_capture {
            "yes"
        } else {
            "no"
        };
        match &self.executable {
            Some(program) => fl!(
                "desktop-capture-other-daemon",
                program = shown(program),
                build = build,
                capture = capture
            ),
            None => fl!("desktop-capture-other-daemon-unnamed", build = build, capture = capture),
        }
    }

    /// Whether the daemon is running this executable's path, as an earlier build that was
    /// replaced on disk does.
    fn runs_this_path(&self) -> bool {
        let (Some(executable), Ok(ours)) = (&self.executable, current_exe()) else {
            return false;
        };
        let executable = executable.to_string_lossy();
        let executable = Path::new(executable.strip_suffix(" (deleted)").unwrap_or(&executable));
        same_file(executable, &ours)
    }

    /// Whether the daemon is running the executable at `path`.
    fn runs(&self, path: &Path) -> bool {
        self.executable.as_deref().is_some_and(|executable| same_file(executable, path))
    }
}

/// The executable of the process that holds the daemon's pidfile, which `info` describes, or else
/// the path that the daemon wrote into the pidfile.
fn daemon_executable(info: PidfileInfo) -> Option<PathBuf> {
    #[cfg(unix)]
    if let Some(exe) = atuin_common::os::unix::process::pid_from_u32(info.pid)
        .and_then(atuin_common::os::unix::process::exe)
    {
        return Some(exe);
    }
    info.program
}

/// What a shell runs as `atuin`, and where this `atuin` is.
#[derive(Debug)]
pub struct PathCheck {
    atuin: PathAtuin,
    /// The directory that a fix puts first on `PATH`: the directory of a later `atuin` on `PATH`
    /// that is this executable, or else the directory of this executable.
    ours: Option<PathBuf>,
    /// The Homebrew formula that links the first `atuin` on `PATH` into Homebrew's `bin`, when
    /// that `atuin` is another `atuin`.
    brew_link: Option<String>,
}

impl PathCheck {
    /// Looks up `atuin` on `PATH`. `None` when the OS can't say where this executable is.
    pub async fn run() -> Option<Self> {
        let atuin = path_atuin().await.ok()?;
        let ours = match &atuin {
            PathAtuin::Other {
                later: Some(later), ..
            } => later.parent().map(Path::to_path_buf),
            _ => current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)),
        };
        let brew_link = match &atuin {
            PathAtuin::Other { first, .. } => homebrew::linked_formula(first),
            PathAtuin::This(_) | PathAtuin::Absent => None,
        };
        Some(Self {
            atuin,
            ours,
            brew_link,
        })
    }

    /// The `atuin` on `PATH`, when that `atuin` runs another executable.
    fn other(&self) -> Option<&Path> {
        match &self.atuin {
            PathAtuin::Other { first, .. } => Some(first),
            PathAtuin::This(_) | PathAtuin::Absent => None,
        }
    }

    /// The sentences that say how to make `PATH` run this `atuin`, when `PATH` runs another one.
    fn fix(&self) -> Option<String> {
        let other = self.other()?;
        let dir =
            self.ours.as_deref().map_or_else(|| fl!("desktop-capture-path-fix-this-dir"), shown);
        Some(match &self.brew_link {
            Some(formula) => fl!(
                "desktop-capture-path-fix-brew",
                path = shown(other),
                dir = dir,
                formula = formula.as_str()
            ),
            None => fl!("desktop-capture-path-fix", path = shown(other), dir = dir),
        })
    }

    /// Explains that `PATH` runs another `atuin`, which will stop desktop capture, and how to fix
    /// `PATH`. `None` when `PATH` runs this `atuin` or no `atuin`.
    fn explain(&self) -> Option<String> {
        let other = self.other()?;
        let runs = fl!("desktop-capture-path-runs-other", path = shown(other));
        join([Some(runs), self.fix()])
    }
}

/// How the JSON of `atuin doctor` shows what `PATH` runs as `atuin`.
impl Serialize for PathCheck {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Shown<'a> {
            path: &'a Path,
            runs_this_atuin: bool,
        }

        match &self.atuin {
            PathAtuin::This(path) => Shown {
                path,
                runs_this_atuin: true,
            }
            .serialize(serializer),
            PathAtuin::Other { first, .. } => Shown {
                path: first,
                runs_this_atuin: false,
            }
            .serialize(serializer),
            PathAtuin::Absent => serializer.serialize_none(),
        }
    }
}

/// The `brew services` job of Homebrew's `atuin`, which launchd runs at each login and whenever the
/// daemon stops.
#[derive(Debug, Serialize)]
pub struct BrewService {
    plist: PathBuf,
}

impl BrewService {
    /// Finds the job that `brew services start atuin` installed, unless this `atuin` is
    /// Homebrew's.
    fn find() -> Option<Self> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let home = atuin_common::dirs::try_home_dir().ok()?;
        let plist = homebrew::launch_agent(&home, FORMULA)?;
        let ours = current_exe().ok().and_then(|exe| homebrew::keg_formula(&exe));
        (ours.as_deref() != Some(FORMULA)).then_some(Self { plist })
    }
}

/// The sentence that says that this command replaces the daemon.
pub fn replacing() -> String {
    let program = current_exe().map_or_else(|_| "atuin".to_owned(), |exe| shown(&exe));
    fl!("desktop-capture-replacing-daemon", program = program)
}

/// Whether `a` and `b` lead to the same file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (AbsPathBuf::canonicalize(a), AbsPathBuf::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// `path` as the user would type it, with `~` for the home directory.
fn shown(path: &Path) -> String {
    path.display_rich().tilde_me().to_string()
}

/// The sentences that are present, separated by spaces. `None` when none is present.
fn join(sentences: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    let mut joined: Option<String> = None;
    for sentence in sentences.into_iter().flatten() {
        match &mut joined {
            Some(joined) => {
                joined.push(' ');
                joined.push_str(&sentence);
            }
            None => joined = Some(sentence),
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const RELEASED: &str = "/opt/released/atuin";
    const BREW_LINK: &str = "/opt/homebrew/bin/atuin";
    const BREW_OPT: &str = "/opt/homebrew/opt/atuin/bin/atuin";

    fn daemon(executable: Option<&str>, has_desktop_capture: bool) -> OtherDaemon {
        OtherDaemon {
            build: "18.25.1".to_owned(),
            executable: executable.map(PathBuf::from),
            has_desktop_capture,
            newer: false,
            brew_service: None,
        }
    }

    /// The daemon that `brew services start atuin` runs.
    fn brew_services_daemon() -> OtherDaemon {
        OtherDaemon {
            brew_service: Some(FORMULA.to_owned()),
            ..daemon(Some(BREW_OPT), false)
        }
    }

    fn path(atuin: PathAtuin) -> PathCheck {
        PathCheck {
            atuin,
            ours: Some(PathBuf::from("/opt/preview")),
            brew_link: None,
        }
    }

    /// `PATH` that runs Homebrew's link to its atuin first.
    fn brew_path() -> PathCheck {
        PathCheck {
            brew_link: Some(FORMULA.to_owned()),
            ..path(other(BREW_LINK))
        }
    }

    fn this() -> PathAtuin {
        PathAtuin::This(PathBuf::from("/opt/preview/atuin"))
    }

    fn other(first: &str) -> PathAtuin {
        PathAtuin::Other {
            first: PathBuf::from(first),
            later: None,
        }
    }

    fn brew_service() -> BrewService {
        BrewService {
            plist: PathBuf::from("/Users/u/Library/LaunchAgents/sh.brew.atuin.plist"),
        }
    }

    fn conflict(
        daemon: Option<OtherDaemon>,
        path: Option<PathCheck>,
        brew_service: Option<BrewService>,
    ) -> Conflict {
        Conflict {
            other_daemon: daemon,
            path_atuin: path,
            brew_service,
        }
    }

    const BREW_SERVICES: &str = "A `brew services` job runs Homebrew's `atuin` as the daemon. \
                                 launchd starts the job at each login and again whenever the \
                                 job's daemon exits, so Homebrew's daemon takes over once the \
                                 daemon of this `atuin` stops, and desktop capture will stop. To \
                                 use desktop capture, stop the job with `brew services stop \
                                 atuin`. The Homebrew package will stay installed.";

    /// Each situation gets the sentences that apply to it, in order.
    #[rstest]
    #[case::released_daemon_first_on_path(
        conflict(Some(daemon(Some(RELEASED), false)), Some(path(other(RELEASED))), None),
        "The daemon was started by /opt/released/atuin (Atuin 18.25.1), which doesn't have \
         desktop capture. That `atuin` comes first on PATH, so it will replace the daemon again \
         after your next command. To keep desktop capture running, rename /opt/released/atuin, or \
         put /opt/preview first on PATH. Then open a new terminal, because a terminal that is \
         already open keeps its old PATH."
    )]
    #[case::released_daemon_another_atuin_on_path(
        conflict(Some(daemon(Some(RELEASED), false)), Some(path(other("/opt/third/atuin"))), None),
        "The daemon was started by /opt/released/atuin (Atuin 18.25.1), which doesn't have \
         desktop capture. Desktop capture will stop after your next command, because PATH runs \
         /opt/third/atuin, another `atuin`, which replaces the daemon. To keep desktop capture \
         running, rename /opt/third/atuin, or put /opt/preview first on PATH. Then open a new \
         terminal, because a terminal that is already open keeps its old PATH."
    )]
    #[case::released_daemon_this_on_path(
        conflict(Some(daemon(Some(RELEASED), false)), Some(path(this())), None),
        "The daemon was started by /opt/released/atuin (Atuin 18.25.1), which doesn't have \
         desktop capture. PATH runs this `atuin`, so something other than your PATH ran \
         /opt/released/atuin. For example, a terminal that was open before PATH changed still \
         runs it. If desktop capture stops again, close the terminals that are older than that \
         change."
    )]
    #[case::unnamed_daemon_with_capture(
        conflict(Some(daemon(None, true)), Some(path(PathAtuin::Absent)), None),
        "The daemon is running Atuin 18.25.1, another build with desktop capture."
    )]
    #[case::path_alone(
        conflict(None, Some(path(other(RELEASED))), None),
        "Desktop capture will stop after your next command, because PATH runs \
         /opt/released/atuin, another `atuin`, which replaces the daemon. To keep desktop capture \
         running, rename /opt/released/atuin, or put /opt/preview first on PATH. Then open a new \
         terminal, because a terminal that is already open keeps its old PATH."
    )]
    #[case::brew_link_alone(
        conflict(None, Some(brew_path()), None),
        "Desktop capture will stop after your next command, because PATH runs \
         /opt/homebrew/bin/atuin, another `atuin`, which replaces the daemon. To keep desktop \
         capture running, run `brew unlink atuin`, or put /opt/preview first on PATH. Then open a \
         new terminal, because a terminal that is already open keeps its old PATH. `brew unlink` \
         removes only the link /opt/homebrew/bin/atuin, and Homebrew keeps the package installed."
    )]
    #[case::brew_link_daemon_first_on_path(
        conflict(Some(daemon(Some(BREW_LINK), false)), Some(brew_path()), None),
        "The daemon was started by /opt/homebrew/bin/atuin (Atuin 18.25.1), which doesn't have \
         desktop capture. That `atuin` comes first on PATH, so it will replace the daemon again \
         after your next command. To keep desktop capture running, run `brew unlink atuin`, or \
         put /opt/preview first on PATH. Then open a new terminal, because a terminal that is \
         already open keeps its old PATH. `brew unlink` removes only the link \
         /opt/homebrew/bin/atuin, and Homebrew keeps the package installed."
    )]
    #[case::brew_services_daemon_this_on_path(
        conflict(Some(brew_services_daemon()), Some(path(this())), None),
        &format!(
            "The daemon was started by /opt/homebrew/opt/atuin/bin/atuin (Atuin 18.25.1), which \
             doesn't have desktop capture. {BREW_SERVICES}"
        )
    )]
    #[case::released_daemon_brew_service_installed(
        conflict(Some(daemon(Some(BREW_OPT), false)), Some(path(this())), Some(brew_service())),
        &format!(
            "The daemon was started by /opt/homebrew/opt/atuin/bin/atuin (Atuin 18.25.1), which \
             doesn't have desktop capture. {BREW_SERVICES}"
        )
    )]
    #[case::brew_service_installed_alone(
        conflict(None, Some(path(this())), Some(brew_service())),
        BREW_SERVICES
    )]
    #[case::brew_service_and_brew_link(
        conflict(None, Some(brew_path()), Some(brew_service())),
        &format!(
            "{BREW_SERVICES} Desktop capture will stop after your next command, because PATH runs \
             /opt/homebrew/bin/atuin, another `atuin`, which replaces the daemon. To keep desktop \
             capture running, run `brew unlink atuin`, or put /opt/preview first on PATH. Then \
             open a new terminal, because a terminal that is already open keeps its old PATH. \
             `brew unlink` removes only the link /opt/homebrew/bin/atuin, and Homebrew keeps the \
             package installed."
        )
    )]
    fn explains_each_situation(#[case] conflict: Conflict, #[case] expected: &str) {
        assert_eq!(conflict.explain().as_deref(), Some(expected));
    }

    /// Nothing is wrong when the daemon is this build, `PATH` runs this `atuin` or none, and
    /// `brew services` doesn't run Homebrew's `atuin`.
    #[rstest]
    #[case::this(Some(path(this())))]
    #[case::absent(Some(path(PathAtuin::Absent)))]
    #[case::unknown(None)]
    fn says_nothing_when_nothing_is_wrong(#[case] path: Option<PathCheck>) {
        assert_eq!(conflict(None, path, None).explain(), None);
    }

    #[rstest]
    #[case::this(PathAtuin::This(PathBuf::from("/a/atuin")), serde_json::json!({"path": "/a/atuin", "runs_this_atuin": true}))]
    #[case::other(other("/b/atuin"), serde_json::json!({"path": "/b/atuin", "runs_this_atuin": false}))]
    #[case::absent(PathAtuin::Absent, serde_json::Value::Null)]
    fn doctor_shows_what_path_runs(#[case] atuin: PathAtuin, #[case] expected: serde_json::Value) {
        assert_eq!(serde_json::to_value(path(atuin)).unwrap(), expected);
    }
}
