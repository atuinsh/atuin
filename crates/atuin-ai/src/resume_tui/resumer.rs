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
//! A session that went on separately on several machines is restored along one branch only (the
//! newest, unless one was named), never with every branch's messages at once.
//! One that is here is caught up with sync first ([`Resumer::catch_up`], [`super::catchup`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use atuin_client::ai_session::{HarnessKind, SourceId};
use atuin_client::settings::AiSessionResume;
use atuin_common::harnesstools::continuation::{self, Flattened};
use atuin_common::harnesstools::rehydrate::{
    ForkOf, RehydrateError, RehydrateMessage, RehydrateSession,
};
pub use atuin_common::harnesstools::resume::{ResumeError, ResumePlan, ResumeTarget, quote};
use atuin_common::harnesstools::sync::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, ReplaceOutcome, SessionSync as _, SyncError,
};
use atuin_common::harnesstools::{AnyHarness, Harness as _, fork};

use super::ResumeContext;
use super::catchup::{self, CatchUp, Held, Step, Why};
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

    #[error("forking it failed: {0}")]
    Fork(String),

    #[error("catching it up with sync failed: {0}")]
    CatchUp(String),

    #[error("switching it to another branch failed: {0}")]
    Switch(String),

    /// A continuation or fork of a session with nothing of the conversation in it.
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
    /// Set when the session's transcript is here: it is caught up with sync first
    /// ([`Resumer::catch_up`]), and `plan` resumes it as it is.
    pub catch_up: bool,
    /// Whether an agent here has the session open, or may have.
    pub live: bool,
}

impl Resume {
    /// A session ready to resume with `plan`.
    pub fn ready(plan: ResumePlan) -> Self {
        Self {
            plan,
            restore: None,
            catch_up: false,
            live: false,
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
    /// The status line: `continuing in Codex: 42 tool calls become notes`.
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

/// What a fork starts from ([`Resumer::fork`]): by default, every row synced of the session.
#[derive(Debug, Clone, Default)]
pub struct ForkFrom {
    /// The rows to fork, in transcript order, instead of those synced of the session.
    pub rows: Option<Vec<RehydrateMessage>>,
    /// The row to fork at, by source id: the rows up to it, it included.
    pub tip: Option<String>,
}

/// A session forked: written out as a new session of its own harness, ready to resume with
/// `plan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forked {
    pub harness: HarnessKind,
    /// The fork's native id.
    pub id: String,
    pub plan: ResumePlan,
    /// Why it resumes somewhere other than the directory the session ran in, when it does.
    pub note: Option<String>,
}

impl Forked {
    /// The status line: `forked into a new Claude Code session`. It names no id: the fork's
    /// atuin id is minted by capture when it first sees the fork, which can't be known here.
    pub fn status(&self) -> String {
        format!("forked into a new {} session", harness_label(self.harness))
    }
}

/// This machine's copy of a session, switched to another branch (in place: [`Resumer::switch`]),
/// ready to resume with `plan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Switched {
    /// The branch's host, as [`catchup::host_label`] names it.
    pub host: String,
    /// The messages the copy holds now, as [`catchup::message_count`] counts them.
    pub messages: usize,
    /// Where the copy as it was before the switch is kept.
    pub backup: PathBuf,
    /// What went wrong once the copy was switched, which the status says too.
    pub warning: Option<String>,
    pub plan: ResumePlan,
}

impl Switched {
    /// The status line: `switched to @3f9a12bc's branch: 12 messages (your copy is at …)`, and
    /// what went wrong once it was, if anything.
    pub fn status(&self) -> String {
        let status = catchup::switched(self.messages, &self.host, &self.backup);
        match &self.warning {
            Some(warning) => format!("{status}; but {warning}"),
            None => status,
        }
    }
}

/// Why a fork from a [tip](ForkFrom::tip) fails when no row is it.
const NO_TIP: &str = "no row is the one to fork at";

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

