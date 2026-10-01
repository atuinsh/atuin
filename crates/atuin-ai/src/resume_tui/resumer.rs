//! The picker's resume seam: how a session turns into a command, or why it can't here.
//!
//! Plans come from the harness tools ([`atuin_common::harnesstools::resume`]); this layer adds
//! what only the picker knows: whether the session's transcript is on this machine, where to
//! resume one that isn't, and the user's `[ai.sessions.resume]` templates. Planning may walk the
//! harness's session directories, so the picker asks only for the selected session, never per
//! row.
//!
//! A session whose native transcript isn't here (recorded on another host, or deleted) is
//! resumed from the synced messages: the plan says so ([`Resume::restore`]), and only once the
//! user accepts it does [`Resumer::restore`] write the transcript back out
//! ([`Harness::rehydrate`](atuin_common::harnesstools::Harness::rehydrate)) and plan resuming it.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use atuin_client::settings::AiSessionResume;
use atuin_common::harnesstools::rehydrate::{RehydrateError, RehydrateSession};
pub use atuin_common::harnesstools::resume::{ResumeError, ResumePlan, ResumeTarget, quote};
use atuin_common::harnesstools::{AnyHarness, Harness as _};

use super::ResumeContext;
use super::source::{SessionRow, SessionSource, harness_label};

/// Why a session can be viewed but not resumed here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotResumable {
    #[error("atuin can't resume {0} sessions")]
    Unsupported(&'static str),

    #[error("`{0}` isn't installed here (not found on PATH)")]
    NotInstalled(String),

    #[error("restoring it from sync failed: {0}")]
    Restore(String),

    #[error(transparent)]
    Harness(#[from] ResumeError),
}

/// How to resume a session here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
    pub plan: ResumePlan,
    /// Set when the session's transcript isn't on this machine: it is written from the synced
    /// messages first ([`Resumer::restore`]), and `plan` only shows what will run.
    pub restore: Option<Restore>,
}

impl Resume {
    /// A session ready to resume with `plan`.
    pub fn ready(plan: ResumePlan) -> Self {
        Self {
            plan,
            restore: None,
        }
    }
}

/// Where a session restored from sync resumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restore {
    pub cwd: PathBuf,
    /// Why that isn't the directory the session ran in, when it isn't.
    pub note: Option<String>,
}

/// Plans resuming a session.
#[async_trait]
pub trait Resumer: Send + Sync {
    /// How to resume `session` here, checked against the filesystem. Never writes anything.
    async fn plan(&self, session: &SessionRow) -> Result<Resume, NotResumable>;

    /// Write out the transcript of `session`, planned with `restore`, from what `source` holds of
    /// it, and plan resuming it. Only once the user has chosen to resume it.
    async fn restore(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        restore: &Restore,
    ) -> Result<ResumePlan, NotResumable>;
}

/// The shell line for a plan: `cd -- <cwd> && <command>`.
pub fn shell_line(plan: &ResumePlan) -> String {
    plan.render().unwrap_or_else(|_| plan.command())
}

/// What a [`HarnessResumer`] reads and writes on this machine: the harness's transcripts and
/// the programs on `PATH`. Tests swap it for one that touches nothing.
#[async_trait]
pub trait Machine: Send + Sync {
    /// Where `harness` keeps session `id` here ([`Harness::locate`]).
    ///
    /// [`Harness::locate`]: atuin_common::harnesstools::Harness::locate
    async fn locate(&self, harness: AnyHarness, id: &str) -> Option<PathBuf>;

    /// Write `session` out for `harness` ([`Harness::rehydrate`]).
    ///
    /// [`Harness::rehydrate`]: atuin_common::harnesstools::Harness::rehydrate
    async fn rehydrate(
        &self,
        harness: AnyHarness,
        session: &RehydrateSession,
    ) -> Result<PathBuf, RehydrateError>;

    /// Whether `program` can be run here.
    fn installed(&self, program: &str) -> bool;
}

/// This machine, as the harnesses themselves see it.
pub struct ThisMachine;

#[async_trait]
impl Machine for ThisMachine {
    async fn locate(&self, harness: AnyHarness, id: &str) -> Option<PathBuf> {
        harness.locate(id).await
    }

    async fn rehydrate(
        &self,
        harness: AnyHarness,
        session: &RehydrateSession,
    ) -> Result<PathBuf, RehydrateError> {
        harness.rehydrate(session).await
    }

    fn installed(&self, program: &str) -> bool {
        on_path(program)
    }
}

/// The real [`Resumer`]: the harness's own resume command (or the user's template), restoring
/// the session's transcript from sync when it isn't on this machine.
pub struct HarnessResumer {
    host_id: String,
    templates: AiSessionResume,
    context: ResumeContext,
    machine: Box<dyn Machine>,
}

impl HarnessResumer {
    pub fn new(context: ResumeContext, templates: AiSessionResume) -> Self {
        Self::on(context, templates, ThisMachine)
    }

    /// A resumer looking at `machine` instead of this one.
    pub fn on(
        context: ResumeContext,
        templates: AiSessionResume,
        machine: impl Machine + 'static,
    ) -> Self {
        Self {
            host_id: context.host_id.clone(),
            templates,
            context,
            machine: Box::new(machine),
        }
    }

    fn check_installed(&self, plan: &ResumePlan) -> Result<(), NotResumable> {
        if self.machine.installed(&program_to_check(plan)) {
            Ok(())
        } else {
            Err(NotResumable::NotInstalled(plan.program.clone()))
        }
    }

