//! The files that start desktop capture with each session on Linux. Setup writes them, and
//! `atuin desktop-capture login`, which each of them runs, keeps the environment file up to date.

mod config_home;
mod created_dirs;
mod exec;
mod marked;
mod syntax;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atuin_common::atuin_exe::{LaunchPathError, launch_path, location_env};
use atuin_common::dirs::{HomeError, try_home_dir};
use atuin_common::fs::ReplaceError;
use atuin_common::path::PathBufExt as _;
#[cfg(all(unix, not(target_os = "macos")))]
use atuin_common::shell::Shell;
use atuin_desktop_capture::{Desktop, Platform as _};
use thiserror::Error;

use self::marked::{Placement, Syntax};
use self::syntax::ENVIRONMENT_CONTENT;
#[cfg(test)]
pub use self::syntax::IN_DESKTOP_SESSION_FLAG;
pub use self::syntax::{Launch, MARKER};
use crate::i18n::fl;

/// A variable that each session needs for capture, at the value that capture needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variable {
    pub name: &'static str,
    pub value: &'static str,
}

/// The variables that each session needs for capture, which the desktop backend names.
pub fn variables() -> impl Iterator<Item = Variable> {
    Desktop::session_variables().iter().filter_map(|variable| {
        Some(Variable {
            name: variable.name,
            value: variable.setup_value()?,
        })
    })
}

/// The user's `environment.d`, relative to the home directory. It is always under `~/.config`,
/// because the service manager reads it before anything could set `XDG_CONFIG_HOME`.
const ENVIRONMENT_DIR: &str = ".config/environment.d";

/// The files that belong to setup alone, by their names.
pub const ENVIRONMENT_FILE: &str = "60-atuin-desktop-capture.conf";
const AUTOSTART_ENTRY: &str = "atuin-desktop-capture.desktop";
const FISH_FILE: &str = "atuin-desktop-capture.fish";

/// The files of Bash's login shells, which Bash reads before `~/.profile`.
const BASH_FILES: [&str; 2] = [".bash_profile", ".bash_login"];

/// One file that setup changes: its content before and after, where `None` means no file.
///
/// The content is bytes, because the user's files may be in any encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

impl Edit {
    fn changes(&self) -> bool {
        self.before != self.after
    }

    /// Writes or removes the file, unless the file no longer holds the content that the edit
    /// was planned from.
    ///
    /// The new content goes to a temporary file that replaces the file at once. The edit writes
    /// through a symbolic link, so that the link of a dotfile manager stays a link, and keeps
    /// the file's permissions. A link to a file that doesn't exist yet gets the file. A new file
    /// gets the permissions that the umask leaves, and the directories that it needs get the
    /// permissions that the user's umask leaves.
    fn apply(&self) -> Result<(), SessionError> {
        let changed = || SessionError::Changed {
            path: self.path.clone(),
        };
        let write_failed = |source| SessionError::Write {
            path: self.path.clone(),
            source,
        };
        let target = atuin_common::fs::follow_links(self.path.clone()).map_err(write_failed)?;
        match (&self.before, &self.after) {
            (_, None) => {
                if read(&target)? != self.before {
                    return Err(changed());
                }
                match std::fs::remove_file(&self.path) {
                    Err(err) if err.kind() != io::ErrorKind::NotFound => Err(write_failed(err)),
                    _ => Ok(()),
                }
            }
            (None, Some(content)) => {
                if let Some(dir) = target.parent() {
                    created_dirs::create(dir)?;
                }
                match atuin_common::fs::write_new_with_umask(&target, content) {
                    Ok(()) => Ok(()),
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(changed()),
                    Err(err) => Err(write_failed(err)),
                }
            }
            (Some(before), Some(content)) => {
                let replaced =
                    atuin_common::fs::replace(&target, content, |now| now == before.as_slice());
                match replaced {
                    // The file holds the new content, and only a crash could still lose it.
                    Ok(()) | Err(ReplaceError::Unsynced(_)) => Ok(()),
                    Err(ReplaceError::NotReplaced(err))
                        if matches!(
                            err.kind(),
                            io::ErrorKind::Interrupted | io::ErrorKind::NotFound
                        ) =>
                    {
                        Err(changed())
                    }
                    Err(ReplaceError::NotReplaced(err)) => Err(write_failed(err)),
                }
            }
        }
    }
}

/// The files that start capture at login, as setup finds them.
#[derive(Debug, Default)]
pub struct LoginFiles {
    edits: Vec<Edit>,
    /// The files that setup could not read, and so leaves alone.
    unreadable: Vec<SessionError>,
    /// The line of `sh` that the user has to add to the login file of a login shell that setup
    /// does not know. `None` when setup knows the login shell.
    pub unknown_shell: Option<String>,
    /// The `.zprofile` in the home that setup writes to because Zsh didn't say its `ZDOTDIR`,
    /// when a file of Zsh may set `ZDOTDIR`.
    pub guessed_zprofile: Option<PathBuf>,
    /// The files whose block comes after an `exec` that replaces the shell, because setup couldn't
    /// tell where the statement of the `exec` starts.
    ///
    /// The user has to move the block above the `exec`.
    pub blocks_after_exec: Vec<PathBuf>,
    /// The `XDG_CONFIG_HOME` of this session, when the variable names a directory other than
    /// `~/.config` that the user's login files don't set.
    ///
    /// A login that doesn't set the variable looks for setup's files in `~/.config`.
    pub config_home: Option<ConfigHome>,
    /// Whether applying the files also removes the directories that setup created and that end
    /// up empty.
    removes_created_dirs: bool,
}