    /// Bring this machine's copy of `session` to `head` (one of its heads; the newest when
    /// `None`) from what `source` holds of it, and plan resuming it ([`super::catchup`]): restore
    /// it when it isn't here, append what it lacks, or say what to choose. Only once the user has
    /// chosen to resume it.
    async fn catch_up(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        _head: Option<&SourceId>,
    ) -> Result<CatchUp, NotResumable> {
        let resume = self.plan(session).await?;
        let (plan, status) = match &resume.restore {
            Some(restore) => (self.restore(source, session, restore).await?, restored(restore)),
            None => (resume.plan, None),
        };
        Ok(CatchUp::Ready { plan, status })
    }

    /// Switch this machine's copy of `session` to `head`'s branch (the newest it can be switched
    /// to when `None`), from what `source` holds of it, and plan resuming it ([`super::catchup`]):
    /// in place, keeping the history it shares with the branch as it is, and the copy as it was
    /// as a backup. Refused, with nothing written, unless sync holds every row of the copy, no
    /// agent here has it open, and its agent's store is a transcript that can be switched so.
    /// Only once the user has chosen to.
    async fn switch(
        &self,
        _source: &dyn SessionSource,
        session: &SessionRow,
        _head: Option<&SourceId>,
    ) -> Result<Switched, NotResumable> {
        Err(NotResumable::Unsupported(harness_label(session.handle.harness)))
    }

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

    /// Whether `session` can be forked here: its harness forks, and is installed.
    fn can_fork(&self, _session: &SessionRow) -> bool {
        false
    }

