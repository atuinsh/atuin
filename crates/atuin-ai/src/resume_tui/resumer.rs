//! The picker's resume seam: how a session turns into a command, or why it can't here.
//!
//! Plans come from the harness tools ([`atuin_common::harnesstools::resume`]); this layer adds
//! what only the picker knows: whether the session's transcript is on this machine, and the
//! user's `[ai.sessions.resume]` templates. Planning may walk the harness's session
//! directories, so the picker asks only for the selected session, never per row.
//!
//! A session whose native transcript isn't here (recorded on another host, or deleted) can be
//! viewed but not resumed.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use atuin_client::settings::AiSessionResume;
pub use atuin_common::harnesstools::resume::{ResumeError, ResumePlan, ResumeTarget};
use atuin_common::harnesstools::{AnyHarness, Harness as _};

use super::source::{SessionRow, harness_label};

/// Why a session can be viewed but not resumed here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotResumable {
    #[error("atuin can't resume {0} sessions")]
    Unsupported(&'static str),

    #[error("`{0}` isn't installed here (not found on PATH)")]
    NotInstalled(String),

    #[error("its transcript isn't on this machine")]
    NotHere,

    #[error(transparent)]
    Harness(#[from] ResumeError),
}

/// Plans resuming a session.
#[async_trait]
pub trait Resumer: Send + Sync {
    /// How to resume `session` here, checked against the filesystem. Never writes anything.
    async fn plan(&self, session: &SessionRow) -> Result<ResumePlan, NotResumable>;
}

/// The shell line for a plan: `cd -- <cwd> && <command>`.
pub fn shell_line(plan: &ResumePlan) -> String {
    plan.render().unwrap_or_else(|_| plan.command())
}

/// What a [`HarnessResumer`] reads on this machine: the harness's transcripts and the programs
/// on `PATH`. Tests swap it for one that touches nothing.
#[async_trait]
pub trait Machine: Send + Sync {
    /// Where `harness` keeps session `id` here ([`Harness::locate`]).
    ///
    /// [`Harness::locate`]: atuin_common::harnesstools::Harness::locate
    async fn locate(&self, harness: AnyHarness, id: &str) -> Option<PathBuf>;

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

    fn installed(&self, program: &str) -> bool {
        on_path(program)
    }
}

/// The real [`Resumer`]: the harness's own resume command (or the user's template), for a
/// session whose transcript is on this machine.
pub struct HarnessResumer {
    templates: AiSessionResume,
    machine: Box<dyn Machine>,
}

impl HarnessResumer {
    pub fn new(templates: AiSessionResume) -> Self {
        Self::on(templates, ThisMachine)
    }

    /// A resumer looking at `machine` instead of this one.
    pub fn on(templates: AiSessionResume, machine: impl Machine + 'static) -> Self {
        Self {
            templates,
            machine: Box::new(machine),
        }
    }
}

#[async_trait]
impl Resumer for HarnessResumer {
    async fn plan(&self, session: &SessionRow) -> Result<ResumePlan, NotResumable> {
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let id = session.handle.session.as_ref();

        let mut target = ResumeTarget::new(id);
        if let Some(cwd) = &session.cwd {
            target = target.with_cwd(cwd);
        }
        // The harness decides first (a subagent is never resumable, whatever is on disk).
        harness.resume_plan(&target)?;

        let native = self.machine.locate(harness, id).await.ok_or(NotResumable::NotHere)?;
        let target = target.with_native_path(native);
        let plan = harness.resume(&target, self.templates.template(kind))?.prepare()?;
        if !self.machine.installed(&program_to_check(&plan)) {
            return Err(NotResumable::NotInstalled(plan.program));
        }
        Ok(plan)
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