/// A directory of `XDG_CONFIG_HOME` that setup writes to.
#[derive(Debug)]
pub struct ConfigHome {
    pub dir: PathBuf,
    /// The files in the directory that setup writes.
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("{}", fl!("desktop-capture-session-no-executable"))]
    Executable(#[source] LaunchPathError),

    #[error("{}", fl!("desktop-capture-session-path-not-utf8", path = path.display().to_string()))]
    NotUtf8 {
        path: PathBuf,
    },

    #[error("{}", fl!("desktop-capture-session-no-home"))]
    Home(#[from] HomeError),

    #[error("{}", fl!("desktop-capture-session-read-failed", path = path.display().to_string()))]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("{}", fl!("desktop-capture-session-write-failed", path = path.display().to_string()))]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("{}", fl!("desktop-capture-session-changed", path = path.display().to_string()))]
    Changed {
        path: PathBuf,
    },
}

/// What [`LoginFiles::apply`] did.
#[derive(Debug, Default)]
pub struct Applied {
    /// How many files it changed.
    pub changed: usize,
    /// The files that it could not change.
    pub failures: Vec<SessionError>,
}

/// What setup's plan does with an autostart entry that the desktop turned off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Autostart {
    /// Turn the entry back on, because the user asked for capture at login, as when setup turns
    /// capture on.
    TurnBackOn,
    /// Keep the desktop's choice, as when setup keeps capture on for another reason.
    KeepDesktopsChoice,
}

impl LoginFiles {
    /// The files that make every session of this user start capture at login, as this `atuin`
    /// would write them.
    ///
    /// The plan puts back a block that the user took out. `autostart` says what happens to an
    /// autostart entry that the desktop turned off.
    pub async fn planned(autostart: Autostart) -> Result<Self, SessionError> {
        let atuin = launch_path().await.map_err(SessionError::Executable)?;
        let Some(atuin) = atuin.to_str() else {
            return Err(SessionError::NotUtf8 {
                path: atuin.to_path_buf(),
            });
        };
        let env = location_env(atuin_common::env::vars_utf8());
        let launch = Launch { atuin, env: &env };
        let places = Places::find()?;
        let login_shell = LoginShellFile::find(&places).await;

        let sh = launch.sh_line();
        let mut files = Self::default();
        files.add_marked(places.profile(), &sh, Syntax::Sh);
        let mut unknown = false;
        match login_shell {
            LoginShellFile::Profile => {}
            LoginShellFile::Sh(path) => files.add_marked(path, &sh, Syntax::Sh),
            LoginShellFile::Nu(path) => files.add_marked(path, &launch.nu_line(), Syntax::Nu),
            LoginShellFile::Fish(path) => {
                files.add(own(path, |_| Some(launch.fish_file().into_bytes())));
            }
            LoginShellFile::ZshInHome(path) => {
                if sets_zdotdir(&places.home) {
                    files.guessed_zprofile = Some(path.clone());
                }
                files.add_marked(path, &sh, Syntax::Sh);
            }
            LoginShellFile::Unknown => unknown = true,
        }
        if Path::new("/etc/X11/Xsession.d").is_dir() {
            files.add_marked(places.xsessionrc(), &sh, Syntax::Sh);
        }
        if is_systemd_init() {
            files.add(own(places.environment_file(), |_| {
                Some(ENVIRONMENT_CONTENT.as_bytes().to_vec())
            }));
        }
        files.add(own(places.autostart_entry(), |before| {
            // The path belongs to setup, so a desktop that turned the entry off may have
            // rewritten it without setup's marker.
            let kept = before.filter(|_| autostart == Autostart::KeepDesktopsChoice);
            Some(launch.autostart_entry(kept).into_bytes())
        }));
        files.unknown_shell = unknown.then_some(sh);
        files.config_home =
            places.config_home().filter(|_| !config_home::set_at_login(&places.home)).map(|dir| {
                let files = files.edits.iter().filter(|edit| edit.path.starts_with(&dir));
                let files = files.map(|edit| edit.path.clone()).collect();
                ConfigHome { dir, files }
            });
        Ok(files)
    }

    /// Removes what setup wrote to every file that setup may have written to.
    pub async fn removal() -> Result<Self, SessionError> {
        let places = Places::find()?;
        let mut files = Self {
            removes_created_dirs: true,
            ..Self::default()
        };
        files.add(shared_removal(places.profile(), Emptied::Remove));
        for kind in ShellKind::ALL {
            match kind {
                ShellKind::Posix => {}
                // Setup writes to a file of Bash only when the file exists, and an empty file
                // still keeps Bash from reading the files after it.
                ShellKind::Bash => {
                    for name in BASH_FILES {
                        let path = places.home.join(name);
                        files.add(shared_removal(path, Emptied::Keep));
                    }
                }
                // Setup writes to the `.zprofile` in the home when Zsh doesn't say its `ZDOTDIR`
                // in time, so removal looks there as well as in the `ZDOTDIR`.
                ShellKind::Zsh => {
                    let home_zprofile = places.home.join(".zprofile");
                    let asks_zsh =
                        sets_zdotdir(&places.home) || login_shell_name().as_deref() == Some("zsh");
                    if asks_zsh
                        && let Some(dir) = zdotdir(&places.home).await
                        && dir != places.home
                    {
                        files.add(shared_removal(dir.joined(".zprofile"), Emptied::Remove));
                    }
                    files.add(shared_removal(home_zprofile, Emptied::Remove));
                }
                ShellKind::Nu => {
                    let path = places.nu_login();
                    files.add(shared_removal(path, Emptied::Remove));
                }
                ShellKind::Fish => files.add_setups(places.fish_file()),
            }
        }
        files.add(shared_removal(places.xsessionrc(), Emptied::Remove));
        files.add_setups(places.environment_file());
        files.add_setups(places.autostart_entry());
        Ok(files)
    }

