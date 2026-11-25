//! What `atuin setup` and `atuin desktop-capture apps` do with the helper.

use std::ffi::OsStr;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};

use atuin_client::settings::Settings;
use atuin_common::error::WithSources;
#[cfg(feature = "ai")]
use atuin_common::harnesstools::{InstallHookError, ccode};
use atuin_daemon::{HELPER_NAME, Signing, SigningError, helper_path};
use atuin_desktop_capture::link::process::{
    APPS_RECORD_FD_VARIABLE, AppsRecord, COMMAND_VARIABLE, CaptureExit,
};
use eyre::{Result, WrapErr as _, bail};

use super::hiding;
use super::session::{self, Autostart, LoginFiles, MARKER, SessionError, Variable};
use super::toolkit::{self, Restored, Toolkit};
use crate::error::Refusal;
use crate::i18n::fl;

/// What [`Logins::start`] or [`Logins::stop`] did, in phrases for the list of `atuin setup`.
#[derive(Debug, Default)]
pub struct LoginReport {
    pub done: Vec<String>,
    /// What failed.
    pub failures: Vec<String>,
    /// What keeps capture from working in some sessions.
    pub notes: Vec<String>,
}

/// Whether [`LoginReport::add_to`] passes on the report's notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notes {
    Always,
    /// Only when the report did something, so that a run that changes nothing stays quiet.
    WhenDone,
}

impl LoginReport {
    /// Adds the report's phrases to the lists of `atuin setup`. The failures go to `failed`, and
    /// the notes also go there when `notes` allows.
    pub fn add_to(self, done: &mut Vec<String>, failed: &mut Vec<String>, notes: Notes) {
        failed.extend(self.failures);
        if notes == Notes::Always || !self.done.is_empty() {
            failed.extend(self.notes);
        }
        done.extend(self.done);
    }

    /// Applies `files`, which `change` planned, and adds what happened. Returns the files, when
    /// the plan succeeded.
    fn add_logins(
        &mut self,
        change: LoginChange,
        files: Result<LoginFiles, SessionError>,
    ) -> Option<LoginFiles> {
        let files = match files {
            Ok(files) => files,
            Err(err) => {
                self.failures.push(change.failed(&err));
                return None;
            }
        };
        let applied = files.apply();
        let failures = files.unreadable().iter().chain(&applied.failures);
        self.failures.extend(failures.map(|err| change.failed(err)));
        if applied.changed > 0 {
            self.done.push(change.done(applied.changed));
        }
        Some(files)
    }
}

/// What `atuin setup` does to the files that start capture at login.
#[derive(Debug, Clone, Copy)]
enum LoginChange {
    Start,
    Stop,
}

impl LoginChange {
    /// The phrase that says that the change went through for `count` files.
    fn done(self, count: usize) -> String {
        match self {
            Self::Start => fl!("desktop-capture-logins-started", count = count),
            Self::Stop => fl!("desktop-capture-logins-stopped", count = count),
        }
    }

    /// The phrase that says that the change failed with `err`.
    fn failed(self, err: &SessionError) -> String {
        let error = WithSources(err).to_string();
        match self {
            Self::Start => fl!("desktop-capture-logins-start-failed", error = error),
            Self::Stop => fl!("desktop-capture-logins-stop-failed", error = error),
        }
    }
}

/// The files and settings that make each session start capture at login, on a platform whose
/// sessions need them.
///
/// The methods take `self` because only [`Self::for_platform`] makes a `Logins`, so a caller
/// can reach them only on such a platform.
#[derive(Debug)]
pub struct Logins {
    _platform: (),
}

#[expect(clippy::unused_self, reason = "a `Logins` shows that the platform needs the files")]
impl Logins {
    /// The logins of this platform. `None` on a platform whose sessions start capture without
    /// the files of setup.
    pub fn for_platform() -> Option<Self> {
        cfg!(target_os = "linux").then_some(Self { _platform: () })
    }

