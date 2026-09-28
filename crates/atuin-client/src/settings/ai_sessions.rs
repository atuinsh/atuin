//! `[ai.sessions]`: the `atuin ai resume` picker.
//!
//! Only the picker-specific knobs live here. The look and feel (`style`, `invert`, `show_preview`,
//! `max_preview_height`, `enter_accept`, `keymap_mode`, `theme`) is read from the top-level
//! settings, so the picker matches the history search without configuring it twice.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::ai_session::HarnessKind;

/// Which sessions the picker shows, cycled with ctrl-r.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum AiSessionFilterMode {
    /// Every session, from every host.
    Global,
    /// Sessions recorded on this host.
    Host,
    /// Sessions whose working directory is inside the current git repository.
    Workspace,
    /// Sessions started in the current directory.
    Directory,
    /// Sessions on the current git branch of the current repository.
    Branch,
}

impl AiSessionFilterMode {
    /// The label shown in the `[ MODE ] >` input prefix.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "GLOBAL",
            Self::Host => "HOST",
            Self::Workspace => "WORKSPACE",
            Self::Directory => "DIRECTORY",
            Self::Branch => "BRANCH",
        }
    }

    /// Whether this mode needs the current directory to be inside a git repository.
    #[must_use]
    pub fn needs_git_root(&self) -> bool {
        matches!(self, Self::Workspace | Self::Branch)
    }
}

/// A column in the picker's session rows, left to right after the selection indicator.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiSessionColumn {
    /// Relative time since the session was last updated (e.g. "5m ago").
    Time,
    /// The harness badge: CC, CX, OC or PI.
    Harness,
    /// `+N` when forks or subagents are grouped under the session.
    Children,
    /// The session title (falls back to the first prompt). Expands to fill the row.
    Title,
    /// The repository (or directory) name the session ran in.
    Repo,
    /// The git branch the session ran on.
    Branch,
    /// The message count.
    Messages,
}

impl AiSessionColumn {
    /// Default width in cells. The title expands instead.
    #[must_use]
    pub fn width(&self) -> u16 {
        match self {
            Self::Time => 8,
            Self::Harness => 2,
            Self::Children => 3,
            Self::Title => 0,
            Self::Repo => 12,
            Self::Branch => 12,
            Self::Messages => 4,
        }
    }

    /// Whether the column expands to fill the remaining width.
    #[must_use]
    pub fn expands(&self) -> bool {
        matches!(self, Self::Title)
    }
}

/// Per-harness resume command templates. Unset harnesses use the built-in command.
///
/// A template replaces the harness's program and arguments; the harness still decides whether a
/// session can be resumed and from which directory (the command runs after `cd -- <cwd> &&`). It
/// is split into words like a shell command line, then these placeholders are substituted inside
/// their word (so never quote them):
/// - `{id}`: the harness's native session id;
/// - `{path}`: the session's native transcript (or database) on this machine;
/// - `{cwd}`: the session's working directory.
///
/// The substituted value stays one argument of the harness command, but it is not quoted for a
/// shell, so never hand a placeholder to another shell (`sh -c "cd {cwd} && …"`): that shell
/// would split and expand it.
///
/// For example: `claude = "claude --resume {id} --permission-mode plan"`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AiSessionResume {
    #[serde(alias = "claude-code", alias = "claude_code")]
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub opencode: Option<String>,
    pub pi: Option<String>,
}

impl AiSessionResume {
    /// The configured template for `harness`, if any.
    #[must_use]
    pub fn template(&self, harness: HarnessKind) -> Option<&str> {
        match harness {
            HarnessKind::ClaudeCode => self.claude.as_deref(),
            HarnessKind::Codex => self.codex.as_deref(),
            HarnessKind::Opencode => self.opencode.as_deref(),
            HarnessKind::Pi => self.pi.as_deref(),
            HarnessKind::Copilot | HarnessKind::Unknown => None,
        }
    }
}

/// `[ai.sessions]`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AiSessions {
    /// The filter the picker opens in. Unset: `workspace`, widening to `global` when not in a git
    /// repository or when the workspace has no sessions.
    pub filter_mode: Option<AiSessionFilterMode>,

    /// The filter modes ctrl-r cycles through, in order. Modes that need a git repository are
    /// skipped outside one.
    pub filters: Vec<AiSessionFilterMode>,

    /// Row columns, left to right.
    pub columns: Vec<AiSessionColumn>,

    /// List subagent sessions among a root session's children in the Inspect tab. (They always
    /// count toward the root's `+N`.)
    pub show_subagents: bool,

    /// Group forks (including Claude Code `--resume` forks) and subagents under their root
    /// session, so only the root shows as a row, with `+N`; a child's match finds its root. When
    /// false, every session gets a row of its own.
    pub group_forks: bool,

    /// Height of the inline picker. Unset: the top-level `inline_height`.
    pub inline_height: Option<u16>,

    /// Ask where to resume a session once it's chosen (enter or tab): in its own harness, or
    /// continued in another one installed here. When false, a session resumes in its own harness
    /// straight away; the chooser still opens for one that can't.
    pub resume_chooser: bool,

    /// Per-harness resume command templates.
    pub resume: AiSessionResume,
}

impl Default for AiSessions {
    fn default() -> Self {
        Self {
            filter_mode: None,
            filters: vec![
                AiSessionFilterMode::Global,
                AiSessionFilterMode::Host,
                AiSessionFilterMode::Workspace,
                AiSessionFilterMode::Directory,
                AiSessionFilterMode::Branch,
            ],
            columns: vec![
                AiSessionColumn::Time,
                AiSessionColumn::Harness,
                AiSessionColumn::Children,
                AiSessionColumn::Title,
                AiSessionColumn::Repo,
                AiSessionColumn::Branch,
                AiSessionColumn::Messages,
            ],
            show_subagents: true,
            group_forks: true,
            inline_height: None,
            resume_chooser: true,
            resume: AiSessionResume::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn empty_table_uses_defaults() {
        let parsed: AiSessions = toml::from_str("").unwrap();
        assert_eq!(parsed, AiSessions::default());
    }

    #[rstest]
    fn parses_every_field() {
        let parsed: AiSessions = toml::from_str(
            r#"
            filter_mode = "global"
            filters = ["workspace", "global"]
            columns = ["harness", "title"]
            show_subagents = false
            group_forks = false
            inline_height = 20
            resume_chooser = false

            [resume]
            claude = "claude --resume {id} --permission-mode plan"
            pi = "pi --session {id}"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.filter_mode, Some(AiSessionFilterMode::Global));
        assert_eq!(parsed.filters, vec![
            AiSessionFilterMode::Workspace,
            AiSessionFilterMode::Global
        ]);
        assert_eq!(parsed.columns, vec![AiSessionColumn::Harness, AiSessionColumn::Title]);
        assert!(!parsed.show_subagents);
        assert!(!parsed.group_forks);
        assert_eq!(parsed.inline_height, Some(20));
        assert!(!parsed.resume_chooser);
        assert_eq!(
            parsed.resume.template(HarnessKind::ClaudeCode),
            Some("claude --resume {id} --permission-mode plan")
        );
        assert_eq!(parsed.resume.template(HarnessKind::Codex), None);
        assert_eq!(parsed.resume.template(HarnessKind::Pi), Some("pi --session {id}"));
    }
}