    /// The edits that change a file.
    pub fn changes(&self) -> impl Iterator<Item = &Edit> {
        self.edits.iter().filter(|edit| edit.changes())
    }

    /// The files that setup could not read, and so leaves alone.
    pub fn unreadable(&self) -> &[SessionError] {
        &self.unreadable
    }

    /// Writes each changed file, or removes it. A file that fails leaves the others to go on.
    ///
    /// A removal also removes the directories that setup created for its files, such as
    /// `~/.config/autostart`, when they are empty.
    pub fn apply(&self) -> Applied {
        let mut applied = Applied::default();
        for edit in self.changes() {
            match edit.apply() {
                Ok(()) => applied.changed += 1,
                Err(err) => applied.failures.push(err),
            }
        }
        if self.removes_created_dirs
            && let Err(err) = created_dirs::remove_empty()
        {
            applied.failures.push(err);
        }
        applied
    }

    /// Adds the edit that puts setup's block with `line` into `path`, a file of `syntax` that the
    /// user also writes.
    fn add_marked(&mut self, path: PathBuf, line: &str, syntax: Syntax) {
        let mut placement = Placement::BeforeAnyExec;
        let edit = shared(path, |text| {
            let placed = marked::with_line(text, line, syntax);
            placement = placed.placement;
            placed.text
        });
        if let Ok(edit) = &edit
            && placement == Placement::AfterExec
        {
            self.blocks_after_exec.push(edit.path.clone());
        }
        self.add(edit);
    }

    fn add(&mut self, edit: Result<Edit, SessionError>) {
        match edit {
            Ok(edit) => self.edits.push(edit),
            Err(err) => self.unreadable.push(err),
        }
    }

    /// Adds the removal of `path`, a file that belongs to setup alone, when setup wrote it.
    fn add_setups(&mut self, path: PathBuf) {
        match own(path, |_| None) {
            Ok(edit) if edit.before.as_deref().is_some_and(is_setups_file) => {
                self.edits.push(edit);
            }
            Ok(_) => {}
            Err(err) => self.unreadable.push(err),
        }
    }
}

/// Rewrites Atuin's file in `environment.d` when the file is setup's and differs from what this
/// build would write.
pub fn refresh_environment_file() -> Result<(), SessionError> {
    let path = try_home_dir()?.joined(ENVIRONMENT_DIR).joined(ENVIRONMENT_FILE);
    let Some(existing) = read(&path)? else {
        return Ok(());
    };
    if existing == ENVIRONMENT_CONTENT.as_bytes() || !is_setups_file(&existing) {
        return Ok(());
    }
    Edit {
        path,
        before: Some(existing),
        after: Some(ENVIRONMENT_CONTENT.as_bytes().to_vec()),
    }
    .apply()
}

/// Whether `content`, of a file that belongs to setup alone, is setup's: its first line names
/// setup, as every file that any build of setup wrote does.
fn is_setups_file(content: &[u8]) -> bool {
    let first = content.split(|&byte| byte == b'\n').next().unwrap_or_default();
    syntax::is_first_line_of_setups_file(first)
}

/// An edit of a file that the user also writes, which `edit` turns from its content into the new
/// content. A missing file counts as empty.
fn shared(path: PathBuf, edit: impl FnOnce(&[u8]) -> Vec<u8>) -> Result<Edit, SessionError> {
    let before = read(&path)?;
    let after = Some(edit(before.as_deref().unwrap_or_default()));
    Ok(Edit {
        path,
        before,
        after,
    })
}

/// What the removal of setup's blocks does with a file that held nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emptied {
    /// The file goes, because setup may have created it.
    Remove,
    /// The file stays, empty.
    Keep,
}

/// The removal of setup's blocks from a file that the user also writes.
///
/// A missing file stays missing. A file that held only setup's blocks goes as `emptied` says,
/// and stays when it is a symbolic link, which a dotfile manager may keep.
fn shared_removal(path: PathBuf, emptied: Emptied) -> Result<Edit, SessionError> {
    let before = read(&path)?;
    let keeps = emptied == Emptied::Keep || path.is_symlink();
    let after = before.as_deref().and_then(|text| {
        let rest = marked::without_line(text);
        (keeps || text.is_empty() || !rest.is_empty()).then_some(rest)
    });
    Ok(Edit {
        path,
        before,
        after,
    })
}

