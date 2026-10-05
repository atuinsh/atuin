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
use atuin_client::ai_session::HarnessKind;
use atuin_client::settings::AiSessionResume;
use atuin_common::harnesstools::continuation::{self, Flattened};
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

    #[error("continuing it in {0} failed: {1}")]
    Continue(&'static str, String),

    /// A continuation of a session with nothing of the conversation in it.
    #[error(transparent)]
    Empty(#[from] continuation::NothingToContinue),

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

/// What a template's `{path}` shows as in the plan for a session not yet restored: its
/// transcript has no path until it is written out.
pub const RESTORED_PATH_PLACEHOLDER: &str = "<restored transcript>";

/// A session continued in another harness: written out as a new session of `target`, ready to
/// resume with `plan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continued {
    pub target: HarnessKind,
    pub plan: ResumePlan,
    /// What the target couldn't take as it was.
    pub flattened: Flattened,
    /// Why it resumes somewhere other than the directory the session ran in, when it does.
    pub note: Option<String>,
}

impl Continued {
    /// The status line: `continuing in Codex: 42 tool calls become notes, reasoning dropped`.
    pub fn status(&self) -> String {
        continuing(self.target, Some(&self.flattened))
    }
}

/// `continuing in Codex`, and what that flattens when it's known and anything.
pub fn continuing(target: HarnessKind, flattened: Option<&Flattened>) -> String {
    let label = harness_label(target);
    match flattened.map(Flattened::summary) {
        Some(summary) if !summary.is_empty() => format!("continuing in {label}: {summary}"),
        _ => format!("continuing in {label}"),
    }
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

    /// The harnesses `session` can be continued in here: installed, and not its own.
    fn continue_targets(&self, _session: &SessionRow) -> Vec<HarnessKind> {
        Vec::new()
    }

    /// Write `session`, from what `source` holds of it, out as a new session of `target`
    /// ([`continuation`]), and plan resuming that. Only once the user has chosen to.
    async fn continue_in(
        &self,
        _source: &dyn SessionSource,
        session: &SessionRow,
        _target: HarnessKind,
    ) -> Result<Continued, NotResumable> {
        Err(NotResumable::Unsupported(harness_label(session.handle.harness)))
    }
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

    /// How `kind` would resume `target` before its transcript is written: a template's `{path}`
    /// stands for where it will go ([`RESTORED_PATH_PLACEHOLDER`]). Only for showing and
    /// checking; [`Self::write_out`] plans it again with the real path.
    fn unwritten_plan(
        &self,
        harness: AnyHarness,
        kind: HarnessKind,
        target: &ResumeTarget,
    ) -> Result<ResumePlan, NotResumable> {
        let mut plan = harness.resume_plan(target)?;
        if let Some(template) = self.templates.template(kind) {
            let preview = target.clone().with_native_path(RESTORED_PATH_PLACEHOLDER);
            plan = plan.with_template(template, &preview)?;
        }
        Ok(plan.prepare()?)
    }

    /// Write `data` out as a transcript of `kind` (`failed` says why that failed), and plan
    /// resuming it in `data.cwd`.
    async fn write_out(
        &self,
        kind: HarnessKind,
        data: &RehydrateSession,
        failed: impl FnOnce(String) -> NotResumable + Send,
    ) -> Result<ResumePlan, NotResumable> {
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let native = match self.machine.rehydrate(harness, data).await {
            Ok(path) | Err(RehydrateError::AlreadyExists(path)) => path,
            Err(e) => return Err(failed(e.to_string())),
        };
        let target = ResumeTarget::new(&data.id).with_cwd(&data.cwd).with_native_path(native);
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

        let resume = match self.machine.locate(harness, id).await {
            Some(native) => {
                // A session restored here before (another host's, or this host's whose directory
                // is gone too): resume where it was restored to.
                if !session.cwd.as_deref().is_some_and(Path::is_dir) {
                    target.cwd = Some(resolve_cwd(session.cwd.as_deref(), &self.context).cwd);
                }
                let target = target.with_native_path(native);
                Resume::ready(harness.resume(&target, template)?.prepare()?)
            }
            None => {
                let restore = resolve_cwd(session.cwd.as_deref(), &self.context);
                target.cwd = Some(restore.cwd.clone());
                // The transcript is only written once the user accepts the restore.
                Resume {
                    plan: self.unwritten_plan(harness, kind, &target)?,
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
        self.write_out(session.handle.harness, &data, NotResumable::Restore).await
    }

    fn continue_targets(&self, session: &SessionRow) -> Vec<HarnessKind> {
        let own = session.handle.harness;
        if own.harness().is_none() {
            return Vec::new();
        }
        let target = ResumeTarget::new("atuin")
            .with_cwd(resolve_cwd(session.cwd.as_deref(), &self.context).cwd);
        AnyHarness::all()
            .iter()
            .map(HarnessKind::from)
            .filter(|kind| *kind != own)
            .filter(|kind| {
                kind.harness()
                    .and_then(|harness| self.unwritten_plan(harness, *kind, &target).ok())
                    .is_some_and(|plan| self.check_installed(&plan).is_ok())
            })
            .collect()
    }

    async fn continue_in(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        target: HarnessKind,
    ) -> Result<Continued, NotResumable> {
        let from_kind = session.handle.harness;
        let from =
            from_kind.harness().ok_or(NotResumable::Unsupported(harness_label(from_kind)))?;
        let into = target.harness().ok_or(NotResumable::Unsupported(harness_label(target)))?;
        let fail = |e: String| NotResumable::Continue(harness_label(target), e);
        if from_kind == target {
            return Err(fail("it's that agent's own session".to_owned()));
        }
        let restore = resolve_cwd(session.cwd.as_deref(), &self.context);
        let original = source
            .rehydrate(&session.handle, &restore.cwd)
            .await
            .map_err(|e| fail(format!("{e:#}")))?;
        let atuin_id = session.atuin_id.to_string();
        let continued = continuation::continue_in(from, &original, Some(&atuin_id), into)?;
        // Checked before anything is written.
        let planned = ResumeTarget::new(&continued.session.id).with_cwd(&continued.session.cwd);
        self.check_installed(&self.unwritten_plan(into, target, &planned)?)?;
        let plan = self.write_out(target, &continued.session, fail).await?;
        Ok(Continued {
            target,
            plan,
            flattened: continued.flattened,
            note: restore.note,
        })
    }
}

/// Where to resume a session that ran in `original` on this machine: there, when it exists;
/// else, when the current directory is in a git repository named like one `original` was in,
/// the same place in that repository (or its root); else the current directory.
///
/// The same place is what follows a component of `original` named like the checkout. When
/// several are (`/work/atuin/other/atuin/crates` could be `other/atuin/crates` or `crates` in a
/// checkout named `atuin`), it is the one of those existing here that holds the current
/// directory (other than the checkout's root, which holds all of it), as the user is likely in
/// the right one already; else the first from the left that exists.
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
        let named: Vec<usize> =
            (0..components.len()).filter(|&i| components[i].as_os_str() == name).collect();
        if named.is_empty() {
            return None;
        }
        // Rebuilt from components, so no trailing separator comes along.
        let root: PathBuf = root.components().collect();
        // Below each component named like the checkout, the directories in it: the last may be
        // a directory in the repository named like it (`atuin/crates/atuin`).
        let places: Vec<PathBuf> = named
            .into_iter()
            .map(|at| root.components().chain(components[at + 1..].iter().copied()).collect())
            .filter(|path: &PathBuf| path.is_dir())
            .collect();
        // The root holds every directory in the checkout, so says nothing about which is meant.
        let here = places.iter().position(|place| *place != root && context.cwd.starts_with(place));
        let same_place = places.into_iter().nth(here.unwrap_or(0));
        Some(same_place.unwrap_or(root))
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
    find_program(program).is_some()
}

/// The file [`on_path`] finds for `program`, so that what runs is what was checked: on Windows that
/// is `claude.cmd` for `claude`, which spawning the bare name would not find.
pub fn find_program(program: &str) -> Option<PathBuf> {
    if program.contains(std::path::MAIN_SEPARATOR) || program.contains('/') {
        return runnable(Path::new(program));
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| runnable(&dir.join(program)))
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

/// The file at `path`, if it can be run (on Windows, `path` with a `PATHEXT` extension also counts).
fn runnable(path: &Path) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .then(|| path.to_path_buf())
    }
    #[cfg(windows)]
    {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
        let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        extensions.split(';').filter(|ext| !ext.is_empty()).find_map(|ext| {
            let mut with_ext = path.as_os_str().to_owned();
            with_ext.push(ext);
            let with_ext = PathBuf::from(with_ext);
            with_ext.is_file().then_some(with_ext)
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        path.is_file().then(|| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests;