    /// Write `session` out as a new session of its own harness, linked to it as its fork
    /// ([`fork`]), from `from` (by default what `source` holds of it), and plan resuming that.
    /// Only once the user has chosen to.
    async fn fork(
        &self,
        _source: &dyn SessionSource,
        session: &SessionRow,
        _from: ForkFrom,
    ) -> Result<Forked, NotResumable> {
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

    /// What `harness`'s copy of session `id` here holds ([`SessionSync::local_tip`]).
    ///
    /// [`SessionSync::local_tip`]: atuin_common::harnesstools::sync::SessionSync::local_tip
    async fn local_tip(
        &self,
        _harness: AnyHarness,
        _id: &str,
    ) -> Result<Option<LocalTip>, SyncError> {
        Ok(None)
    }

    /// Whether a `harness` process here may be writing session `id`
    /// ([`SessionSync::is_live`]).
    ///
    /// [`SessionSync::is_live`]: atuin_common::harnesstools::sync::SessionSync::is_live
    async fn is_live(&self, _harness: AnyHarness, _id: &str, _cwd: Option<&Path>) -> Liveness {
        Liveness::NotLive
    }

    /// Append `lines` to `harness`'s copy of session `id`, read as `base`
    /// ([`SessionSync::append`]).
    ///
    /// [`SessionSync::append`]: atuin_common::harnesstools::sync::SessionSync::append
    async fn append(
        &self,
        _harness: AnyHarness,
        _id: &str,
        _base: &LocalTip,
        _lines: &[RehydrateMessage],
        _options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        Err(SyncError::Unsupported("this machine can't catch sessions up"))
    }

    /// Switch `harness`'s copy of session `id`, read as `base`, to the branch `branch` (its rows
    /// root to head), keeping the copy as it was in `backups` ([`SessionSync::replace`]).
    ///
    /// [`SessionSync::replace`]: atuin_common::harnesstools::sync::SessionSync::replace
    async fn replace(
        &self,
        _harness: AnyHarness,
        _id: &str,
        _base: &LocalTip,
        _branch: &[RehydrateMessage],
        _options: &AppendOptions<'_>,
        _backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError> {
        Err(SyncError::Unsupported("this machine can't switch sessions"))
    }
}

/// What the status line says of a session restored as `restore`, when anything.
fn restored(restore: &Restore) -> Option<String> {
    restore.note.as_ref().map(|note| format!("restored the session from sync; {note}"))
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

    async fn local_tip(
        &self,
        harness: AnyHarness,
        id: &str,
    ) -> Result<Option<LocalTip>, SyncError> {
        harness.local_tip(id).await
    }

    async fn is_live(&self, harness: AnyHarness, id: &str, cwd: Option<&Path>) -> Liveness {
        harness.is_live(id, cwd).await
    }

    async fn append(
        &self,
        harness: AnyHarness,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        harness.append(id, base, lines, options).await
    }

    async fn replace(
        &self,
        harness: AnyHarness,
        id: &str,
        base: &LocalTip,
        branch: &[RehydrateMessage],
        options: &AppendOptions<'_>,
        backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError> {
        harness.replace(id, base, branch, options, backups).await
    }
}

/// The real [`Resumer`]: the harness's own resume command (or the user's template), restoring
/// the session's transcript from sync when it isn't on this machine.
pub struct HarnessResumer {
    templates: AiSessionResume,
    context: ResumeContext,
    machine: Box<dyn Machine>,
    /// Where a copy switched to another branch is kept as it was: a directory of each harness's
    /// within it ([`switched_dir`]).
    switched: PathBuf,
}

/// Where copies switched to another branch are kept as they were: `ai/switched` in atuin's data
/// directory (`data_dir`, `ATUIN_DATA_DIR`), which no agent lists sessions from.
pub fn switched_dir() -> PathBuf {
    atuin_client::settings::Settings::effective_data_dir().join("ai").join("switched")
}

impl HarnessResumer {
    pub fn new(context: ResumeContext, templates: AiSessionResume) -> Self {
        Self::on(context, templates, ThisMachine)
    }

    /// The resumer, keeping copies switched to another branch in `dir` instead of
    /// [`switched_dir`].
    #[cfg(test)]
    #[must_use]
    pub fn keeping_switched_in(mut self, dir: impl Into<PathBuf>) -> Self {
        self.switched = dir.into();
        self
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
            switched: switched_dir(),
        }
    }

    /// Whether `kind` could resume a session here, in the directory `session` would: it is
    /// installed (or the user's template's program is).
    fn runs_here(&self, kind: HarnessKind, session: &SessionRow) -> bool {
        let target = ResumeTarget::new("atuin")
            .with_cwd(resolve_cwd(session.cwd.as_deref(), &self.context).cwd);
        kind.harness()
            .and_then(|harness| self.unwritten_plan(harness, kind, &target).ok())
            .is_some_and(|plan| self.check_installed(&plan).is_ok())
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

    /// [`Resumer::restore`], along `head` (see [`restored_session`]).
    async fn restore_along(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        restore: &Restore,
        head: Option<&SourceId>,
    ) -> Result<ResumePlan, NotResumable> {
        let data = restored_session(source, session, &restore.cwd, head)
            .await
            .map_err(|e| NotResumable::Restore(format!("{e:#}")))?;
        self.write_out(session.handle.harness, &data, NotResumable::Restore).await
    }
}

/// `session` as `source` holds it, to be written out and resumed in `cwd`: along `head` (or, for
/// a session that diverged, its newest head) when there is one ([`catchup::branch_rows`]), so no
/// other branch's messages come with it; else every row synced of it.
async fn restored_session(
    source: &dyn SessionSource,
    session: &SessionRow,
    cwd: &Path,
    head: Option<&SourceId>,
) -> eyre::Result<RehydrateSession> {
    let mut data = source.rehydrate(&session.handle, cwd).await?;
    if let Some(rows) = catchup::branch_rows(source, &session.handle, head).await? {
        data.messages = rows;
    }
    Ok(data)
}

/// This machine's copy of a session, as [`HarnessResumer::plan_here`] read it.
struct Copy {
    /// What it holds ([`Machine::local_tip`]).
    tip: Result<Option<LocalTip>, SyncError>,
    /// Where an agent here would have it open, as asked whether one has: where the copy records
    /// working, else where the session works here ([`local_dir`]); `None` (an agent of its
    /// harness anywhere counts) when neither is known.
    dir: Option<PathBuf>,
}

impl HarnessResumer {
    /// [`Resumer::plan`], with the copy here it planned resuming, when there is one.
    async fn plan_here(
        &self,
        session: &SessionRow,
    ) -> Result<(Resume, Option<Copy>), NotResumable> {
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
                // Where this copy works, as it records; else where the session works here,
                // which for another host's session isn't where it was recorded.
                let tip = self.machine.local_tip(harness, id).await;
                let recorded = tip.as_ref().ok().and_then(|t| t.as_ref()?.cwd.clone());
                let dir = recorded
                    .filter(|dir| dir.is_dir())
                    .or_else(|| local_dir(session.cwd.as_deref(), &self.context));
                let live = self.machine.is_live(harness, id, dir.as_deref()).await;
                let resume = Resume {
                    catch_up: true,
                    live: live != Liveness::NotLive,
                    ..Resume::ready(harness.resume(&target, template)?.prepare()?)
                };
                (resume, Some(Copy { tip, dir }))
            }
            None => {
                let restore = resolve_cwd(session.cwd.as_deref(), &self.context);
                target.cwd = Some(restore.cwd.clone());
                // The transcript is only written once the user accepts the restore.
                let resume = Resume {
                    restore: Some(restore),
                    ..Resume::ready(self.unwritten_plan(harness, kind, &target)?)
                };
                (resume, None)
            }
        };
        self.check_installed(&resume.0.plan)?;
        Ok(resume)
    }
}

#[async_trait]
impl Resumer for HarnessResumer {
    async fn plan(&self, session: &SessionRow) -> Result<Resume, NotResumable> {
        Ok(self.plan_here(session).await?.0)
    }