/// An edit of a file that belongs to setup alone, which `after` makes from the file's content.
fn own(
    path: PathBuf,
    after: impl FnOnce(Option<&[u8]>) -> Option<Vec<u8>>,
) -> Result<Edit, SessionError> {
    let before = read(&path)?;
    let after = after(before.as_deref());
    Ok(Edit {
        path,
        before,
        after,
    })
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, SessionError> {
    match std::fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SessionError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Whether setup writes its file in `environment.d`, which needs systemd's service manager.
pub fn writes_environment_file() -> bool {
    is_systemd_init()
}

fn is_systemd_init() -> bool {
    #[cfg(target_os = "linux")]
    return atuin_common::os::linux::systemd_is_init();
    #[cfg(not(target_os = "linux"))]
    false
}

/// Where the files of a session are.
struct Places {
    home: PathBuf,
    /// `$XDG_CONFIG_HOME`, or `~/.config`, where fish, Nushell, and the desktop's autostart look.
    config: PathBuf,
}

impl Places {
    fn find() -> Result<Self, SessionError> {
        let home = try_home_dir()?;
        let config = atuin_common::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        Ok(Self { home, config })
    }

    fn profile(&self) -> PathBuf {
        self.home.join(".profile")
    }

    fn xsessionrc(&self) -> PathBuf {
        self.home.join(".xsessionrc")
    }

    fn environment_dir(&self) -> PathBuf {
        self.home.join(ENVIRONMENT_DIR)
    }

    fn environment_file(&self) -> PathBuf {
        self.environment_dir().joined(ENVIRONMENT_FILE)
    }

    fn autostart_entry(&self) -> PathBuf {
        self.config.join("autostart").joined(AUTOSTART_ENTRY)
    }

    fn fish_file(&self) -> PathBuf {
        self.config.join("fish/conf.d").joined(FISH_FILE)
    }

    fn nu_login(&self) -> PathBuf {
        self.config.join("nushell/login.nu")
    }

    /// The configuration directory, when it isn't `~/.config`.
    fn config_home(&self) -> Option<PathBuf> {
        let default =
            self.config.strip_prefix(&self.home).is_ok_and(|rest| rest == Path::new(".config"));
        (!default).then(|| self.config.clone())
    }
}

/// The login shells that setup knows, by the file that each reads at login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    /// A POSIX shell, which reads `~/.profile`.
    Posix,
    Bash,
    Zsh,
    Fish,
    Nu,
}

impl ShellKind {
    const ALL: [Self; 5] = [Self::Posix, Self::Bash, Self::Zsh, Self::Fish, Self::Nu];

    /// The kind of the shell whose executable has the file name `name`.
    fn named(name: &str) -> Option<Self> {
        match name {
            "sh" | "dash" | "ksh" | "ksh93" | "mksh" | "lksh" | "oksh" | "posh" | "yash" => {
                Some(Self::Posix)
            }
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            "fish" => Some(Self::Fish),
            "nu" => Some(Self::Nu),
            _ => None,
        }
    }
}

/// The file name of the user's login shell, from the user's entry in the password database,
/// because `$SHELL` describes only the current session.
fn login_shell_name() -> Option<String> {
    #[cfg(all(unix, not(target_os = "macos")))]
    return Shell::login_shell_name().ok();
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    None
}

/// The file of the user's login shell that setup writes to, besides `~/.profile`.
enum LoginShellFile {
    /// The login shell reads `~/.profile`.
    Profile,
    /// A file of `sh` syntax.
    Sh(PathBuf),
    /// A file of Nushell.
    Nu(PathBuf),
    /// A file of fish's `conf.d`, which belongs to setup alone.
    Fish(PathBuf),
    /// Zsh's `.zprofile` in the home, because Zsh didn't say where its `ZDOTDIR` is.
    ZshInHome(PathBuf),
    /// A login shell that setup does not know.
    Unknown,
}

impl LoginShellFile {
    async fn find(places: &Places) -> Self {
        match login_shell_name().as_deref().and_then(ShellKind::named) {
            Some(ShellKind::Posix) => Self::Profile,
            // Bash reads the first of these that exists, and `~/.profile` when neither does.
            Some(ShellKind::Bash) => BASH_FILES
                .into_iter()
                .map(|name| places.home.join(name))
                .find(|path| path.exists())
                .map_or(Self::Profile, Self::Sh),
            Some(ShellKind::Zsh) => zdotdir(&places.home).await.map_or_else(
                || Self::ZshInHome(places.home.join(".zprofile")),
                |dir| Self::Sh(dir.joined(".zprofile")),
            ),
            Some(ShellKind::Fish) => Self::Fish(places.fish_file()),
            Some(ShellKind::Nu) => Self::Nu(places.nu_login()),
            None => Self::Unknown,
        }
    }
}

/// The files of Zsh that may set `ZDOTDIR` for the user, besides the user's `.zshenv`. Debian
/// builds Zsh to read the first, and other builds read the second.
const SYSTEM_ZSHENVS: [&str; 2] = ["/etc/zsh/zshenv", "/etc/zshenv"];

/// Whether a file of Zsh may set `ZDOTDIR` for the user whose home is `home`.
fn sets_zdotdir(home: &Path) -> bool {
    home.join(".zshenv").exists() || SYSTEM_ZSHENVS.iter().any(|path| Path::new(path).exists())
}

/// The text that comes right before the directory that Zsh prints for [`zdotdir`].
const ZDOTDIR_MARKER: &[u8] = b"\natuin-zdotdir=";

/// The script that prints the directory for [`zdotdir`]. `printf` turns the `\n` of the format
/// into the newline of [`ZDOTDIR_MARKER`].
const ZDOTDIR_SCRIPT: &str = "printf '\\natuin-zdotdir=%s' \"${ZDOTDIR:-$HOME}\"";

/// The directory that Zsh reads `.zprofile` from: `$ZDOTDIR` as the user's `.zshenv` leaves it,
/// or `home`. `None` when Zsh doesn't say.
async fn zdotdir(home: &Path) -> Option<PathBuf> {
    const TIMEOUT: Duration = Duration::from_secs(2);
    // `.zshenv` can print text of its own, so the directory follows a marker.
    let output = tokio::process::Command::new("zsh")
        .env_remove("ZDOTDIR")
        .env("HOME", home)
        .args(["-c", ZDOTDIR_SCRIPT])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(TIMEOUT, output).await {
        Ok(Ok(output)) if output.status.success() => printed_zdotdir(&output.stdout),
        _ => None,
    }
}

/// The directory in `stdout`, which Zsh printed for [`zdotdir`] after any text of `.zshenv`.
fn printed_zdotdir(stdout: &[u8]) -> Option<PathBuf> {
    let marker = stdout.windows(ZDOTDIR_MARKER.len()).rposition(|bytes| bytes == ZDOTDIR_MARKER)?;
    let dir = std::str::from_utf8(&stdout[marker + ZDOTDIR_MARKER.len()..]).ok()?;
    (!dir.contains('\n') && Path::new(dir).is_absolute()).then(|| PathBuf::from(dir))
}

/// Prints the variables for the shell that reads them.
pub fn write_variables(out: &mut impl io::Write, shell: LoginShell) -> io::Result<()> {
    match shell {
        LoginShell::Sh => {
            for Variable { name, value } in variables() {
                writeln!(out, "export {name}={value}")?;
            }
        }
        LoginShell::Fish => {
            for Variable { name, value } in variables() {
                writeln!(out, "set -gx {name} {value}")?;
            }
        }
        LoginShell::Nu => {
            out.write_all(b"{")?;
            for (index, Variable { name, value }) in variables().enumerate() {
                if index > 0 {
                    out.write_all(b",")?;
                }
                serde_json::to_writer(&mut *out, name)?;
                out.write_all(b":")?;
                serde_json::to_writer(&mut *out, value)?;
            }
            out.write_all(b"}\n")?;
        }
    }
    Ok(())
}

/// The shell whose syntax `login` prints the variables in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LoginShell {
    Sh,
    Fish,
    Nu,
}