    /// Writes the files that make each session start capture at login, and turns the
    /// `toolkit-accessibility` setting on, without asking.
    ///
    /// `autostart` says what happens to an autostart entry that the desktop turned off.
    ///
    /// The report says that setup's files override a hiding variable only when the files
    /// changed in this run, so that a run that changes nothing says nothing.
    pub async fn start(&self, autostart: Autostart) -> LoginReport {
        start_at_logins(autostart).await
    }

    /// Removes what setup wrote to start capture at login, and puts back the value of the
    /// `toolkit-accessibility` setting from before setup changed it.
    pub async fn stop(&self) -> LoginReport {
        stop_at_logins().await
    }

    /// Brings the file that sets the variables of graphical sessions up to date at the start of
    /// a session.
    ///
    /// # Errors
    ///
    /// Fails when the file can't be read or written.
    pub fn refresh(&self) -> Result<(), SessionError> {
        session::refresh_environment_file()
    }

    /// The variables that capture needs in each session.
    pub fn session_variables(&self) -> impl Iterator<Item = Variable> {
        session::variables()
    }
}

async fn start_at_logins(autostart: Autostart) -> LoginReport {
    let mut report = LoginReport::default();
    let files = report.add_logins(LoginChange::Start, LoginFiles::planned(autostart).await);
    let files_changed = !report.done.is_empty();
    let found: Vec<_> = hiding::find().collect();
    let setup_files = if session::writes_environment_file() {
        hiding::SetupFiles::WithEnvironmentFile
    } else {
        hiding::SetupFiles::WithoutEnvironmentFile
    };
    for hiding in &found {
        if !hiding.is_overridden(&found, setup_files) {
            report.notes.push(hiding.describe());
        } else if files_changed {
            report.done.push(hiding.describe_overridden());
        }
    }
    if let Some(files) = files {
        if let Some(line) = files.unknown_shell {
            report.notes.push(fl!(
                "desktop-capture-logins-unknown-shell",
                marker = MARKER,
                line = line
            ));
        }
        if let Some(path) = files.guessed_zprofile {
            let path = path.display().to_string();
            report.notes.push(fl!("desktop-capture-logins-zdotdir-unknown", path = path));
        }
        for path in files.blocks_after_exec {
            let path = path.display().to_string();
            report.notes.push(fl!(
                "desktop-capture-logins-block-after-exec",
                path = path,
                marker = MARKER
            ));
        }
        if let Some(config_home) = files.config_home {
            let paths = std::fmt::from_fn(|f| {
                for (index, path) in config_home.files.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", path.display())?;
                }
                Ok(())
            });
            report.notes.push(fl!(
                "desktop-capture-logins-config-home",
                path = config_home.dir.display().to_string(),
                files = paths.to_string()
            ));
        }
    }
    match toolkit::read().await {
        Toolkit::Off => match toolkit::turn_on().await {
            Ok(()) => report.done.push(fl!("desktop-capture-toolkit-turned-on")),
            Err(err) => report.failures.push(WithSources(&err).to_string()),
        },
        Toolkit::Unsaved => report.notes.push(fl!("desktop-capture-toolkit-left-alone")),
        Toolkit::Absent | Toolkit::On => {}
    }
    report
}

async fn stop_at_logins() -> LoginReport {
    let mut report = LoginReport::default();
    report.add_logins(LoginChange::Stop, LoginFiles::removal().await);
    match toolkit::restore().await {
        Ok(Restored::Off) => report.done.push(fl!("desktop-capture-toolkit-turned-off")),
        Ok(Restored::Nothing) => {}
        Err(err) => report.failures.push(WithSources(&err).to_string()),
    }
    report
}

/// Whether [`ensure_certificate`] made the certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateOutcome {
    Present,
    Created,
}