    async fn restore(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        restore: &Restore,
    ) -> Result<ResumePlan, NotResumable> {
        self.restore_along(source, session, restore, None).await
    }

    async fn catch_up(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        head: Option<&SourceId>,
    ) -> Result<CatchUp, NotResumable> {
        // The append looks for an agent where the plan did, in the copy the plan read.
        let (resume, copy) = self.plan_here(session).await?;
        let ready = |plan, status| Ok(CatchUp::Ready { plan, status });
        if let Some(restore) = &resume.restore {
            let plan = self.restore_along(source, session, restore, head).await?;
            return ready(plan, restored(restore));
        }
        let Some(Copy { tip, dir }) = copy else {
            return ready(resume.plan, None);
        };
        let tip = match tip {
            Ok(Some(tip)) => Ok(tip),
            Ok(None) => {
                // Gone since it was located: restored, as a copy that isn't here is.
                let restore = resolve_cwd(session.cwd.as_deref(), &self.context);
                let plan = self.restore_along(source, session, &restore, head).await?;
                return ready(plan, restored(&restore));
            }
            Err(e) => Err(e),
        };
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let id = session.handle.session.as_ref();
        let analysis = source.analyse(&session.handle).await;
        let Some(analysis) = analysis.map_err(|e| NotResumable::CatchUp(format!("{e:#}")))? else {
            return ready(resume.plan, None);
        };
        let heads = analysis.heads();
        let chosen = match head {
            Some(named) => heads.iter().position(|h| h.source_id == *named).ok_or_else(|| {
                NotResumable::CatchUp(format!("{named} is no longer one of its branches' heads"))
            })?,
            None => 0,
        };
        let Some(head) = heads.get(chosen) else {
            return ready(resume.plan, None);
        };
        let here = &self.context.host_id;
        let tip = match tip {
            Ok(tip) => tip,
            // A copy the agent can't read, or can't catch up (opencode 2.0's): never resumed
            // as if it were caught up.
            Err(e) => {
                let branches = catchup::branches(&analysis, None, here);
                return Ok(held(Why::Refused(e.to_string()), kind, resume.plan, branches, chosen));
            }
        };
        let branches = || catchup::branches(&analysis, Some(&tip), here);
        let (rows, base) = match catchup::classify(&analysis, kind, &head.source_id, &tip) {
            Step::AsIs | Step::Ahead => return ready(resume.plan, None),
            Step::Choice(why) => {
                let mut branches = branches();
                // Offered only for a copy on another line alone: not one with rows sync hasn't
                // got, which the switch would lose, nor one an agent here has open.
                if why == Why::Diverged && !resume.live {
                    for branch in &mut branches {
                        branch.switch =
                            catchup::can_switch(&analysis, kind, &branch.head.source_id, &tip);
                    }
                }
                return Ok(held(why, kind, resume.plan, branches, chosen));
            }
            Step::FastForward { rows, base } => (rows, *base),
        };
        // Never under an agent that may be writing the session.
        if resume.live {
            return Ok(held(Why::Live, kind, resume.plan, branches(), chosen));
        }
        // Ids the copy must not take (pi's are short): every row synced on any branch.
        let mut taken: HashSet<String> = heads
            .iter()
            .flat_map(|h| analysis.path_to(&h.source_id))
            .map(|m| m.source_id.to_string())
            .collect();
        for row in &rows {
            taken.remove(&row.source_id);
        }
        let options = AppendOptions {
            taken_ids: Some(&taken),
            cwd: dir.as_deref(),
        };
        let outcome = match self.machine.append(harness, id, &base, &rows, &options).await {
            Ok(outcome) => outcome,
            Err(e) => {
                let why = match e {
                    SyncError::Live(_) | SyncError::MaybeLive => Why::Live,
                    e => Why::Refused(e.to_string()),
                };
                return Ok(held(why, kind, resume.plan, branches(), chosen));
            }
        };
        // Resumed from where it was written, which a `{path}` template names.
        let plan = if outcome.native_path == tip.native_path {
            resume.plan
        } else {
            let mut target = ResumeTarget::new(id).with_native_path(&outcome.native_path);
            target.cwd.clone_from(&resume.plan.cwd);
            harness.resume(&target, self.templates.template(kind))?.prepare()?
        };
        let status = (!outcome.appended.is_empty()).then(|| {
            catchup::caught_up(catchup::message_count(&rows), &catchup::host_label(head, here))
        });
        ready(plan, status)
    }