    /// Write `data`, session `session` read from sync, out as its harness's transcript, planned
    /// with `restore`, and plan resuming it.
    async fn write_out(
        &self,
        session: &SessionRow,
        data: &RehydrateSession,
        restore: &Restore,
    ) -> Result<ResumePlan, NotResumable> {
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let native = match self.machine.rehydrate(harness, data).await {
            Ok(path) | Err(RehydrateError::AlreadyExists(path)) => path,
            Err(e) => return Err(NotResumable::Restore(e.to_string())),
        };
        let target = ResumeTarget::new(session.handle.session.as_ref())
            .with_cwd(&restore.cwd)
            .with_native_path(native);
        let plan = harness.resume(&target, self.templates.template(kind))?.prepare()?;
        self.check_installed(&plan)?;
        Ok(plan)
    }
}

#[async_trait]
impl Resumer for HarnessResumer {
    async fn plan(&self, session: &SessionRow) -> Result<Resume, NotResumable> {
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let id = session.handle.session.as_ref();
        let template = self.templates.template(kind);

        let mut target = ResumeTarget::new(id);
        if let Some(cwd) = &session.cwd {
            target = target.with_cwd(cwd);
        }
        // The harness decides first (a subagent is never resumable, whatever is on disk).
        harness.resume_plan(&target)?;

        let local = session.host_id == self.host_id;
        let resume = match self.machine.locate(harness, id).await {
            Some(native) => {
                // Another host's session restored here before: resume where it was restored to.
                if !local && !session.cwd.as_deref().is_some_and(Path::is_dir) {
                    target.cwd = Some(resolve_cwd(session.cwd.as_deref(), &self.context).cwd);
                }
                let target = target.with_native_path(native);
                Resume::ready(harness.resume(&target, template)?.prepare()?)
            }
            None => {
                let restore = resolve_cwd(session.cwd.as_deref(), &self.context);
                target.cwd = Some(restore.cwd.clone());
                Resume {
                    plan: harness.resume(&target, template)?.prepare()?,
                    restore: Some(restore),
                }
            }
        };
        self.check_installed(&resume.plan)?;
        Ok(resume)
    }

    async fn restore(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        restore: &Restore,
    ) -> Result<ResumePlan, NotResumable> {
        let data = source
            .rehydrate(&session.handle, &restore.cwd)
            .await
            .map_err(|e| NotResumable::Restore(format!("{e:#}")))?;
        self.write_out(session, &data, restore).await
    }
}

/// Where to resume a session that ran in `original` on this machine: there, when it exists;
/// else, when the current directory is in a git repository named like one `original` was in,
/// the same place in that repository (or its root); else the current directory.
pub fn resolve_cwd(original: Option<&Path>, context: &ResumeContext) -> Restore {
    let Some(original) = original else {
        return Restore {
            cwd: context.cwd.clone(),
            note: Some("its directory isn't known; resuming in the current directory".to_owned()),
        };
    };
    if original.is_dir() {
        return Restore {
            cwd: original.to_owned(),
            note: None,
        };
    }
    let checkout = context.git_root.as_deref().and_then(|root| {
        let name = root.file_name()?;
        let components: Vec<_> = original.components().collect();
        let at = components.iter().rposition(|c| c.as_os_str() == name)?;
        // Rebuilt from components, so no trailing separator comes along.
        let root: PathBuf = root.components().collect();
        let same_place: PathBuf =
            root.components().chain(components[at + 1..].iter().copied()).collect();
        Some(if same_place.is_dir() {
            same_place
        } else {
            root
        })
    });
    match checkout {
        Some(cwd) => Restore {
            note: Some(format!(
                "{} isn't on this machine; resuming in {}",
                original.display(),
                cwd.display()
            )),
            cwd,
        },
        None => Restore {
            cwd: context.cwd.clone(),
            note: Some(format!(
                "{} isn't on this machine; resuming in the current directory",
                original.display()
            )),
        },
    }
}

/// Whether `program` can be run: a path to a file, or a file of that name in a `PATH` directory
/// (executable, on unix; on Windows, also with any of the `PATHEXT` extensions, as `cmd` finds
/// `cmd.exe`).
pub fn on_path(program: &str) -> bool {
    if program.contains(std::path::MAIN_SEPARATOR) || program.contains('/') {
        return runnable(Path::new(program));
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| runnable(&dir.join(program)))
}

/// The program a plan runs, as [`on_path`] should check it: a relative path such as
/// `./bin/wrapper` names a file under the directory the command runs from, not the one the
/// picker was opened in.
pub fn program_to_check(plan: &ResumePlan) -> String {
    let program = Path::new(&plan.program);
    let has_dir = plan.program.contains(std::path::MAIN_SEPARATOR) || plan.program.contains('/');
    match &plan.cwd {
        Some(cwd) if has_dir && program.is_relative() => {
            cwd.join(program).to_string_lossy().into_owned()
        }
        _ => plan.program.clone(),
    }
}

/// Whether the file at `path` can be run.
fn runnable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(windows)]
    {
        if path.is_file() {
            return true;
        }
        let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        extensions.split(';').filter(|ext| !ext.is_empty()).any(|ext| {
            let mut with_ext = path.as_os_str().to_owned();
            with_ext.push(ext);
            Path::new(&with_ext).is_file()
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests;
