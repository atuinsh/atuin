//! `[ai.sessions]`: the `atuin ai resume` picker.
//!
//! Only the picker-specific knobs live here. The look and feel (`style`, `invert`, `show_preview`,
//! `max_preview_height`, `enter_accept`, `keymap_mode`, `theme`) is read from the top-level
//! settings, so the picker matches the history search without configuring it twice.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::ai_session::HarnessKind;

/// Which sessions the picker shows, cycled with ctrl-r in this order (see [`Self::CYCLE`]).
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
    /// The order ctrl-r cycles through the modes, wrapping around: from the workspace (where the
    /// picker opens) straight to every session.
    pub const CYCLE: [Self; 5] =
        [Self::Workspace, Self::Global, Self::Host, Self::Directory, Self::Branch];

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
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AiSessions {
    /// The filter the picker opens in. Unset: `workspace`, widening to `global` when not in a git
    /// repository or when the workspace has no sessions.
    pub filter_mode: Option<AiSessionFilterMode>,

    /// Height of the inline picker. Unset: the top-level `inline_height`.
    pub inline_height: Option<u16>,

    /// Take the mouse: the wheel scrolls the preview under it, or moves the selection over the
    /// list. While the picker has the mouse, most terminals still select text with shift held
    /// while dragging (option in iTerm2, fn in macOS Terminal). Unset: as the top-level
    /// `no_mouse` says (on, by default).
    pub mouse: Option<bool>,

    /// Per-harness resume command templates.
    pub resume: AiSessionResume,
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
            inline_height = 20
            mouse = false

            [resume]
            claude = "claude --resume {id} --permission-mode plan"
            pi = "pi --session {id}"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.filter_mode, Some(AiSessionFilterMode::Global));
        assert_eq!(parsed.inline_height, Some(20));
        assert_eq!(parsed.mouse, Some(false));
        assert_eq!(
            parsed.resume.template(HarnessKind::ClaudeCode),
            Some("claude --resume {id} --permission-mode plan")
        );
        assert_eq!(parsed.resume.template(HarnessKind::Codex), None);
        assert_eq!(parsed.resume.template(HarnessKind::Pi), Some("pi --session {id}"));
    }
}
