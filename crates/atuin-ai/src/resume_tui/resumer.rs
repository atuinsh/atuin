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
//!
//! Resuming goes through [`Resumer::sync`], which also catches a transcript that is here up with
//! sync first, or puts the branch the user picked in it ([`super::catchup`]).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use atuin_client::ai_session::{HarnessKind, Head, SourceId};
use atuin_client::settings::AiSessionResume;
use atuin_common::harnesstools::continuation::{self, Flattened};
use atuin_common::harnesstools::rehydrate::{RehydrateError, RehydrateMessage, RehydrateSession};
pub use atuin_common::harnesstools::resume::{ResumeError, ResumePlan, ResumeTarget, quote};
use atuin_common::harnesstools::sync::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, SessionSync as _, SyncError,
};
use atuin_common::harnesstools::{AnyHarness, Harness as _};

use super::ResumeContext;
use super::catchup::{Caught, Kept, Step, Synced, classify};
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

    #[error("catching it up with sync failed: {0}")]
    CatchUp(String),

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
    /// The status line: `continuing in Codex: 42 tool calls flattened to notes, reasoning
    /// dropped`.
    pub fn status(&self) -> String {
        let label = harness_label(self.target);
        match self.flattened.summary() {
            summary if summary.is_empty() => format!("continuing in {label}"),
            summary => format!("continuing in {label}: {summary}"),
        }
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

    /// Bring this machine's copy of `session` to `head` (one of its heads; the newest when
    /// `None`) from what `source` holds of it, and plan resuming it ([`super::catchup`]): write it
    /// out when it isn't here, append what it lacks, or put the head's branch in it. Only once
    /// the user has chosen to resume it.
    ///
    /// Without heads to go by, a session is resumed as [`plan`](Self::plan) and
    /// [`restore`](Self::restore) have it.
    async fn sync(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        _head: Option<&SourceId>,
    ) -> Result<Synced, NotResumable> {
        let resume = self.plan(session).await?;
        let Some(restore) = resume.restore else {
            return Ok(Synced::up_to_date(resume.plan));
        };
        let plan = self.restore(source, session, &restore).await?;
        Ok(Synced {
            caught: Caught::Restored {
                rows: 0,
                note: restore.note,
            },
            ..Synced::up_to_date(plan)
        })
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
    async fn local_tip(&self, harness: AnyHarness, id: &str)
    -> Result<Option<LocalTip>, SyncError>;

    /// Whether a `harness` process here may be writing session `id`
    /// ([`SessionSync::is_live`]).
    ///
    /// [`SessionSync::is_live`]: atuin_common::harnesstools::sync::SessionSync::is_live
    async fn is_live(&self, harness: AnyHarness, id: &str, cwd: Option<&Path>) -> Liveness;

    /// Append `lines` to `harness`'s copy of session `id`, read as `base`
    /// ([`SessionSync::append`]).
    ///
    /// [`SessionSync::append`]: atuin_common::harnesstools::sync::SessionSync::append
    async fn append(
        &self,
        harness: AnyHarness,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError>;
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
        if self.machine.installed(&plan.program) {
            Ok(())
        } else {
            Err(NotResumable::NotInstalled(plan.program.clone()))
        }
    }

    /// Whether `head` was written on this machine (rows from before hosts were recorded were).
    fn is_here(&self, head: &Head) -> bool {
        head.host.is_none_or(|h| h.0.as_simple().to_string() == self.host_id)
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

    /// Write `session` out from sync along `head`'s branch (the whole session when it went one
    /// way, `branched` false, as a restore always did), and plan resuming it, with how many rows
    /// were written.
    async fn restore_branch(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        head: &Head,
        branched: bool,
        restore: &Restore,
    ) -> Result<(ResumePlan, usize), NotResumable> {
        let handle = &session.handle;
        let data = if branched {
            source.branch(handle, &head.source_id, &restore.cwd).await
        } else {
            source.rehydrate(handle, &restore.cwd).await
        };
        let data = data.map_err(|e| NotResumable::Restore(format!("{e:#}")))?;
        let rows = data.messages.len();
        Ok((self.write_out(session, &data, restore).await?, rows))
    }

    /// `head`'s branch of `session`, which its harness can't take in place, continued as a new
    /// session of the same harness linked to it ([`continuation`]), resuming in `cwd`: its plan,
    /// and what was flattened.
    async fn fork(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        head: &Head,
        cwd: &Path,
    ) -> Result<(ResumePlan, Flattened), NotResumable> {
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let fail = NotResumable::CatchUp;
        let branch = source
            .branch(&session.handle, &head.source_id, cwd)
            .await
            .map_err(|e| fail(format!("{e:#}")))?;
        let continued = continuation::continue_in(harness, &branch, harness);
        let template = self.templates.template(kind);
        let planned = ResumeTarget::new(&continued.session.id).with_cwd(cwd);
        self.check_installed(&harness.resume(&planned, template)?)?;
        let native = self
            .machine
            .rehydrate(harness, &continued.session)
            .await
            .map_err(|e| fail(e.to_string()))?;
        let plan = harness.resume(&planned.with_native_path(native), template)?.prepare()?;
        Ok((plan, continued.flattened))
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

    #[allow(clippy::too_many_lines)]
    async fn sync(
        &self,
        source: &dyn SessionSource,
        session: &SessionRow,
        head: Option<&SourceId>,
    ) -> Result<Synced, NotResumable> {
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let handle = &session.handle;
        let id = handle.session.as_ref();
        let resume = self.plan(session).await?;
        let failed = |e: eyre::Report| NotResumable::CatchUp(format!("{e:#}"));

        let heads = source.heads(handle).await.map_err(failed)?.unwrap_or_default();
        let picked = head.and_then(|h| heads.heads.iter().find(|x| &x.source_id == h));
        let Some(chosen) = picked.or(heads.heads.first()).cloned() else {
            // Nothing to go by: as before.
            let Some(restore) = resume.restore else {
                return Ok(Synced::up_to_date(resume.plan));
            };
            let data = source.rehydrate(handle, &restore.cwd).await.map_err(failed)?;
            let rows = data.messages.len();
            let plan = self.write_out(session, &data, &restore).await?;
            return Ok(Synced {
                caught: Caught::Restored {
                    rows,
                    note: restore.note,
                },
                ..Synced::up_to_date(plan)
            });
        };
        let branched = heads.heads.len() > 1;
        let others: Vec<Head> = if heads.diverged {
            heads.heads.iter().filter(|h| h.source_id != chosen.source_id).cloned().collect()
        } else {
            Vec::new()
        };
        let synced = |plan: ResumePlan, caught: Caught| Synced {
            plan,
            caught,
            head: Some(chosen.clone()),
            others: others.clone(),
        };
        let cwd = resume
            .plan
            .cwd
            .clone()
            .unwrap_or_else(|| resolve_cwd(session.cwd.as_deref(), &self.context).cwd);
        let fork = |why: &str| {
            let why = why.to_owned();
            async {
                let (plan, flattened) = self.fork(source, session, &chosen, &cwd).await?;
                Ok(synced(plan, Caught::Forked { why, flattened }))
            }
        };

        let tip = match self.machine.local_tip(harness, id).await {
            Ok(tip) => tip,
            // The harness can't catch a copy up (yet): as before, along the head's branch.
            Err(SyncError::Unsupported(why)) => {
                if let Some(restore) = &resume.restore {
                    let (plan, rows) =
                        self.restore_branch(source, session, &chosen, branched, restore).await?;
                    let note = restore.note.clone();
                    return Ok(synced(plan, Caught::Restored { rows, note }));
                }
                // The copy here is on some branch, which it can't say: another host's branch
                // is continued as a session of its own.
                if !others.is_empty() && !self.is_here(&chosen) {
                    return fork(why).await;
                }
                return Ok(synced(resume.plan, Caught::UpToDate));
            }
            Err(e) => return Err(NotResumable::CatchUp(e.to_string())),
        };
        let Some(tip) = tip else {
            let restore = resume
                .restore
                .clone()
                .unwrap_or_else(|| resolve_cwd(session.cwd.as_deref(), &self.context));
            let (plan, rows) =
                self.restore_branch(source, session, &chosen, branched, &restore).await?;
            return Ok(synced(plan, Caught::Restored {
                rows,
                note: restore.note,
            }));
        };
        // Resuming the copy here as it is.
        let here = match resume.restore {
            None => resume.plan.clone(),
            Some(_) => {
                let target =
                    ResumeTarget::new(id).with_cwd(&cwd).with_native_path(&tip.native_path);
                harness.resume(&target, self.templates.template(kind))?.prepare()?
            }
        };

        let path = source.branch(handle, &chosen.source_id, &cwd).await.map_err(failed)?.messages;
        let tip_path = match tip.tip_source_id.as_deref() {
            Some(at)
                if at != chosen.source_id.as_ref() && !path.iter().any(|m| m.source_id == at) =>
            {
                let at = SourceId::from(at.to_owned());
                Some(source.branch(handle, &at, &cwd).await.map_err(failed)?.messages)
            }
            _ => None,
        };
        let step = classify(
            &tip,
            chosen.source_id.as_ref(),
            &path,
            tip_path.as_deref(),
            picked.is_some() && branched,
            kind,
        );
        let (lines, head_line) = match &step {
            Step::AsIs | Step::Ahead => return Ok(synced(here, Caught::UpToDate)),
            Step::Unsynced => return Ok(synced(here, Caught::Kept(Kept::Unsynced))),
            Step::FastForward(rows) | Step::Branch(rows) => (rows.clone(), None),
            Step::Switch => (Vec::new(), Some(chosen.source_id.to_string())),
        };

        // Never under a harness that may be writing the session.
        match self.machine.is_live(harness, id, Some(&cwd)).await {
            Liveness::NotLive => {}
            Liveness::Live { pid } => return Ok(synced(here, Caught::Kept(Kept::Live { pid }))),
            Liveness::Unknown => return Ok(synced(here, Caught::Kept(Kept::Live { pid: None }))),
        }
        let mut taken = source.source_ids(handle).await.map_err(failed)?;
        for line in &lines {
            taken.remove(&line.source_id);
        }
        let options = AppendOptions {
            make_tip: true,
            head: head_line.as_deref(),
            taken_ids: Some(&taken),
        };
        match self.machine.append(harness, id, &tip, &lines, &options).await {
            Ok(_) => {
                // The synced messages caught up; the harness may write fewer lines for them
                // (calls captured without their input fold into notes).
                let rows = lines.len();
                let caught = match step {
                    Step::FastForward(_) => Caught::FastForwarded { rows },
                    _ => Caught::Switched { rows },
                };
                Ok(synced(here, caught))
            }
            Err(SyncError::Live(pid)) => Ok(synced(here, Caught::Kept(Kept::Live { pid }))),
            Err(SyncError::MaybeLive) => Ok(synced(here, Caught::Kept(Kept::Live { pid: None }))),
            // It can't take the branch in place (opencode keeps a session as one line).
            Err(SyncError::Unsupported(why)) => fork(why).await,
            Err(e) => Ok(synced(here, Caught::Kept(Kept::Failed(e.to_string())))),
        }
    }

    fn continue_targets(&self, session: &SessionRow) -> Vec<HarnessKind> {
        let own = session.handle.harness;
        AnyHarness::all()
            .iter()
            .map(HarnessKind::from)
            .filter(|kind| *kind != own && own.harness().is_some())
            .filter(|kind| {
                let Some(harness) = kind.harness() else {
                    return false;
                };
                harness
                    .resume(&ResumeTarget::new("atuin"), self.templates.template(*kind))
                    .is_ok_and(|plan| self.machine.installed(&plan.program))
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
            return Err(fail("it's that harness's own session".to_owned()));
        }
        let restore = resolve_cwd(session.cwd.as_deref(), &self.context);
        // A session with several branches goes on from the newest, never from all of them.
        let heads = source.heads(&session.handle).await.map_err(|e| fail(format!("{e:#}")))?;
        let original = match heads.as_ref().filter(|h| h.heads.len() > 1).and_then(|h| h.latest()) {
            Some(latest) => source.branch(&session.handle, &latest.source_id, &restore.cwd).await,
            None => source.rehydrate(&session.handle, &restore.cwd).await,
        };
        let original = original.map_err(|e| fail(format!("{e:#}")))?;
        let continued = continuation::continue_in(from, &original, into);
        let id = continued.session.id.clone();
        let template = self.templates.template(target);
        // Checked before anything is written.
        let planned = ResumeTarget::new(&id).with_cwd(&restore.cwd);
        self.check_installed(&into.resume(&planned, template)?)?;
        let native = self
            .machine
            .rehydrate(into, &continued.session)
            .await
            .map_err(|e| fail(e.to_string()))?;
        let plan = into.resume(&planned.with_native_path(native), template)?.prepare()?;
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
/// (executable, on unix).
pub fn on_path(program: &str) -> bool {
    let runnable = |path: &Path| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        {
            path.is_file()
        }
    };
    if program.contains(std::path::MAIN_SEPARATOR) || program.contains('/') {
        return runnable(Path::new(program));
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let path = dir.join(program);
        runnable(&path) || (cfg!(windows) && runnable(&path.with_extension("exe")))
    })
}

#[cfg(test)]
mod tests;