    async fn switch(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        head: Option<&SourceId>,
    ) -> Result<Switched, NotResumable> {
        let fail = |why: String| NotResumable::Switch(why);
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let id = session.handle.session.as_ref();
        // The replace looks for an agent where the plan did, in the copy the plan read.
        let (resume, copy) = self.plan_here(session).await?;
        let Some(Copy { tip, dir }) = copy else {
            return Err(fail("there's no copy of it here: resuming it restores it".to_owned()));
        };
        let tip = match tip {
            Ok(Some(tip)) => tip,
            Ok(None) => {
                return Err(fail("there's no copy of it here: resuming it restores it".to_owned()));
            }
            Err(e) => return Err(fail(e.to_string())),
        };
        let analysis = source.analyse(&session.handle).await.map_err(|e| fail(format!("{e:#}")))?;
        let analysis = analysis.ok_or_else(|| {
            fail("its branches aren't known yet: the daemon hasn't synced its messages".to_owned())
        })?;
        let head = catchup::switch_to(&analysis, kind, head, &tip).map_err(fail)?;
        let running = || fail(format!("{} is running this session here", harness_label(kind)));
        // Never under an agent that may be writing the session: checked again as it is written.
        if resume.live {
            return Err(running());
        }
        // The branch's rows, with those that go with it beside the tree (pi's prompts from
        // before ids, extensions' messages, titles), as a restore or a fork writes it: the copy
        // keeps those it shares, and the rest are appended to them as a fast-forward appends
        // them.
        let branch: Vec<RehydrateMessage> =
            analysis.rows_for(&head.source_id).into_iter().map(|m| m.clone().into()).collect();
        // Ids the rows appended must not take (pi's are short): every row synced on any branch.
        let mut taken: HashSet<String> = analysis
            .heads()
            .iter()
            .flat_map(|h| analysis.path_to(&h.source_id))
            .map(|m| m.source_id.to_string())
            .collect();
        for row in &branch {
            taken.remove(&row.source_id);
        }
        let options = AppendOptions {
            taken_ids: Some(&taken),
            cwd: dir.as_deref(),
        };
        let backups = self.switched.join(harness.name());
        let replaced = self.machine.replace(harness, id, &tip, &branch, &options, &backups).await;
        let outcome = match replaced {
            Ok(outcome) => outcome,
            Err(SyncError::Live(_) | SyncError::MaybeLive) => return Err(running()),
            Err(e) => return Err(fail(e.to_string())),
        };
        // Resumed from where it was written, which a `{path}` template names.
        let plan = if outcome.native_path == tip.native_path {
            resume.plan
        } else {
            let mut target = ResumeTarget::new(id).with_native_path(&outcome.native_path);
            target.cwd.clone_from(&resume.plan.cwd);
            harness.resume(&target, self.templates.template(kind))?.prepare()?
        };
        Ok(Switched {
            host: catchup::host_label(head, &self.context.host_id),
            messages: catchup::message_count(&branch),
            backup: outcome.backup,
            warning: outcome.warning,
            plan,
        })
    }

