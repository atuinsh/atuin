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
    /// When the session was last updated: `12m` or `3h` while recent, then a clock time
    /// (`14:02`, `yest 09:40`, `Mon 09:40`, `Sep 27`, `2025-09-27`).
    Time,
    /// The harness badge: CC, CX, OC or PI.
    Harness,
    /// The session title (falls back to one derived from the first prompt). Expands to fill
    /// the row.
    Title,
    /// The message count, left out when the title would be short of room. (The repository,
    /// branch and host are in the preview.)
    Messages,
}

impl AiSessionColumn {
    /// Default width in cells. The title expands instead.
    #[must_use]
    pub fn width(&self) -> u16 {
        match self {
            Self::Time => 10,
            Self::Harness => 2,
            Self::Title => 0,
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

    /// The filter modes ctrl-r cycles through, in order, wrapping around. Modes that need a git
    /// repository are skipped outside one. The default goes from the workspace (where the
    /// picker opens) straight to every session.
    pub filters: Vec<AiSessionFilterMode>,

    /// Row columns, left to right.
    pub columns: Vec<AiSessionColumn>,

    /// Group forks (including Claude Code `--resume` forks and continuations in another harness)
    /// under their root session, so only the root shows as a row; a fork's match finds its root.
    /// When false, every fork gets a row of its own. Subagents never get a row either way: they
    /// can't be resumed, so the picker leaves them out (grouped, a match in one still finds its
    /// root).
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
                AiSessionFilterMode::Workspace,
                AiSessionFilterMode::Global,
                AiSessionFilterMode::Host,
                AiSessionFilterMode::Directory,
                AiSessionFilterMode::Branch,
            ],
            columns: vec![
                AiSessionColumn::Time,
                AiSessionColumn::Harness,
                AiSessionColumn::Title,
                AiSessionColumn::Messages,
            ],
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