#[cfg(test)]
mod tests {
    use atuin_common::env::MockEnv;
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::syntax::MARKER;
    use super::*;

    /// A home of its own, which the mock environment names as `HOME` and the configuration
    /// directory.
    struct Home {
        dir: TempDir,
        _env: MockEnv,
    }

    impl Home {
        fn path(&self, relative: &str) -> PathBuf {
            self.dir.path().join(relative)
        }

        fn write(&self, relative: &str, content: impl AsRef<[u8]>) {
            let path = self.path(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        fn read(&self, relative: &str) -> Option<String> {
            std::fs::read_to_string(self.path(relative)).ok()
        }
    }

    /// Runs `future` on a runtime of this thread.
    ///
    /// A test with cases calls it, because rstest expects such a test's future to be `Send`,
    /// and the mock environment cannot leave its thread.
    fn block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
    }

    #[fixture]
    fn home() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let env = MockEnv::install();
        env.set("HOME", dir.path());
        Home { dir, _env: env }
    }

    const BLOCK: &str = "# Added by atuin setup: starts desktop capture at login.\nif [ -x \
                         /opt/atuin ]; then eval \"$(/opt/atuin desktop-capture login --shell sh \
                         2>/dev/null)\"; fi\n";

    /// A second run of setup finds every file as the first run left it.
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn a_second_setup_changes_nothing(home: Home) {
        home.write(".profile", "export EDITOR=vi\n");
        let first = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap();
        assert!(first.changes().any(|edit| edit.path.ends_with(".profile")));
        let applied = first.apply();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);

        let second = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap();

