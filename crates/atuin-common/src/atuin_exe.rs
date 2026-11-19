//! Provides the path to the `atuin` binary.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt as _;
use tokio::sync::OnceCell;

use crate::executable;
use crate::os::process::current_exe;
#[cfg(unix)]
use crate::os::unix;
use crate::path::AbsPathBuf;

/// The arguments that make Atuin print its own executable path.
///
/// See `Cmd::PrintExe` in crates/atuin/src/command/client/internal.rs.
const PRINT_EXE_ARGS: [&str; 2] = ["__internal", "print-exe"];

/// A minimal `$PATH` containing only directories that are highly likely to be in the real `$PATH`
/// across all environments.
///
/// We use this to ensure we don't configure anything to run Atuin via a shim script that requires
/// a `$PATH` that may not always hold. For example, systemd and launchd often have a more minimal
/// `$PATH` which might cause certain shim scripts to fail.
#[cfg(unix)]
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// How long [`launch_path`] waits for a candidate executable to respond, when the executable has to
/// be probed (i.e., called with `__internal print-exe`).
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The most that a probe reads of an answer. An answer is a pid and a path, so a longer answer
/// comes from something other than atuin.
const MAX_ANSWER: u64 = 64 * 1024;

/// The path to `atuin` that tools should be configured to use.
///
/// Unlike [`current_exe`], which can break when a user upgrades Atuin if they've installed it from
/// certain package managers like Homebrew[^1], this function tries to produce a stable, reliable
/// path.
///
/// [^1]: This is because Homebrew gives every version of Atuin a different path.
pub async fn launch_path() -> Result<&'static Path, LaunchPathError> {
    static LAUNCH_PATH: OnceCell<PathBuf> = OnceCell::const_new();
    LAUNCH_PATH
        .get_or_try_init(|| calculate_launch_path(current_exe(), PROBE_TIMEOUT))
        .await
        .map(PathBuf::as_path)
        .map_err(LaunchPathError)
}

