//! The picker's resume seam: how a session turns into a command, or why it can't.

use std::path::{Path, PathBuf};

use atuin_client::ai_session::HarnessKind;
use atuin_client::settings::AiSessionResume;

use super::source::{SessionRow, harness_label};

/// How to resume one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumePlan {
    /// The directory the command must run in, if any.
    pub cwd: Option<PathBuf>,
    /// The harness command, without the `cd`.
    pub command: String,
    /// Why the session can't be resumed here (another host, transcript gone, missing cwd).
    pub blocked: Option<String>,
}

impl ResumePlan {
    /// The full shell line: `cd -- '<cwd>' && <command>`.
    pub fn shell_line(&self) -> String {
        match &self.cwd {
            Some(cwd) => format!("cd -- {} && {}", quote(&cwd.to_string_lossy()), self.command),
            None => self.command.clone(),
        }
    }
}

/// Plans resuming a session.
pub trait Resumer: Send + Sync {
    fn plan(&self, session: &SessionRow) -> ResumePlan;
}

/// Single-quote `s` for a POSIX shell (also valid in fish, whose `'` quoting matches).
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Quote `s` only if it has characters a shell would interpret.
fn quote_if_needed(s: &str) -> String {
    if !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
    {
        s.to_owned()
    } else {
        quote(s)
    }
}

/// An interim [`Resumer`]: the built-in (or `[ai.sessions.resume]`) command per harness, blocked
/// for sessions from another host or whose working directory is gone.
///
/// It does not check that the harness's own transcript is still on disk; the harnesstools
/// `resume()`/`locate()` implementation replaces this at merge.
pub struct TemplateResumer {
    host_id: String,
    templates: AiSessionResume,
}

impl TemplateResumer {
    pub fn new(host_id: impl Into<String>, templates: AiSessionResume) -> Self {
        Self {
            host_id: host_id.into(),
            templates,
        }
    }

    fn default_template(harness: HarnessKind) -> Option<&'static str> {
        match harness {
            HarnessKind::ClaudeCode => Some("claude --resume {id}"),
            HarnessKind::Codex => Some("codex resume {id}"),
            HarnessKind::Opencode => Some("opencode --session {id}"),
            HarnessKind::Pi => Some("pi --session {id}"),
            HarnessKind::Copilot | HarnessKind::Unknown => None,
        }
    }

    /// Whether the harness finds its sessions by working directory, so resuming needs it.
    fn needs_cwd(harness: HarnessKind) -> bool {
        !matches!(harness, HarnessKind::Codex)
    }
}

impl Resumer for TemplateResumer {
    fn plan(&self, session: &SessionRow) -> ResumePlan {
        let harness = session.handle.harness;
        let template = self.templates.template(harness).or_else(|| Self::default_template(harness));
        let cwd_str = session.cwd.as_deref().map(Path::to_string_lossy).unwrap_or_default();
        let command = template
            .unwrap_or_default()
            .replace("{id}", &quote_if_needed(session.handle.session.as_ref()))
            .replace("{cwd}", &quote(&cwd_str));

        let cwd_exists = session.cwd.as_deref().is_some_and(Path::is_dir);
        let blocked = if template.is_none() {
            Some(format!("{} sessions can't be resumed", harness_label(harness)))
        } else if session.host_id != self.host_id {
            Some(format!("recorded on {}; only viewable here", session.hostname))
        } else if Self::needs_cwd(harness) && !cwd_exists {
            Some(match &session.cwd {
                Some(cwd) => format!("working directory {} no longer exists", cwd.display()),
                None => "no working directory was recorded".to_owned(),
            })
        } else {
            None
        };

        ResumePlan {
            cwd: session.cwd.clone().filter(|_| cwd_exists),
            command,
            blocked,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resume_tui::fake;

    #[test]
    fn shell_line_quotes_the_directory() {
        let plan = ResumePlan {
            cwd: Some(PathBuf::from("/tmp/it's here")),
            command: "claude --resume abc".to_owned(),
            blocked: None,
        };
        assert_eq!(plan.shell_line(), r"cd -- '/tmp/it'\''s here' && claude --resume abc");
    }

    #[test]
    fn template_resumer_blocks_other_hosts_and_missing_directories() {
        let resumer = TemplateResumer::new(fake::THIS_HOST_ID, AiSessionResume {
            codex: Some("codex resume {id} --yolo".to_owned()),
            ..AiSessionResume::default()
        });

        let mut row = fake::row(HarnessKind::ClaudeCode, "abc-123", "t");
        row.cwd = Some(std::env::temp_dir());
        let plan = resumer.plan(&row);
        assert!(plan.blocked.is_none(), "{plan:?}");
        assert_eq!(plan.command, "claude --resume abc-123");

        row.host_id = "other".to_owned();
        row.hostname = "buildbox".to_owned();
        assert_eq!(
            resumer.plan(&row).blocked.as_deref(),
            Some("recorded on buildbox; only viewable here")
        );

        let mut row = fake::row(HarnessKind::Pi, "p1", "t");
        row.cwd = Some(PathBuf::from("/definitely/not/here"));
        assert!(resumer.plan(&row).blocked.unwrap().contains("no longer exists"));

        // Codex looks sessions up by id, so a missing directory doesn't block it.
        let mut row = fake::row(HarnessKind::Codex, "c1", "t");
        row.cwd = Some(PathBuf::from("/definitely/not/here"));
        let plan = resumer.plan(&row);
        assert!(plan.blocked.is_none());
        assert_eq!(plan.shell_line(), "codex resume c1 --yolo");
    }
}
