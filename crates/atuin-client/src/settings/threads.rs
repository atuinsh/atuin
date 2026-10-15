//! `[threads]`: capturing coding agents' threads, and the `atuin threads resume` picker.
//!
//! Before `[threads]`, these were `ai.capture_sessions`, `ai.capture_tools` and `[ai.sessions]`,
//! which are still read for any it doesn't set (see `Settings::with_legacy_threads`).
//!
//! Only the picker-specific knobs live here. The look and feel (`style`, `invert`, `show_preview`,
//! `max_preview_height`, `enter_accept`, `keymap_mode`, `theme`) is read from the top-level
//! settings, so the picker matches the history search without configuring it twice.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::ai_session::HarnessKind;

/// Which threads the picker shows, cycled with ctrl-r in this order (see [`Self::CYCLE`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum AiSessionFilterMode {
    /// Every thread, from every host.
    Global,
    /// Threads recorded on this host.
    Host,
    /// Threads whose working directory is inside the current git repository.
    Workspace,
    /// Threads started in the current directory.
    Directory,
    /// Threads on the current git branch of the current repository.
    Branch,
}

impl AiSessionFilterMode {
    /// The order ctrl-r cycles through the modes, wrapping around: from every thread (where the
    /// picker opens) narrowing to the workspace, its branch and the directory, then this host's.
    /// The picker lists them in this order.
    pub const CYCLE: [Self; 5] =
        [Self::Global, Self::Workspace, Self::Branch, Self::Directory, Self::Host];

    /// The mode's name in upper case, as the ultracompact picker abbreviates it.
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
/// thread can be resumed and from which directory (the command runs after `cd -- <cwd> &&`). It
/// is split into words like a shell command line, then these placeholders are substituted inside
/// their word (so never quote them):
/// - `{id}`: the harness's native session id;
/// - `{path}`: the thread's native transcript (or database) on this machine;
/// - `{cwd}`: the thread's working directory.
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

/// `[threads]`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Threads {
    /// Whether the daemon captures live coding-agent threads (Claude Code, Codex, ...) into the
    /// synced record store. Off by default: capture copies the conversation (what was said, and
    /// the tools called, see [`Self::capture_tools`]) into the encrypted store used by sync, so
    /// it is strictly opt-in.
    pub capture: bool,

    /// With [`Self::capture`], also capture what each tool call was given and what it returned
    /// (commands and their output, files read and written), so a resumed thread has them too. On
    /// by default, with secrets redacted and each payload held to 64 KiB; off keeps only each
    /// tool's name.
    pub capture_tools: bool,

    /// The filter the picker opens in. Unset: `global`. A mode that can't apply here opens in
    /// `workspace` inside a git repository (`branch` on a detached `HEAD`), else in `global`; and
    /// `workspace` opens in `global` when the workspace has no threads at all.
    pub filter_mode: Option<AiSessionFilterMode>,

    /// Height of the inline picker. Unset: the top-level `inline_height`.
    pub inline_height: Option<u16>,

    /// Ask where to resume a thread once it's chosen (enter or tab): in its own harness, or
    /// continued in another one installed here. When false, a thread resumes in its own harness
    /// straight away; the chooser still opens for one that can't.
    pub resume_chooser: bool,

    /// Take the mouse: the wheel scrolls the preview under it, or moves the selection over the
    /// list. While the picker has the mouse, most terminals still select text with shift held
    /// while dragging (option in iTerm2, fn in macOS Terminal). Unset: as the top-level
    /// `no_mouse` says (on, by default).
    pub mouse: Option<bool>,

    /// Per-harness resume command templates.
    pub resume: AiSessionResume,
}

impl Default for Threads {
    fn default() -> Self {
        Self {
            capture: false,
            capture_tools: true,
            filter_mode: None,
            inline_height: None,
            resume_chooser: true,
            mouse: None,
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
        let parsed: Threads = toml::from_str("").unwrap();
        assert_eq!(parsed, Threads::default());
    }

    #[rstest]
    fn parses_every_field() {
        let parsed: Threads = toml::from_str(
            r#"
            capture = true
            capture_tools = false
            filter_mode = "global"
            inline_height = 20
            resume_chooser = false
            mouse = false

            [resume]
            claude = "claude --resume {id} --permission-mode plan"
            pi = "pi --session {id}"
            "#,
        )
        .unwrap();
        assert!(parsed.capture);
        assert!(!parsed.capture_tools);
        assert_eq!(parsed.filter_mode, Some(AiSessionFilterMode::Global));
        assert_eq!(parsed.inline_height, Some(20));
        assert!(!parsed.resume_chooser);
        assert_eq!(parsed.mouse, Some(false));
        assert_eq!(
            parsed.resume.template(HarnessKind::ClaudeCode),
            Some("claude --resume {id} --permission-mode plan")
        );
        assert_eq!(parsed.resume.template(HarnessKind::Codex), None);
        assert_eq!(parsed.resume.template(HarnessKind::Pi), Some("pi --session {id}"));
    }
}