    fn continue_targets(&self, session: &SessionRow) -> Vec<HarnessKind> {
        let own = session.handle.harness;
        if own.harness().is_none() {
            return Vec::new();
        }
        AnyHarness::all()
            .iter()
            .map(HarnessKind::from)
            .filter(|kind| *kind != own && self.runs_here(*kind, session))
            .collect()
    }

    /// Every harness atuin writes forks (Copilot isn't one), when installed.
    fn can_fork(&self, session: &SessionRow) -> bool {
        self.runs_here(session.handle.harness, session)
    }

    async fn fork(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        from: ForkFrom,
    ) -> Result<Forked, NotResumable> {
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let fail = |e: String| NotResumable::Fork(e);
        let restore = resolve_cwd(session.cwd.as_deref(), &self.context);
        // Restored as a resume would restore it (along its newest head, when it diverged), should
        // pi need it written out below.
        let mut original = restored_session(source, session, &restore.cwd, None)
            .await
            .map_err(|e| fail(format!("{e:#}")))?;
        // Checked before anything is written. A fork of no conversation is refused, as a
        // continuation of it is: opencode's would have no prompt to carry its link.
        let rows = from.rows.as_deref().unwrap_or(&original.messages);
        let rows = match from.tip.as_deref() {
            Some(tip) => fork::up_to(rows, tip).ok_or_else(|| fail(NO_TIP.to_owned()))?,
            None => rows,
        };
        continuation::conversational(rows)?;
        let planned = ResumeTarget::new("atuin").with_cwd(&restore.cwd);
        self.check_installed(&self.unwritten_plan(harness, kind, &planned)?)?;
        // pi names a fork's parent by its file: the original is restored first when it isn't here.
        // A Codex fork continues the history its original's rollout does, when that is here (a
        // reverted thread's holds only what came after the revert); restored, it continues none,
        // as the original restored would.
        let id = session.handle.session.as_ref();
        let path = match harness {
            AnyHarness::Pi(_) => Some(match self.machine.locate(harness, id).await {
                Some(path) => path,
                None => match self.machine.rehydrate(harness, &original).await {
                    Ok(path) | Err(RehydrateError::AlreadyExists(path)) => path,
                    Err(e) => return Err(fail(format!("restoring the original: {e}"))),
                },
            }),
            AnyHarness::Codex(_) => self.machine.locate(harness, id).await,
            _ => None,
        };
        if let Some(rows) = from.rows {
            original.messages = rows;
        }
        let of = ForkOf {
            id: id.to_owned(),
            atuin_id: Some(session.atuin_id.to_string()),
            path,
        };
        let forked = fork::fork(harness, &original, of, from.tip.as_deref())
            .ok_or_else(|| fail(NO_TIP.to_owned()))?;
        let plan = self.write_out(kind, &forked, fail).await?;
        Ok(Forked {
            harness: kind,
            id: forked.id,
            plan,
            note: restore.note,
        })
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

/// Catching up held for `why`: the choice between resuming the copy here with `plan` as it is and
/// forking from one of `branches`.
fn held(
    why: Why,
    harness: HarnessKind,
    plan: ResumePlan,
    branches: Vec<catchup::Branch>,
    chosen: usize,
) -> CatchUp {
    CatchUp::Choice(Box::new(Held {
        why,
        harness,
        plan,
        branches,
        chosen,
    }))
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
    match in_checkout(original, context) {
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

/// The directory here a session that ran in `original` works in: `original` when it exists,
/// else the same place in this checkout of its repository ([`resolve_cwd`]); `None` when neither
/// is known, as the current directory [`resolve_cwd`] falls back to says nothing of where an
/// agent may already have it open.
fn local_dir(original: Option<&Path>, context: &ResumeContext) -> Option<PathBuf> {
    let original = original?;
    if original.is_dir() {
        return Some(original.to_owned());
    }
    in_checkout(original, context)
}

/// The same place as `original` in the checkout the current directory is in, or its root, when
/// `original` was in a repository named like it ([`resolve_cwd`]).
fn in_checkout(original: &Path, context: &ResumeContext) -> Option<PathBuf> {
    context.git_root.as_deref().and_then(|root| {
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
    })
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