/// Error returned when [`launch_path`] can't find atuin's executable.
#[derive(Debug, thiserror::Error)]
#[error("could not determine the path to `atuin`")]
pub struct LaunchPathError(#[source] std::io::Error);

/// The core of [`launch_path`]: calculates the launch path given `exe` as the result of
/// [`current_exe`].
///
/// The returned path can either be a *link* or a *shim*:
///
/// * A link is a file that [canonicalizes] to the same path as `exe`. This could be a symbolic link
///   to `exe`, or `exe` itself.
///
/// * A shim is a file that doesn't canonicalize to `exe`, but when run, ends up running `exe`. A
///   shim could be, for example, a shell script that runs `exec /path/to/real/atuin`. A shim must
///   execute the real Atuin binary via `exec`; it cannot launch it as a child process.
///
/// This function goes through each `atuin` in `$PATH` and classifies each as a *link*, a *shim*, or
/// *neither*. The return value is calculated as follows:
///
/// 1. If `$PATH` contains at least one shim, and every `atuin` before the shim is a link, the shim
///    is returned.[^1]
/// 2. Otherwise, if the first `atuin` in `$PATH` is a link, that `atuin` is returned.
/// 3. Otherwise, `exe` is returned.
///
/// As an exception, if `exe` is [`Err`], or if `exe` can't be canonicalized (which likely indicates
/// Atuin was upgraded and we're running a version that no longer exists on disk), then when
/// determining if a candidate path is a link or shim, instead of checking it against `exe`, we
/// probe the first `atuin` in `$PATH` to tell what binary it actually runs, and check the candidate
/// paths against that instead. This has essentially the same result as if we were to run
/// [`calculate_launch_path`] in the first `atuin` itself. If that probe gets no answer and `exe`
/// doesn't exist, the result is an error, since a path that doesn't exist can't be run.
///
/// If this function needs to probe[^2] a candidate executable, `timeout` controls how long the
/// probe can take before giving up.
///
/// [^1]: This is because certain package managers like mise put `atuin` in a version-dependent
/// directory that will break on upgrade, and put that directory ahead of the shim directory in
/// `$PATH`. Preferring the shim, whose path isn't version-dependent, avoids this.
///
/// [^2]: This means calling the executable with `__internal print-exe`.
///
/// [canonicalizes]: tokio::fs::canonicalize
async fn calculate_launch_path(
    exe: std::io::Result<PathBuf>,
    timeout: Duration,
) -> std::io::Result<PathBuf> {
    let paths = crate::env::var_os("PATH").unwrap_or_default();
    let mut atuins = atuins_in(&paths).peekable();

    // The result of trying to canonicalize `exe`.
    enum ExeCanonicalized {
        Ok {
            exe: PathBuf,
            ours: AbsPathBuf,
        },
        Err {
            exe: std::io::Result<PathBuf>,
        },
    }

    let canonicalized = match exe {
        Ok(exe) => match AbsPathBuf::canonicalize_async(&exe).await {
            Ok(ours) => ExeCanonicalized::Ok { exe, ours },
            // A path to an executable that's gone can't be run, so don't use it as a fallback.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                ExeCanonicalized::Err { exe: Err(e) }
            }
            Err(_) => ExeCanonicalized::Err { exe: Ok(exe) },
        },
        Err(e) => ExeCanonicalized::Err { exe: Err(e) },
    };

    // How the first `atuin` ran its executable, when it was probed to find that executable.
    let mut first_launch: Option<Launch> = None;
    let (exe, ours) = match canonicalized {
        ExeCanonicalized::Ok { exe, ours } => (exe, ours),
        ExeCanonicalized::Err { exe } => {
            // We couldn't canonicalize `exe`, most likely because Atuin was upgraded and the
            // version of Atuin this process is running no longer exists. Probe the first `atuin`
            // in `$PATH` and use its exe path instead.
            let Some(atuin) = atuins.peek() else {
                return exe;
            };
            let Some(answer) = probe_exe(atuin, timeout).await else {
                return exe;
            };
            let Ok(theirs) = AbsPathBuf::canonicalize_async(&answer.exe).await else {
                return exe;
            };
            first_launch = Some(answer.launch);
            (answer.exe, theirs)
        }
    };

    let mut link = None;
    for atuin in atuins {
        let cached = first_launch.take();
        if AbsPathBuf::canonicalize_async(&atuin).await.is_ok_and(|theirs| theirs == ours) {
            // `atuin` resolves to the same path as `exe`. Don't stop here, though -- `atuin` might
            // have version-dependent components in its path if it was installed by certain package
            // managers. Keep searching for a shim that runs `exe`, and prefer that if we find one:
            // the shim is more likely to have a version-independent path.
            link.get_or_insert(atuin);
            continue;
        }

        // `atuin` didn't resolve to `exe`, so it could be a shim.
        let launch = match cached {
            Some(launch) => launch,
            None => {
                let Some(answer) = probe_exe(&atuin, timeout).await else {
                    // Wasn't a valid shim.
                    break;
                };
                if !AbsPathBuf::canonicalize_async(answer.exe)
                    .await
                    .is_ok_and(|theirs| theirs == ours)
                {
                    // Was a shim but doesn't run `exe`.
                    break;
                }
                answer.launch
            }
        };

        if launch != Launch::Exec {
            // Was a shim that ran `exe`, but spawned it in a subprocess rather than using `exec`,
            // which interferes with certain programs like launchd.
            break;
        }
        return Ok(atuin);
    }

    // We didn't find a matching shim. If the first `atuin` was a link to `exe`, return that;
    // otherwise, fall back to returning `exe`.
    Ok(link.unwrap_or(exe))
}