/// Creates the certificate that signs the helper, unless it exists.
///
/// # Errors
///
/// Fails when the certificate cannot be created. The helper then stays unsigned, which works
/// until its next update.
pub async fn ensure_certificate(signing: &Signing) -> Result<CertificateOutcome, SigningError> {
    if signing.has_certificate().await {
        return Ok(CertificateOutcome::Present);
    }
    signing.create_certificate().await?;
    Ok(CertificateOutcome::Created)
}

/// What [`ensure_prompt_hook`] found or did.
#[cfg(feature = "ai")]
#[derive(Debug)]
pub enum PromptHook {
    /// The settings file now holds the hook.
    Added,
    /// The hook was there already.
    Present,
    /// Atuin's Claude Code hooks are not installed, so the prompt hook stays out as well.
    NoHooks,
}

/// Registers Claude Code's prompt hook, which adds recent desktop activity to each prompt, when
/// Atuin's Claude Code hooks are installed.
#[cfg(feature = "ai")]
pub async fn ensure_prompt_hook() -> Result<PromptHook, InstallHookError> {
    if !ccode::is_installed().await {
        return Ok(PromptHook::NoHooks);
    }
    match ccode::install_prompt_hook().await {
        Ok(_) => Ok(PromptHook::Added),
        Err(InstallHookError::AlreadyInstalled) => Ok(PromptHook::Present),
        Err(err) => Err(err),
    }
}

/// What [`remove_prompt_hook`] found.
#[cfg(feature = "ai")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptHookRemoval {
    Removed,
    Absent,
}

/// Takes Claude Code's prompt hook out, and keeps Atuin's other Claude Code hooks.
#[cfg(feature = "ai")]
pub async fn remove_prompt_hook() -> Result<PromptHookRemoval, InstallHookError> {
    Ok(if ccode::uninstall_prompt_hook().await? {
        PromptHookRemoval::Removed
    } else {
        PromptHookRemoval::Absent
    })
}

/// Finds the helper to run.
///
/// With `signing`, this is the signed copy of the helper once a certificate exists, because the
/// Accessibility permission belongs to that copy.
async fn helper_program(settings: &Settings, signing: Option<&Signing>) -> Result<PathBuf> {
    let path = helper_path(settings);
    let Some(signing) = signing else {
        return Ok(path);
    };
    if !path.exists() {
        return Err(helper_missing(&path));
    }
    signing.helper(path).await.wrap_err(fl!("desktop-capture-signing-copy-failed"))
}

fn helper_missing(path: &Path) -> eyre::Report {
    let path = path.display().to_string();
    Refusal(fl!("desktop-capture-helper-missing", path = path)).into()
}

/// The error for a helper at `path` that did not start.
fn spawn_failed(path: &Path, err: io::Error) -> eyre::Report {
    if err.kind() == io::ErrorKind::NotFound {
        return helper_missing(path);
    }
    let path = path.display().to_string();
    eyre::Report::new(err).wrap_err(fl!("desktop-capture-helper-failed", path = path))
}

/// The `atuin` command that runs the helper's `apps` and `doctor`. The helper's hints name this
/// command.
const USER_COMMAND: &str = "atuin desktop-capture";

/// Runs the helper with `args` and the terminal of this process, and exits as the helper exits
/// when the helper fails.
///
/// The helper's hints name the commands of `atuin`.
pub async fn run_helper<I>(settings: &Settings, args: I) -> Result<()>
where
    I: IntoIterator,
    I::Item: AsRef<OsStr>,
{
    let status = HelperCommand::new(settings, args).await?.status()?;
    exit_unless_succeeded(status);
    Ok(())
}