        let left: Vec<_> = second.changes().map(|edit| &edit.path).collect();
        assert!(left.is_empty(), "{left:?}");
        assert!(home.read(".profile").unwrap().starts_with("export EDITOR=vi\n\n# Added by"));
    }

    /// A file in another encoding, such as a Latin-1 `.profile`, gets setup's block and keeps
    /// its own bytes.
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn a_file_in_another_encoding_gets_the_block(home: Home) {
        home.write(".profile", b"export NAME=Jos\xe9\n");

        let files = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap();
        assert!(files.unreadable().is_empty(), "{:?}", files.unreadable());
        let applied = files.apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        let profile = std::fs::read(home.path(".profile")).unwrap();
        assert!(profile.starts_with(b"export NAME=Jos\xe9\n\n# Added by"));
        assert_eq!(LoginFiles::planned(Autostart::TurnBackOn).await.unwrap().changes().count(), 0);
    }

    /// A file that setup cannot read stays as it is, and setup still writes the other files.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn an_unreadable_file_leaves_the_others_to_go_on(home: Home) {
        use std::os::unix::fs::PermissionsExt as _;

        home.write(".profile", "export EDITOR=vi\n");
        let profile = home.path(".profile");
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&profile).is_ok() {
            // A user that reads every file, such as root, cannot test this.
            return;
        }

        let files = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap();
        let applied = files.apply();

        assert!(
            matches!(files.unreadable(), [SessionError::Read { path, .. }] if *path == profile),
            "{:?}",
            files.unreadable()
        );
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert!(home.read(".config/autostart/atuin-desktop-capture.desktop").is_some());
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(home.read(".profile").as_deref(), Some("export EDITOR=vi\n"));
    }

    /// A file that changed after setup read it, such as while setup waited for an answer, stays
    /// as it is, and the other files still change.
    #[rstest]
    #[case::edited(Some("export EDITOR=nano\n"))]
    #[case::removed(None)]
    fn a_file_that_changed_meanwhile_stays(home: Home, #[case] meanwhile: Option<&str>) {
        home.write(".profile", "export EDITOR=vi\n");
        let files = block_on(LoginFiles::planned(Autostart::TurnBackOn)).unwrap();
        match meanwhile {
            Some(content) => home.write(".profile", content),
            None => std::fs::remove_file(home.path(".profile")).unwrap(),
        }

        let applied = files.apply();

        assert!(
            matches!(&applied.failures[..], [SessionError::Changed { path }] if path.ends_with(".profile")),
            "{:?}",
            applied.failures
        );
        assert_eq!(home.read(".profile").as_deref(), meanwhile);
        assert!(home.read(".config/autostart/atuin-desktop-capture.desktop").is_some());
    }

    /// A file that appeared after setup found none stays as it is.
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn a_file_that_appeared_meanwhile_stays(home: Home) {
        let files = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap();
        home.write(".profile", "export EDITOR=vi\n");

        let applied = files.apply();

        assert!(
            matches!(&applied.failures[..], [SessionError::Changed { path }] if path.ends_with(".profile")),
            "{:?}",
            applied.failures
        );
        assert_eq!(home.read(".profile").as_deref(), Some("export EDITOR=vi\n"));
    }

    /// A symbolic link, such as one that a dotfile manager keeps, stays a link, and the file
    /// that it points to keeps its permissions.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn a_linked_file_stays_a_link_with_its_permissions(home: Home) {
        use std::os::unix::fs::PermissionsExt as _;

        home.write("dotfiles/profile", "export EDITOR=vi\n");
        let target = home.path("dotfiles/profile");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&target, home.path(".profile")).unwrap();

        let applied = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap().apply();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);

        assert!(home.path(".profile").is_symlink());
        assert!(home.read("dotfiles/profile").unwrap().contains(MARKER));
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);

        // Removal takes setup's block out of the linked file and keeps the link.
        let applied = LoginFiles::removal().await.unwrap().apply();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert!(home.path(".profile").is_symlink());
        assert_eq!(home.read("dotfiles/profile").as_deref(), Some("export EDITOR=vi\n"));
    }

    /// Removal takes out what setup wrote, and leaves the user's own lines and files.
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn removal_takes_out_only_what_setup_wrote(home: Home) {
        home.write(".profile", format!("export EDITOR=vi\n\n{BLOCK}"));
        home.write(".xsessionrc", BLOCK);
        home.write(".config/fish/conf.d/atuin-desktop-capture.fish", "set -gx MINE 1\n");
        home.write(
            ".config/autostart/atuin-desktop-capture.desktop",
            format!("{}\n[Desktop Entry]\n", syntax::MARKER),
        );
        home.write(".config/environment.d/60-atuin-desktop-capture.conf", ENVIRONMENT_CONTENT);

        let removal = LoginFiles::removal().await.unwrap();
        let applied = removal.apply();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);

        assert_eq!(home.read(".profile").as_deref(), Some("export EDITOR=vi\n"));
        assert_eq!(home.read(".xsessionrc"), None);
        assert_eq!(
            home.read(".config/fish/conf.d/atuin-desktop-capture.fish").as_deref(),
            Some("set -gx MINE 1\n")
        );
        assert_eq!(home.read(".config/autostart/atuin-desktop-capture.desktop"), None);
        assert_eq!(home.read(".config/environment.d/60-atuin-desktop-capture.conf"), None);
        assert_eq!(LoginFiles::removal().await.unwrap().changes().count(), 0);
    }

    /// The directories that setup created get the permissions of the user's umask, and removal
    /// takes them out again once they are empty.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn removal_takes_out_the_directories_that_setup_created(home: Home) {
        use std::os::unix::fs::PermissionsExt as _;

        let applied = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap().apply();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        let mode = std::fs::metadata(home.path(".config")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);

        let applied = LoginFiles::removal().await.unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert!(!home.path(".config").exists());
    }

    /// Removal keeps the directories that existed before setup, including an empty one, and a
    /// directory that setup created and that holds a file of the user's now.
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn removal_keeps_the_directories_of_the_user(home: Home) {
        std::fs::create_dir_all(home.path(".config/environment.d")).unwrap();
        let applied = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap().apply();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        home.write(".config/autostart/mine.desktop", "[Desktop Entry]\n");

        let applied = LoginFiles::removal().await.unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert!(home.path(".config/environment.d").is_dir());
        assert_eq!(
            home.read(".config/autostart/mine.desktop").as_deref(),
            Some("[Desktop Entry]\n")
        );
    }

    const CALL: &str = "if [ -x /opt/atuin ]; then eval \"$(/opt/atuin desktop-capture login \
                        --shell sh 2>/dev/null)\"; fi";

    /// Setup's files that call another `atuin` are found, rewritten, and removed.
    #[rstest]
    fn setups_files_are_rewritten_and_removed(home: Home) {
        let entry = ".config/autostart/atuin-desktop-capture.desktop";
        let environment = ".config/environment.d/60-atuin-desktop-capture.conf";
        let marker = syntax::MARKER;
        let header = syntax::ENVIRONMENT_HEADER;
        home.write(".profile", format!("export EDITOR=vi\n\n{marker}\n{CALL}\n"));
        home.write(entry, format!("{marker}\n[Desktop Entry]\nExec=/opt/atuin\n"));
        home.write(environment, format!("{header}\nACCESSIBILITY_ENABLED=1\n"));

        let plan = block_on(LoginFiles::planned(Autostart::TurnBackOn)).unwrap();
        assert!(plan.changes().any(|edit| edit.path.ends_with(".profile")));
        assert!(plan.changes().any(|edit| edit.path.ends_with(entry)));
        let applied = plan.apply();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        let profile = home.read(".profile").unwrap();
        assert!(profile.contains(marker), "{profile}");
        assert!(!profile.contains(CALL), "{profile}");
        assert!(home.read(entry).unwrap().starts_with(marker));

        let applied = block_on(LoginFiles::removal()).unwrap().apply();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert_eq!(home.read(".profile").as_deref(), Some("export EDITOR=vi\n"));
        assert_eq!(home.read(entry), None);
        assert_eq!(home.read(environment), None);
    }

    /// A file at an owned path whose first line only mentions setup stays untouched.
    #[rstest]
    fn a_file_that_only_mentions_setup_is_not_setups(
        home: Home,
        #[values(
            ".config/fish/conf.d/atuin-desktop-capture.fish",
            ".config/autostart/atuin-desktop-capture.desktop",
            ".config/environment.d/60-atuin-desktop-capture.conf"
        )]
        path: &str,
    ) {
        let content = "# notes on atuin setup\nACCESSIBILITY_ENABLED=0\n";
        home.write(path, content);

        refresh_environment_file().unwrap();
        let removal = block_on(LoginFiles::removal()).unwrap();
        assert!(removal.changes().all(|edit| !edit.path.ends_with(path)));
        assert!(removal.apply().failures.is_empty());

        assert_eq!(home.read(path).as_deref(), Some(content));
    }

    /// Each kind of file that setup alone owns, with the current marker, is found and removed.
    #[rstest]
    fn owned_files_with_the_current_marker_are_removed(
        home: Home,
        #[values(
            ".config/fish/conf.d/atuin-desktop-capture.fish",
            ".config/autostart/atuin-desktop-capture.desktop",
            ".config/environment.d/60-atuin-desktop-capture.conf"
        )]
        path: &str,
    ) {
        let first = if path.contains("environment.d") {
            syntax::ENVIRONMENT_HEADER
        } else {
            syntax::MARKER
        };
        home.write(path, format!("{first}\nrest\n"));

        let applied = block_on(LoginFiles::removal()).unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert_eq!(home.read(path), None);
    }

    #[rstest]
    fn the_environment_content_starts_with_its_header() {
        assert!(ENVIRONMENT_CONTENT.starts_with(syntax::ENVIRONMENT_HEADER));
    }

    /// A file of Bash that held only setup's block stays, empty, because its presence keeps Bash
    /// from reading `~/.profile`. A `.zprofile` that setup may have created goes.
    #[rstest]
    #[case::bash_profile(".bash_profile", Some(""))]
    #[case::bash_login(".bash_login", Some(""))]
    #[case::zprofile(".zprofile", None)]
    fn removal_keeps_an_emptied_file_of_bash(
        home: Home,
        #[case] file: &str,
        #[case] left: Option<&str>,
    ) {
        home.write(file, BLOCK);

        let applied = block_on(LoginFiles::removal()).unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert_eq!(home.read(file).as_deref(), left);
    }

    /// A block after a line that replaces the shell, or at the top, moves to just before that
    /// line.
    #[rstest]
    #[case::appended(format!("# .profile\n\n. ~/.bashrc\n{EXEC}\n{BLOCK}"))]
    #[case::at_the_top(format!("# .profile\n\n{BLOCK}\n. ~/.bashrc\n{EXEC}"))]
    fn a_block_moves_before_the_exec(home: Home, #[case] profile: String) {
        home.write(".profile", profile);

        let applied = block_on(LoginFiles::planned(Autostart::TurnBackOn)).unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        let profile = home.read(".profile").unwrap();
        assert!(
            profile.starts_with(&format!("# .profile\n\n. ~/.bashrc\n{MARKER}\n")),
            "{profile}"
        );
        assert!(profile.ends_with(&format!("fi\n\n{EXEC}")), "{profile}");
    }

    const EXEC: &str = "[ \"$XDG_VTNR\" = 1 ] && exec sway\n";

    /// Explicit setup turns back on an autostart entry that the desktop turned off.
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn setup_turns_the_autostart_entry_back_on(home: Home) {
        let entry = ".config/autostart/atuin-desktop-capture.desktop";
        home.write(
            entry,
            format!("{}\n[Desktop Entry]\nExec=/opt/atuin\nHidden=true\n", syntax::MARKER),
        );

        let applied = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        let entry = home.read(entry).unwrap();
        assert!(!entry.contains("Hidden=true"), "{entry}");
    }

    /// A plan that keeps capture on keeps the autostart entry off when the desktop turned it off,
    /// even when the desktop rewrote the entry without setup's marker.
    #[rstest]
    #[case::with_the_marker(format!("{}\n", syntax::MARKER))]
    #[case::without_the_marker(String::new())]
    fn keeping_capture_on_keeps_the_autostart_entry_off(
        home: Home,
        #[case] leading_comment: String,
    ) {
        let entry = ".config/autostart/atuin-desktop-capture.desktop";
        home.write(
            entry,
            format!("{leading_comment}[Desktop Entry]\nExec=/opt/atuin\nHidden=true\n"),
        );

        let files = block_on(LoginFiles::planned(Autostart::KeepDesktopsChoice)).unwrap();
        let applied = files.apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        let entry = home.read(entry).unwrap();
        assert!(!entry.contains("/opt/atuin"), "{entry}");
        assert!(entry.ends_with("\nHidden=true\n"), "{entry}");
    }

    /// A symbolic link to a file that doesn't exist yet, such as a dotfile manager's, gets the
    /// file, and stays a link.
    #[cfg(unix)]
    #[rstest]
    #[case::absolute(None)]
    #[case::relative(Some("dotfiles/profile"))]
    #[case::through_two_links(Some("dotfiles/link"))]
    fn a_dangling_link_gets_its_file(home: Home, #[case] link: Option<&str>) {
        std::fs::create_dir_all(home.path("dotfiles")).unwrap();
        std::os::unix::fs::symlink("profile", home.path("dotfiles/link")).unwrap();
        match link {
            Some(link) => std::os::unix::fs::symlink(link, home.path(".profile")).unwrap(),
            None => {
                std::os::unix::fs::symlink(home.path("dotfiles/profile"), home.path(".profile"))
                    .unwrap();
            }
        }

        let applied = block_on(LoginFiles::planned(Autostart::TurnBackOn)).unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert!(home.path(".profile").is_symlink());
        assert!(home.read("dotfiles/profile").unwrap().contains(MARKER));
        assert_eq!(
            block_on(LoginFiles::planned(Autostart::TurnBackOn)).unwrap().changes().count(),
            0
        );
    }

    /// A new file gets the permissions that the umask leaves, as a file that a shell creates
    /// does.
    #[cfg(target_os = "linux")]
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn a_new_file_gets_the_umasks_permissions(home: Home) {
        use std::os::unix::fs::PermissionsExt as _;

        let applied = LoginFiles::planned(Autostart::TurnBackOn).await.unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let umask = status
            .lines()
            .find_map(|line| line.strip_prefix("Umask:"))
            .map(|umask| u32::from_str_radix(umask.trim(), 8).unwrap())
            .unwrap();
        let mode = std::fs::metadata(home.path(".profile")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o666 & !umask);
    }

    /// Removal keeps the user's blank line at the end of a file whose block is elsewhere.
    #[rstest]
    #[case::block_first(format!("{BLOCK}export A=1\n\n"), "export A=1\n\n")]
    #[case::block_last(format!("export A=1\n\n{BLOCK}"), "export A=1\n")]
    fn removal_keeps_the_users_blank_lines(
        home: Home,
        #[case] profile: String,
        #[case] left: &str,
    ) {
        home.write(".profile", profile);

        let applied = block_on(LoginFiles::removal()).unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert_eq!(home.read(".profile").as_deref(), Some(left));
    }

    /// Removal takes the block out of the `.zprofile` in `ZDOTDIR`, and out of the `.zprofile`
    /// in the home that setup writes when Zsh doesn't say its `ZDOTDIR` in time.
    #[rstest]
    fn removal_looks_in_the_home_and_in_zdotdir(home: Home) {
        home.write(".zshenv", "ZDOTDIR=\"$HOME/.config/zsh\"\n");
        home.write(".zprofile", format!("export A=1\n\n{BLOCK}"));
        home.write(".config/zsh/.zprofile", format!("export B=1\n\n{BLOCK}"));

        let applied = block_on(LoginFiles::removal()).unwrap().apply();

        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert_eq!(home.read(".zprofile").as_deref(), Some("export A=1\n"));
        if std::process::Command::new("zsh").arg("-c").arg("true").status().is_ok() {
            assert_eq!(home.read(".config/zsh/.zprofile").as_deref(), Some("export B=1\n"));
        }
    }

    /// The refresh rewrites setup's environment file when its content differs from what this
    /// build writes, and leaves a missing file, a current file, and a file of someone else as
    /// they are.
    #[rstest]
    #[case::differs(
        Some("# Written by atuin setup. Some other content.\nACCESSIBILITY_ENABLED=1\n"),
        Some(ENVIRONMENT_CONTENT)
    )]
    #[case::current(Some(ENVIRONMENT_CONTENT), Some(ENVIRONMENT_CONTENT))]
    #[case::someone_elses(Some("ACCESSIBILITY_ENABLED=1\n"), Some("ACCESSIBILITY_ENABLED=1\n"))]
    #[case::missing(None, None)]
    fn refreshes_the_environment_file_when_it_differs(
        home: Home,
        #[case] before: Option<&str>,
        #[case] after: Option<&str>,
    ) {
        let path = ".config/environment.d/60-atuin-desktop-capture.conf";
        if let Some(before) = before {
            home.write(path, before);
        }

        refresh_environment_file().unwrap();

        assert_eq!(home.read(path).as_deref(), after);
    }

    #[rstest]
    #[case::sh(LoginShell::Sh, "export ACCESSIBILITY_ENABLED=1\nexport NO_AT_BRIDGE=0\n")]
    #[case::fish(LoginShell::Fish, "set -gx ACCESSIBILITY_ENABLED 1\nset -gx NO_AT_BRIDGE 0\n")]
    #[case::nu(LoginShell::Nu, "{\"ACCESSIBILITY_ENABLED\":\"1\",\"NO_AT_BRIDGE\":\"0\"}\n")]
    #[cfg_attr(not(target_os = "linux"), ignore = "only the Linux backend names session variables")]
    fn variables_in_each_syntax(#[case] shell: LoginShell, #[case] expected: &str) {
        let mut out = Vec::new();
        write_variables(&mut out, shell).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), expected);
    }

    /// The directory follows the last marker, so text that `.zshenv` prints before it doesn't
    /// count, and a directory that holds a newline or isn't absolute doesn't count either.
    #[rstest]
    #[case::alone(b"\natuin-zdotdir=/home/u/.config/zsh", Some("/home/u/.config/zsh"))]
    #[case::after_output(
        b"/some/output\nmore\natuin-zdotdir=/home/u/.config/zsh",
        Some("/home/u/.config/zsh")
    )]
    #[case::without_a_marker(b"/home/u/.config/zsh", None)]
    #[case::relative(b"\natuin-zdotdir=zsh", None)]
    #[case::with_a_newline(b"\natuin-zdotdir=/home/u\n/x", None)]
    fn the_printed_zdotdir_follows_the_marker(
        #[case] stdout: &[u8],
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(printed_zdotdir(stdout), expected.map(PathBuf::from));
    }

    /// Zsh says the `ZDOTDIR` that `.zshenv` sets, even when `.zshenv` prints text of its own.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn zsh_names_the_zdotdir_of_a_talkative_zshenv() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".zshenv"),
            "echo /from/zshenv\nexport ZDOTDIR=\"$HOME/.config/zsh\"\n",
        )
        .unwrap();
        if std::process::Command::new("zsh").arg("-c").arg("true").output().is_err() {
            return;
        }

        let dir = zdotdir(home.path()).await;

        assert_eq!(dir, Some(home.path().join(".config/zsh")));
    }

    /// Every login shell that setup knows has a file that removal looks at.
    #[rstest]
    #[case::dash("dash", Some(ShellKind::Posix))]
    #[case::bash("bash", Some(ShellKind::Bash))]
    #[case::zsh("zsh", Some(ShellKind::Zsh))]
    #[case::fish("fish", Some(ShellKind::Fish))]
    #[case::nu("nu", Some(ShellKind::Nu))]
    #[case::unknown("tcsh", None)]
    fn shells_have_kinds(#[case] name: &str, #[case] expected: Option<ShellKind>) {
        assert_eq!(ShellKind::named(name), expected);
        if let Some(kind) = expected {
            assert!(ShellKind::ALL.contains(&kind));
        }
    }
}