/// Searches `paths` (formatted like `$PATH`) for `atuin` binaries that the user can run.
///
/// Only absolute directories are searched.
///
/// TODO: On Windows, only `atuin.exe` is found, so a package manager's `atuin.cmd` shim is skipped
/// and the versioned executable is stored instead. Storing the shim needs care, because a harness
/// may spawn the stored path without a shell, and Node's `spawn` refuses a `.cmd` file without one.
fn atuins_in(paths: &OsStr) -> impl Iterator<Item = PathBuf> {
    const ATUIN: &str = if cfg!(windows) {
        "atuin.exe"
    } else {
        "atuin"
    };
    let dirs = std::env::split_paths(paths).filter(|dir| dir.is_absolute());
    executable::resolve_all_in_dirs(dirs, Path::new(ATUIN))
}

/// Probe the given `atuin` executable: ask it to print its own path by passing `__internal
/// print-exe`.
///
/// The answer's [`Answer::launch`] says whether a shim replaced itself with the atuin that it runs.
async fn probe_exe(atuin: &Path, timeout: Duration) -> Option<Answer> {
    let home = crate::dirs::try_home_dir().ok()?;
    let mut command = tokio::process::Command::new(atuin);
    command
        .args(PRINT_EXE_ARGS)
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    // Use a minimal `$PATH` for the probe to prevent landing on a shim script that can't be run
    // in certain environments like systemd or launchd (see `SYSTEM_PATH`).
    #[cfg(unix)]
    command.env_clear().env("HOME", &home).env("PATH", SYSTEM_PATH).process_group(0);

    let mut child = command.spawn().ok()?;
    let child_pid = child.id();
    let mut stdout = child.stdout.take()?;
    let wait = async {
        let mut answer = Vec::new();
        (&mut stdout).take(MAX_ANSWER + 1).read_to_end(&mut answer).await.ok()?;
        if answer.len() as u64 > MAX_ANSWER {
            return None;
        }
        let status = child.wait().await.ok()?;
        Some(std::process::Output {
            status,
            stdout: answer,
            stderr: Vec::new(),
        })
    };
    let mut wait = std::pin::pin!(wait);
    // If the probe times out or is cancelled, kill the whole process group rather than just the
    // process; otherwise the child processes of a shim script would be left running. The guard is
    // declared after `wait` so it's dropped first.
    #[cfg(unix)]
    let group =
        child_pid.and_then(unix::process::pid_from_u32).map(unix::process::KillGroupOnDrop::new);
    let output = tokio::time::timeout(timeout, &mut wait).await.ok()??;

    // Disarm only after the shim has exited and been reaped. On a timeout or an error, the shim's
    // pid still names the shim's group, so the guard kills that group.
    #[cfg(unix)]
    if let Some(group) = group {
        group.disarm();
    }
    if !output.status.success() {
        return None;
    }

    // The answer is the answering process's pid, a NUL byte, and the path.
    let mut answer = output.stdout;
    let nul = answer.iter().position(|&byte| byte == 0)?;
    let pid: u32 = std::str::from_utf8(&answer[..nul]).ok()?.parse().ok()?;

    // On Windows, a shim can't replace itself with what it runs (for example, Scoop's shims start
    // Atuin as their child), so always label the launch as `Exec` in that case.
    let launch = if cfg!(unix) && Some(pid) != child_pid {
        Launch::Child
    } else {
        Launch::Exec
    };
    answer.drain(..=nul);

    // On Unix, the probe can return non-UTF-8 paths -- preserve that.
    #[cfg(unix)]
    let exe = <std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(answer);
    #[cfg(not(unix))]
    let exe = String::from_utf8(answer).ok()?;
    Some(Answer {
        exe: PathBuf::from(exe),
        launch,
    })
}

/// What an `atuin` said when [`probe_exe`] probed it.
struct Answer {
    /// The path to the Atuin binary, as reported by the executable itself.
    exe: PathBuf,
    /// How the probed `atuin` ran [`Self::exe`].
    launch: Launch,
}