/// Runs the helper's `apps` with the terminal of this process, and returns the applications that
/// the helper suggests to capture. Exits as the helper exits when the helper fails.
///
/// Returns nothing when the helper found nothing to suggest.
pub async fn find_apps(settings: &Settings) -> Result<Option<AppsRecord>> {
    let mut helper = HelperCommand::new(settings, ["apps"]).await?;
    let (mut reader, writer) = io::pipe().wrap_err(fl!("desktop-capture-apps-pipe-failed"))?;
    pass_writer(&mut helper.command, &writer)?;
    let mut child = helper.spawn()?;
    // The read ends once every copy of the writer is closed, so this process closes its own
    // copy before the read.
    drop(writer);
    let mut record = String::new();
    let read = reader.read_to_string(&mut record);
    let status = child.wait().wrap_err(fl!("desktop-capture-apps-wait-failed"))?;
    exit_unless_succeeded(status);
    read.wrap_err(fl!("desktop-capture-apps-read-failed"))?;
    if record.is_empty() {
        return Ok(None);
    }
    let record = serde_json::from_str(&record).wrap_err(fl!("desktop-capture-apps-read-failed"))?;
    Ok(Some(record))
}

/// Lets the helper that `command` starts inherit `writer`, and names the writer's file
/// descriptor in [`APPS_RECORD_FD_VARIABLE`].
///
/// The writer stays open in children that this process starts before it closes the writer, so
/// the caller starts only the helper in between.
#[cfg(unix)]
fn pass_writer(command: &mut Command, writer: &io::PipeWriter) -> Result<()> {
    use std::os::fd::AsRawFd as _;

    rustix::io::fcntl_setfd(writer, rustix::io::FdFlags::empty())
        .wrap_err(fl!("desktop-capture-apps-pipe-failed"))?;
    command.env(APPS_RECORD_FD_VARIABLE, writer.as_raw_fd().to_string());
    Ok(())
}

/// Leaves the helper without a writer, so the helper prints the setting for the user to copy.
// TODO: pass the pipe's handle on Windows once the helper captures there.
#[cfg(not(unix))]
fn pass_writer(_: &mut Command, _: &io::PipeWriter) -> Result<()> {
    Ok(())
}

/// A command that runs the helper with the terminal of this process.
struct HelperCommand {
    /// The helper, which the errors of a failed start name.
    path: PathBuf,
    command: Command,
}

impl HelperCommand {
    /// The command that runs the helper with `args`, whose hints name the commands of `atuin`.
    async fn new<I>(settings: &Settings, args: I) -> Result<Self>
    where
        I: IntoIterator,
        I::Item: AsRef<OsStr>,
    {
        let path = helper_program(settings, Signing::for_platform().as_ref()).await?;
        let mut command = Command::new(&path);
        command.args(args).env(COMMAND_VARIABLE, USER_COMMAND);
        Ok(Self { path, command })
    }

    /// Runs the helper, and waits until it exits.
    fn status(mut self) -> Result<ExitStatus> {
        self.command.status().map_err(|err| spawn_failed(&self.path, err))
    }

    fn spawn(mut self) -> Result<Child> {
        self.command.spawn().map_err(|err| spawn_failed(&self.path, err))
    }
}

/// Exits as the helper exited, when the helper failed. The helper told the user why already.
fn exit_unless_succeeded(status: ExitStatus) {
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// Whether the helper has the Accessibility permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperAccess {
    Granted,
    Denied,
}

impl HelperAccess {
    /// The access that the exit code of the helper's `access check` stands for, if any.
    fn from_exit_code(code: Option<i32>) -> Option<Self> {
        match code? {
            0 => Some(Self::Granted),
            code => match CaptureExit::from_code(code)? {
                CaptureExit::NeedsPermission => Some(Self::Denied),
                CaptureExit::NoDesktop => None,
            },
        }
    }
}

/// Checks whether the helper has the Accessibility permission, without asking for it.
///
/// With `signing`, the check runs the signed copy of the helper, and first signs the copy again
/// when the copy is out of date.
///
/// # Errors
///
/// Fails when the helper does not start, and when the helper fails in a way that says nothing
/// about the permission.
pub async fn helper_access(settings: &Settings, signing: Option<&Signing>) -> Result<HelperAccess> {
    helper_access_at(&helper_program(settings, signing).await?).await
}

