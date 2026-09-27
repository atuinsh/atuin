//! The picker's resume seam: how a session turns into a command, or why it can't here.
//!
//! Plans come from the harness tools ([`atuin_common::harnesstools::resume`]); this layer adds
//! what only the picker knows: which host recorded the session, whether its transcript is on this
//! machine, and the user's `[ai.sessions.resume]` templates. Planning may walk the harness's
//! session directories, so the picker asks only for the selected session, never per row.

use async_trait::async_trait;
use atuin_client::settings::AiSessionResume;
use atuin_common::harnesstools::Harness as _;
pub use atuin_common::harnesstools::resume::{ResumeError, ResumePlan, ResumeTarget};

use super::source::{SessionRow, harness_label};

/// Why a session can be viewed but not resumed here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotResumable {
    #[error("recorded on {0}; only viewable here")]
    Remote(String),

    #[error("atuin can't resume {0} sessions")]
    Unsupported(&'static str),

    #[error("its transcript isn't on this machine any more")]
    TranscriptMissing,

    #[error(transparent)]
    Harness(#[from] ResumeError),
}

/// Plans resuming a session.
#[async_trait]
pub trait Resumer: Send + Sync {
    /// How to resume `session` here, checked against the filesystem.
    async fn plan(&self, session: &SessionRow) -> Result<ResumePlan, NotResumable>;
}

/// The shell line for a plan: `cd -- <cwd> && <command>`.
pub fn shell_line(plan: &ResumePlan) -> String {
    plan.render().unwrap_or_else(|_| plan.command())
}

/// The real [`Resumer`]: the harness's own resume command (or the user's template), for sessions
/// recorded on this host whose transcript is still on disk.
pub struct HarnessResumer {
    host_id: String,
    templates: AiSessionResume,
}

impl HarnessResumer {
    pub fn new(host_id: impl Into<String>, templates: AiSessionResume) -> Self {
        Self {
            host_id: host_id.into(),
            templates,
        }
    }
}

#[async_trait]
impl Resumer for HarnessResumer {
    async fn plan(&self, session: &SessionRow) -> Result<ResumePlan, NotResumable> {
        if session.host_id != self.host_id {
            return Err(NotResumable::Remote(session.hostname.clone()));
        }
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported(harness_label(kind)))?;
        let id = session.handle.session.as_ref();

        let mut target = ResumeTarget::new(id);
        if let Some(cwd) = &session.cwd {
            target = target.with_cwd(cwd);
        }
        // The harness decides first (a subagent is never resumable, whatever is on disk).
        harness.resume_plan(&target)?;
        let native = harness.locate(id).await.ok_or(NotResumable::TranscriptMissing)?;
        let target = target.with_native_path(native);

        Ok(harness.resume(&target, self.templates.template(kind))?.prepare()?)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use atuin_client::ai_session::HarnessKind;
    use atuin_common::harnesstools::resume::CwdRequirement;
    use rstest::rstest;

    use super::*;
    use crate::resume_tui::fake;

    #[rstest]
    fn shell_line_quotes_the_directory() {
        let plan = ResumePlan {
            program: "claude".to_owned(),
            args: vec!["--resume".to_owned(), "abc".to_owned()],
            cwd: Some(PathBuf::from("/tmp/it's here")),
            cwd_requirement: CwdRequirement::Preferred,
            native_path: None,
        };
        assert_eq!(shell_line(&plan), r"cd -- '/tmp/it'\''s here' && claude --resume abc");
    }

    #[rstest]
    #[tokio::test]
    async fn remote_and_unsupported_sessions_are_not_resumable() {
        let resumer = HarnessResumer::new(fake::THIS_HOST_ID, AiSessionResume::default());

        let mut row = fake::row(HarnessKind::ClaudeCode, "abc-123", "t");
        row.host_id = "other".to_owned();
        row.hostname = "buildbox".to_owned();
        let err = resumer.plan(&row).await.unwrap_err();
        assert_eq!(err, NotResumable::Remote("buildbox".to_owned()));
        assert_eq!(err.to_string(), "recorded on buildbox; only viewable here");

        let row = fake::row(HarnessKind::Copilot, "cp-1", "t");
        assert_eq!(resumer.plan(&row).await.unwrap_err(), NotResumable::Unsupported("Copilot"));

        // A Claude Code subagent is refused by the harness before anything is looked up.
        let row = fake::row(HarnessKind::ClaudeCode, "agent-a1b2", "t");
        assert!(matches!(
            resumer.plan(&row).await.unwrap_err(),
            NotResumable::Harness(ResumeError::NotResumable(_))
        ));
    }
}