/// How a probed `atuin` ran the real Atuin binary.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum Launch {
    /// The `atuin` replaced itself with the real Atuin binary, or was the real Atuin binary.
    ///
    /// Only this kind of shim can be stored, since a shim that runs Atuin as its child would put
    /// itself between Atuin and whatever runs it. For example, under launchd, the daemon would not
    /// find that it is launchd's job.
    Exec,
    /// The `atuin` ran the real Atuin binary as a child process.
    Child,
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;
    use crate::env::MockEnv;
    use crate::path::PathBufExt as _;

    /// A temporary home that holds `bin/`, the `PATH`, and `real/atuin`, which stands for the
    /// running executable.
    struct InPath {
        dir: tempfile::TempDir,
        bin: PathBuf,
        real: PathBuf,
        env: MockEnv,
    }

    /// [`InPath`] with nothing in `bin/` yet.
    #[fixture]
    fn in_path() -> InPath {
        // Next to the test binary: CI's temporary directory is mounted without exec.
        let dir = tempfile::tempdir_in(current_exe().unwrap().parent().unwrap()).unwrap();
        let (bin, real) = (dir.path().join("bin"), dir.path().join("real"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("atuin"), "").unwrap();
        #[cfg(unix)]
        set_mode(&real.join("atuin"), 0o755);
        let env = MockEnv::install();
        env.set("PATH", &bin);
        env.set("HOME", dir.path());
        InPath {
            dir,
            bin,
            real: real.joined("atuin"),
            env,
        }
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Write `bin/atuin` as a shell script with `body`.
    #[cfg(unix)]
    fn write_shim(bin: &Path, body: &str) -> PathBuf {
        let shim = bin.join("atuin");
        std::fs::write(&shim, format!("#!/bin/sh\n{body}\n")).unwrap();
        set_mode(&shim, 0o755);
        shim
    }

    /// A shell command that answers the probe for `exe` as `atuin __internal print-exe` would, from
    /// a shim that has replaced itself with atuin: the shim's own pid, which `exec` keeps, a NUL
    /// byte, and the path.
    #[cfg(unix)]
    fn answer(exe: impl std::fmt::Display) -> String {
        format!(r#"printf '%s\0%s' "$$" '{exe}'"#)
    }

    /// A shim that runs atuin as its child doesn't count, since it would sit between atuin and
    /// whatever runs atuin. A shim that replaces itself with atuin does count.
    #[cfg(unix)]
    #[rstest]
    // `sh -c` runs the answer in a new process, which has a pid of its own. `exec sh -c` replaces
    // the shim with that process, which keeps the shim's pid.
    #[case::forks("sh -c", "real/atuin")]
    #[case::execs("exec sh -c", "bin/atuin")]
    #[tokio::test]
    async fn a_shim_counts_only_when_it_replaces_itself_with_atuin(
        in_path: InPath,
        #[case] run: &str,
        #[case] want: &str,
    ) {
        let answer = answer(in_path.real.display());
        let quoted = shlex::try_quote(&answer).unwrap();
        write_shim(&in_path.bin, &format!("{run} {quoted}"));

        assert_eq!(calculate(in_path.real).await, in_path.dir.path().join(want));
    }

    /// [`calculate_launch_path`] with a timeout that only a hung shim reaches, since a busy machine
    /// can take a while to start a script that was just written.
    async fn calculate(exe: PathBuf) -> PathBuf {
        calculate_launch_path(Ok(exe), Duration::from_secs(30)).await.unwrap()
    }

    #[rstest]
    #[tokio::test]
    async fn the_running_executable_without_an_atuin_in_path(in_path: InPath) {
        assert_eq!(calculate(in_path.real.clone()).await, in_path.real);
    }

    /// A Homebrew-style install: `bin/atuin` links to the real executable.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_link_to_the_running_executable(in_path: InPath) {
        std::os::unix::fs::symlink(&in_path.real, in_path.bin.join("atuin")).unwrap();

        assert_eq!(calculate(in_path.real).await, in_path.bin.join("atuin"));
    }

    /// A shell skips an `atuin` that it can't execute and runs the next one in `PATH`.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn an_atuin_that_cannot_be_executed_is_skipped(in_path: InPath) {
        std::fs::write(in_path.bin.join("atuin"), "").unwrap();
        set_mode(&in_path.bin.join("atuin"), 0o644);
        let later = in_path.dir.path().join("later");
        std::fs::create_dir(&later).unwrap();
        std::os::unix::fs::symlink(&in_path.real, later.join("atuin")).unwrap();
        in_path.env.set("PATH", std::env::join_paths([&in_path.bin, &later]).unwrap());

        assert_eq!(calculate(in_path.real).await, later.joined("atuin"));
    }

    /// The same `bin`, relative to the current directory, is not a launch path: what it finds
    /// depends on where the program that runs it is.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn only_absolute_path_entries(in_path: InPath) {
        std::os::unix::fs::symlink(&in_path.real, in_path.bin.join("atuin")).unwrap();
        let cwd = std::env::current_dir().unwrap();
        let up: PathBuf = std::iter::repeat_n("..", cwd.components().count() - 1).collect();
        let relative = up.joined(in_path.bin.strip_prefix("/").unwrap());
        assert!(relative.join("atuin").is_file());
        in_path.env.set("PATH", relative);

        assert_eq!(calculate(in_path.real.clone()).await, in_path.real);
    }

    /// A mise- or asdf-style shim: a script that runs some atuin, which says what it is.
    #[cfg(unix)]
    #[rstest]
    #[case::runs_this_executable(Some("real/atuin"), "bin/atuin")]
    #[case::runs_another(Some("bin/../elsewhere"), "real/atuin")]
    #[case::fails(None, "real/atuin")]
    #[tokio::test]
    async fn a_shim_counts_when_it_runs_this_executable(
        in_path: InPath,
        #[case] runs: Option<&str>,
        #[case] want: &str,
    ) {
        std::fs::write(in_path.dir.path().join("elsewhere"), "").unwrap();
        let body = runs.map_or_else(
            || "exit 1".to_owned(),
            |runs| {
                let runs = in_path.dir.path().join(runs);
                let [internal, print_exe] = PRINT_EXE_ARGS;
                format!(r#"[ "$1 $2" = "{internal} {print_exe}" ] && {}"#, answer(runs.display()))
            },
        );
        write_shim(&in_path.bin, &body);

        assert_eq!(calculate(in_path.real).await, in_path.dir.path().join(want));
    }

    /// An `atuin` in `PATH`, for [`the_first_shim_wins_over_a_link`].
    #[cfg(unix)]
    #[derive(Debug, Clone, Copy)]
    enum Entry {
        /// A link to the running executable.
        Link,
        /// A shim that runs the running executable.
        Shim,
        /// A shim that runs another executable.
        Other,
    }

    /// The `atuin`s in `PATH` count in the order that [`calculate_launch_path`] gives. For example,
    /// mise puts a directory that links to one version ahead of its shims.
    #[cfg(unix)]
    #[rstest]
    #[case::link_then_shim(Entry::Link, Entry::Shim, "later/atuin")]
    #[case::link_then_link(Entry::Link, Entry::Link, "bin/atuin")]
    #[case::link_then_other(Entry::Link, Entry::Other, "bin/atuin")]
    #[case::shim_then_link(Entry::Shim, Entry::Link, "bin/atuin")]
    #[case::other_then_link(Entry::Other, Entry::Link, "real/atuin")]
    #[tokio::test]
    async fn the_first_shim_wins_over_a_link(
        in_path: InPath,
        #[case] first: Entry,
        #[case] second: Entry,
        #[case] want: &str,
    ) {
        let later = in_path.dir.path().join("later");
        std::fs::create_dir(&later).unwrap();
        let elsewhere = in_path.dir.path().join("elsewhere");
        std::fs::write(&elsewhere, "").unwrap();
        for (dir, entry) in [(&in_path.bin, first), (&later, second)] {
            let link = dir.join("atuin");
            match entry {
                Entry::Link => std::os::unix::fs::symlink(&in_path.real, link).unwrap(),
                Entry::Shim => {
                    let _ = write_shim(dir, &answer(in_path.real.display()));
                }
                Entry::Other => {
                    let _ = write_shim(dir, &answer(elsewhere.display()));
                }
            }
        }
        in_path.env.set("PATH", std::env::join_paths([&in_path.bin, &later]).unwrap());

        assert_eq!(calculate(in_path.real).await, in_path.dir.path().join(want));
    }

    /// The probe runs the shim with only what a service manager would give it.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_shim_sees_only_home_and_the_system_path(in_path: InPath) {
        let body = format!(
            r#"[ "$PATH" = {SYSTEM_PATH} ] && [ "$HOME" = '{}' ] && [ -z "$CARGO$USER" ] && {}"#,
            in_path.dir.path().display(),
            answer(in_path.real.display())
        );
        write_shim(&in_path.bin, &body);

        assert_eq!(calculate(in_path.real).await, in_path.bin.join("atuin"));
    }

    /// An `atuin` that floods its output is given up on once its answer is longer than any path,
    /// without waiting out the timeout.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn an_answer_longer_than_a_path_is_given_up_on(in_path: InPath) {
        write_shim(&in_path.bin, "exec yes");

        let started = std::time::Instant::now();
        let found = calculate_launch_path(Ok(in_path.real.clone()), Duration::from_secs(60)).await;
        assert_eq!(found.unwrap(), in_path.real);
        assert!(started.elapsed() < Duration::from_secs(30), "{:?}", started.elapsed());
    }

    /// The first `atuin` in `PATH`, for [`a_new_version_calculates_for_itself`].
    #[cfg(unix)]
    #[derive(Debug, Clone, Copy)]
    enum First {
        /// A shim that replaces itself with the new version.
        Exec,
        /// A shim that runs the new version as its child.
        Fork,
        /// A link to the new version.
        Link,
        /// A shim that fails.
        Fails,
    }

    /// When the running executable is gone, for example after an upgrade, the first `atuin` on
    /// `PATH` says which executable is new, and the result is what that executable would calculate
    /// for itself. `later/atuin` is a shim that runs the new version. Every probe appends a line
    /// to `probes`, and each `atuin` is probed at most once.
    #[cfg(unix)]
    #[rstest]
    #[case::exec(First::Exec, Some("bin/atuin"), 1)]
    #[case::fork(First::Fork, Some("new/atuin"), 1)]
    #[case::link(First::Link, Some("later/atuin"), 2)]
    #[case::fails(First::Fails, None, 1)]
    #[tokio::test]
    async fn a_new_version_calculates_for_itself(
        in_path: InPath,
        #[case] first: First,
        #[case] want: Option<&str>,
        #[case] probes: usize,
    ) {
        let dir = in_path.dir.path();
        let (new, later) = (dir.join("new"), dir.join("later"));
        std::fs::create_dir(&new).unwrap();
        std::fs::create_dir(&later).unwrap();
        let log = r#"echo >> "$HOME/probes""#;
        let answer = answer(new.join("atuin").display());
        // The new version answers for itself, so it keeps the pid that it was started with.
        write_shim(&new, &format!("{log}; {answer}"));
        let quoted = shlex::try_quote(&answer).unwrap();
        let body = match first {
            First::Exec => format!("{log}; exec sh -c {quoted}"),
            First::Fork => format!("{log}; sh -c {quoted}"),
            First::Link => {
                std::os::unix::fs::symlink(new.join("atuin"), in_path.bin.join("atuin")).unwrap();
                String::new()
            }
            First::Fails => format!("{log}; exit 1"),
        };
        if !matches!(first, First::Link) {
            write_shim(&in_path.bin, &body);
        }
        write_shim(&later, &format!("{log}; exec sh -c {quoted}"));
        in_path.env.set("PATH", std::env::join_paths([&in_path.bin, &later]).unwrap());
        std::fs::remove_file(&in_path.real).unwrap();

        // With no new version found, the result is an error, since the old path can't be run.
        let found = calculate_launch_path(Ok(in_path.real), Duration::from_secs(30)).await;
        assert_eq!(found.ok(), want.map(|want| dir.join(want)));
        let logged = std::fs::read_to_string(dir.join("probes")).unwrap_or_default();
        assert_eq!(logged.lines().count(), probes);
    }

    /// When the OS can't say where the running executable is, the first `atuin` in `PATH` takes its
    /// place. With no `atuin` to ask, the error is the result.
    #[cfg(unix)]
    #[rstest]
    #[case::answers(Some("bin/atuin"))]
    #[case::no_atuin(None)]
    #[tokio::test]
    async fn an_unknown_executable_is_found_from_path(in_path: InPath, #[case] want: Option<&str>) {
        if want.is_some() {
            write_shim(&in_path.bin, &answer(in_path.real.display()));
        }
        let unknown = Err(std::io::Error::other("unknown"));

        let found = calculate_launch_path(unknown, Duration::from_secs(30)).await;
        match want {
            Some(want) => assert_eq!(found.unwrap(), in_path.dir.path().join(want)),
            None => assert_eq!(found.unwrap_err().to_string(), "unknown"),
        }
    }

    /// A shim whose own path is UTF-8 can run an atuin whose path isn't, since the stored path is
    /// the shim's. APFS rejects names that aren't UTF-8, so this can't run on macOS.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[rstest]
    #[tokio::test]
    async fn a_shim_that_runs_an_atuin_whose_path_is_not_utf8(in_path: InPath) {
        use std::os::unix::ffi::OsStrExt as _;

        let real = in_path.dir.path().join(std::ffi::OsStr::from_bytes(b"real\xff"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("atuin"), "").unwrap();
        set_mode(&real.join("atuin"), 0o755);
        // `printf` writes `\377` as the byte 0xff.
        write_shim(
            &in_path.bin,
            &format!(r#"printf '%s\0{}/real\377/atuin' "$$""#, in_path.dir.path().display()),
        );

        assert_eq!(calculate(real.joined("atuin")).await, in_path.bin.join("atuin"));
    }

    /// The shim picks its version in the home directory.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_shim_runs_in_the_home_directory(in_path: InPath) {
        let body = format!(
            r#"[ "$PWD" = '{}' ] && {}"#,
            in_path.dir.path().display(),
            answer(in_path.real.display())
        );
        write_shim(&in_path.bin, &body);

        assert_eq!(calculate(in_path.real).await, in_path.bin.join("atuin"));
    }

    /// The ways that a probe can end before the shim does.
    #[cfg(unix)]
    #[derive(Debug, Clone, Copy)]
    enum ProbeEnd {
        /// The probe gives up on the shim.
        TimedOut,
        /// The future that runs the probe is dropped.
        Cancelled,
    }

    /// A probe that ends before the shim does kills the shim's children, too.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_shim_that_hangs_is_killed_with_its_children(
        in_path: InPath,
        #[values(ProbeEnd::TimedOut, ProbeEnd::Cancelled)] end: ProbeEnd,
    ) {
        // The shell waits on `sleep` instead of becoming it, so `sleep` is the shim's own child.
        let pidfile = in_path.dir.path().join("sleep.pid");
        write_shim(&in_path.bin, &format!("sleep 30 & echo $! > '{}'; wait", pidfile.display()));

        match end {
            ProbeEnd::TimedOut => {
                let calculated =
                    calculate_launch_path(Ok(in_path.real.clone()), Duration::from_secs(1)).await;
                assert_eq!(calculated.unwrap(), in_path.real);
            }
            ProbeEnd::Cancelled => {
                let probe =
                    calculate_launch_path(Ok(in_path.real.clone()), Duration::from_secs(30));
                assert!(tokio::time::timeout(Duration::from_secs(1), probe).await.is_err());
            }
        }
        let sleep = std::fs::read_to_string(pidfile).unwrap();
        // Killed, `sleep` stays a zombie until whatever adopted it reaps it.
        for _ in 0..100 {
            let ps = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", sleep.trim()])
                .output()
                .unwrap();
            let state = String::from_utf8(ps.stdout).unwrap();
            if state.trim().is_empty() || state.starts_with('Z') {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the shim's child outlived the probe");
    }
}