/// The helper that capture runs now: with `signing`, the signed copy once a certificate signed
/// one, and otherwise the shipped helper.
pub async fn installed_helper(settings: &Settings, signing: Option<&Signing>) -> PathBuf {
    if let Some(signing) = signing
        && signing.has_certificate().await
        && let Ok(copy) = signing.signed_helper().await
        && tokio::fs::try_exists(&copy).await.unwrap_or(false)
    {
        return copy;
    }
    helper_path(settings)
}

/// The name under which System Settings lists the helper that asks for the Accessibility
/// permission.
pub async fn helper_name(signing: Option<&Signing>) -> String {
    let named = match signing {
        Some(signing) => signing.helper_name().await.ok(),
        None => None,
    };
    named.unwrap_or_else(|| HELPER_NAME.to_owned())
}

/// Checks whether the helper at `path` has the Accessibility permission, without asking for it.
///
/// # Errors
///
/// Fails as [`helper_access`] fails.
pub async fn helper_access_at(path: &Path) -> Result<HelperAccess> {
    let status = tokio::process::Command::new(path)
        .args(["access", "check"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|err| spawn_failed(path, err))?;
    HelperAccess::from_exit_code(status.code()).ok_or_else(|| {
        eyre::eyre!(fl!("desktop-capture-access-check-failed", status = status.to_string()))
    })
}

/// A helper that asks the user for the Accessibility permission.
///
/// Dropping the request stops the helper. When `atuin` is killed and the request is never dropped,
/// the helper stops once this process exits.
#[derive(Debug)]
pub struct AccessRequest {
    child: tokio::process::Child,
    /// The other end of the helper's stdin. The helper exits once every copy of this end closes.
    /// The system closes this process's copy when this process exits.
    _stdin: tokio::process::ChildStdin,
}

impl AccessRequest {
    /// Waits until the user grants the permission.
    ///
    /// # Errors
    ///
    /// Fails when the helper ends without the grant.
    pub async fn granted(&mut self) -> Result<()> {
        let status =
            self.child.wait().await.wrap_err(fl!("desktop-capture-access-request-wait"))?;
        ensure_exited_cleanly(status)
    }
}

fn ensure_exited_cleanly(status: ExitStatus) -> Result<()> {
    if !status.success() {
        bail!(fl!("desktop-capture-access-request-failed", status = status.to_string()));
    }
    Ok(())
}

/// Starts the helper's request for the Accessibility permission. `signing` prepares the helper
/// as [`helper_access`] says.
///
/// # Errors
///
/// Fails when the helper does not start.
pub async fn request_access(
    settings: &Settings,
    signing: Option<&Signing>,
) -> Result<AccessRequest> {
    let path = helper_program(settings, signing).await?;
    let mut command = tokio::process::Command::new(&path);
    command.args(["access", "request", "--until-stdin-closes"]);
    AccessRequest::spawn(command).map_err(|err| spawn_failed(&path, err))
}

impl AccessRequest {
    fn spawn(mut command: tokio::process::Command) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        // `Child::wait` closes the stdin that the child holds, so the request keeps stdin apart.
        let stdin =
            child.stdin.take().ok_or_else(|| io::Error::other("the helper has no stdin"))?;
        Ok(Self {
            child,
            _stdin: stdin,
        })
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A run of setup that keeps capture on writes the login files into a home that lacks them, and
    /// a second run doesn't report anything.
    ///
    /// For example, the home's config may have come from another machine.
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn keeping_capture_on_writes_the_login_files_once() {
        let home = tempfile::tempdir().unwrap();
        let env = atuin_common::env::MockEnv::install();
        env.set("HOME", home.path());
        // The memory backend keeps the test away from the user's own toolkit-accessibility.
        env.set("GSETTINGS_BACKEND", "memory");
        std::fs::write(home.path().join(".profile"), "export EDITOR=vi\n").unwrap();

        let first = start_at_logins(Autostart::KeepDesktopsChoice).await;
        let second = start_at_logins(Autostart::KeepDesktopsChoice).await;

        assert!(first.failures.is_empty(), "{:?}", first.failures);
        assert!(!first.done.is_empty(), "{:?}", first.done);
        let profile = std::fs::read_to_string(home.path().join(".profile")).unwrap();
        assert!(profile.contains(MARKER), "{profile}");
        let entry = home.path().join(".config/autostart/atuin-desktop-capture.desktop");
        assert!(entry.exists());
        let (mut done, mut failed) = (Vec::new(), Vec::new());
        second.add_to(&mut done, &mut failed, Notes::WhenDone);
        assert_eq!((done, failed), (Vec::<String>::new(), Vec::<String>::new()));
    }

    #[rstest]
    #[case::granted(Some(0), Some(HelperAccess::Granted))]
    #[case::denied(Some(3), Some(HelperAccess::Denied))]
    #[case::failed(Some(1), None)]
    #[case::killed(None, None)]
    fn the_exit_code_says_whether_the_helper_has_access(
        #[case] code: Option<i32>,
        #[case] expected: Option<HelperAccess>,
    ) {
        assert_eq!(HelperAccess::from_exit_code(code), expected);
    }

    /// Whether the process `pid` is still running. A zombie has stopped, and is waiting only to be
    /// reaped.
    #[cfg(target_os = "linux")]
    fn runs(pid: u32) -> bool {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        // The state follows the parenthesized name of the program.
        stat.rsplit_once(") ").is_some_and(|(_, rest)| !rest.starts_with('Z'))
    }

    /// Leaving the permission page drops the request, which must stop the helper.
    #[cfg(target_os = "linux")]
    #[rstest]
    #[tokio::test]
    async fn dropping_a_request_stops_the_helper() {
        let mut command = tokio::process::Command::new("sleep");
        command.arg("60");
        let request = AccessRequest::spawn(command).unwrap();
        let pid = request.child.id().unwrap();
        assert!(runs(pid));

        drop(request);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while runs(pid) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!runs(pid), "the helper still runs");
    }

    /// The helper's stdin stays open while setup waits for the grant, because a helper that
    /// watches stdin would otherwise stop waiting at once.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn stdin_stays_open_while_setup_waits() {
        // `cat` exits once its stdin closes.
        let mut request = AccessRequest::spawn(tokio::process::Command::new("cat")).unwrap();

        let waited =
            tokio::time::timeout(std::time::Duration::from_millis(300), request.granted()).await;

        assert!(waited.is_err(), "the helper exited: {waited:?}");
    }

    fn report() -> LoginReport {
        LoginReport {
            done: Vec::new(),
            failures: vec!["couldn't update the login files".to_owned()],
            notes: vec!["this session sets NO_AT_BRIDGE=1".to_owned()],
        }
    }

    /// A failure always reaches the list, even when capture stays as it was. A note reaches the
    /// list only when the run changes something, or when `notes` is `Always`.
    #[rstest]
    #[case::stays(Notes::WhenDone, false, &["couldn't update the login files"])]
    #[case::stays_and_changes(
        Notes::WhenDone,
        true,
        &["couldn't update the login files", "this session sets NO_AT_BRIDGE=1"]
    )]
    #[case::flips(
        Notes::Always,
        false,
        &["couldn't update the login files", "this session sets NO_AT_BRIDGE=1"]
    )]
    fn failures_always_reach_the_list(
        #[case] notes: Notes,
        #[case] did_something: bool,
        #[case] expected: &[&str],
    ) {
        let mut report = report();
        if did_something {
            report.done.push("brought 1 file up to date".to_owned());
        }
        let mut done = Vec::new();
        let mut failed = Vec::new();

        report.add_to(&mut done, &mut failed, notes);

        assert_eq!(failed, expected);
        assert_eq!(done.len(), usize::from(did_something));
    }
}
