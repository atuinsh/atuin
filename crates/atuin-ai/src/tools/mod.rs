use std::io::BufRead;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use atuin_client::history::{AuthorPattern, HistoryId};
use atuin_client::settings::FilterMode;
use atuin_common::ansi;
use atuin_common::filter::OrFilter;
use atuin_common::range::PyStyleIdxRange;
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::grpc::history::pb::ChunkedOutputLineView;
use easy_cast::Conv;
use enum_dispatch::enum_dispatch;
use eyre::Result;
use strum_macros::{EnumIter, EnumString, IntoStaticStr};

const DEFAULT_FILE_READ_LINES: u64 = 100;
const MAX_FILE_READ_LINES: u64 = 1000;
/// Page-size bounds for `atuin_history`; mirrored in the MCP schema.
pub const DEFAULT_HISTORY_RESULTS: i64 = 10;
pub const MAX_HISTORY_RESULTS: i64 = 50;
/// Advice appended to `atuin_output` failures. Hedged on safety: telling the
/// model to re-run unconditionally would invite re-executing destructive
/// commands — the very thing output capture exists to avoid.
const NO_OUTPUT_ADVICE: &str = "If the command is safe and cheap to repeat, re-run it to see its \
                                output; otherwise rely on history metadata (exit code, duration) \
                                instead.";

pub mod descriptor;

use crate::permissions::rule::Rule;

/// Check whether a file path matches a scope glob pattern.
///
/// Resolves relative paths against the current directory before matching so
/// that `./foo.md` and `/cwd/foo.md` match the same glob. Supports `*`, `**`,
/// `?`, and `[...]` via `glob_match`.
fn path_matches_scope(path: &Path, scope: &str) -> bool {
    let path = if path.is_relative() {
        std::env::current_dir().map(|cwd| cwd.join(path)).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    // Normalize to forward slashes so globs work on Windows too.
    let path_str = path.to_string_lossy().replace('\\', "/");

    // If the scope is also relative, try matching against both the absolute
    // path and just the filename/relative portion.
    if !scope.starts_with('/') {
        // Match against filename (e.g. "*.md" matches any .md file)
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && glob_match::glob_match(scope, name)
        {
            return true;
        }
        // Also try matching against the full absolute path in case the scope
        // is a relative multi-segment pattern like "crates/**/*.rs"
        if glob_match::glob_match(scope, &path_str) {
            return true;
        }
        // And match relative to cwd (so "src/*.rs" works from project root)
        if let Ok(cwd) = std::env::current_dir()
            && let Ok(rel) = path.strip_prefix(&cwd)
        {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            return glob_match::glob_match(scope, &rel_str);
        }
        return false;
    }

    // Absolute scope — match against absolute path
    glob_match::glob_match(scope, &path_str)
}

/// Result of executing a client-side tool.
#[derive(Debug, Clone)]
pub enum ToolOutcome {
    /// Simple success with a text result (used by Read, AtuinHistory).
    Success(String),
    /// Error with a message.
    Error(String),
    /// Structured shell result with separated stdout, stderr, exit code, and duration.
    Structured {
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        duration_ms: u64,
        interrupted: bool,
    },
}

impl ToolOutcome {
    /// Format this outcome as a string for the tool result sent to the LLM.
    ///
    /// The optional `interrupt_reason` overrides the generic interrupted message
    /// with a specific one (user interrupt vs timeout).
    pub fn format_for_llm(
        &self,
        interrupt_reason: Option<&crate::fsm::tools::InterruptReason>,
    ) -> String {
        match self {
            Self::Success(s) => s.clone(),
            Self::Error(e) => e.clone(),
            Self::Structured {
                stdout,
                stderr,
                exit_code,
                duration_ms,
                interrupted,
            } => {
                let mut parts = Vec::new();

                if let Some(code) = exit_code {
                    parts.push(format!("Exit code: {code}"));
                }

                parts.push(format!("Duration: {duration_ms}ms"));

                if stdout.is_empty() {
                    parts.push("stdout: (empty)".to_string());
                } else {
                    parts.push(format!("stdout:\n{stdout}"));
                }

                if stderr.is_empty() {
                    parts.push("stderr: (empty)".to_string());
                } else {
                    parts.push(format!("stderr:\n{stderr}"));
                }

                if *interrupted {
                    use crate::fsm::tools::InterruptReason;
                    let msg = match interrupt_reason {
                        Some(InterruptReason::Timeout(secs)) => {
                            format!("[Timed out after {secs}s]")
                        }
                        _ => "[Interrupted by user]".to_string(),
                    };
                    parts.push(msg);
                }

                parts.join("\n\n")
            }
        }
    }

    /// Whether this outcome represents an error.
    pub fn is_error(&self) -> bool {
        match self {
            Self::Error(_) => true,
            Self::Structured {
                exit_code: Some(code),
                ..
            } if *code != 0 => true,
            _ => false,
        }
    }
}

/// Cached VT100 preview data for a shell tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPreview {
    pub lines: Vec<String>,
    pub exit_code: Option<i32>,
    pub interrupted: Option<crate::fsm::tools::InterruptReason>,
}

/// A tool call from the server, with parsed input parameters.
#[derive(Debug, Clone)]
#[enum_dispatch(PermissibleToolCall)]
pub enum ClientToolCall {
    Read(ReadToolCall),
    Edit(EditToolCall),
    Write(WriteToolCall),
    Shell(ShellToolCall),
    AtuinHistory(AtuinHistoryToolCall),
    AtuinOutput(AtuinOutputToolCall),
    LoadSkill(LoadSkillToolCall),
}

impl TryFrom<(&str, &serde_json::Value)> for ClientToolCall {
    type Error = eyre::Error;

    fn try_from((name, input): (&str, &serde_json::Value)) -> Result<Self, Self::Error> {
        match name {
            "read_file" => Ok(Self::Read(ReadToolCall::try_from(input)?)),
            "edit_file" => Ok(Self::Edit(EditToolCall::try_from(input)?)),
            "write_file" => Ok(Self::Write(WriteToolCall::try_from(input)?)),
            "execute_shell_command" => Ok(Self::Shell(ShellToolCall::try_from(input)?)),
            "atuin_history" => Ok(Self::AtuinHistory(AtuinHistoryToolCall::try_from(input)?)),
            "atuin_output" => Ok(Self::AtuinOutput(AtuinOutputToolCall::try_from(input)?)),
            "load_skill" => Ok(Self::LoadSkill(LoadSkillToolCall::try_from(input)?)),
            _ => Err(eyre::eyre!("Unknown tool call: {name}")),
        }
    }
}

impl ClientToolCall {
    pub(crate) fn descriptor(&self) -> &'static descriptor::ToolDescriptor {
        match self {
            Self::Read(_) => descriptor::READ,
            Self::Edit(_) => descriptor::EDIT,
            Self::Write(_) => descriptor::WRITE,
            Self::Shell(_) => descriptor::SHELL,
            Self::AtuinHistory(_) => descriptor::ATUIN_HISTORY,
            Self::AtuinOutput(_) => descriptor::ATUIN_OUTPUT,
            Self::LoadSkill(_) => descriptor::LOAD_SKILL,
        }
    }

    /// The permission rule name for this tool category.
    ///
    /// Edit and Write share the `"Write"` rule name — a Write permission
    /// covers both str_replace edits and full file creates. Write also
    /// implies Read (checked in `ReadToolCall::matches_rule`).
    pub(crate) fn rule_name(&self) -> &'static str {
        match self {
            Self::Read(_) => "Read",
            Self::Edit(_) => "Write",
            Self::Write(_) => "Write",
            Self::Shell(_) => "Shell",
            Self::AtuinHistory(_) => "AtuinHistory",
            Self::AtuinOutput(_) => "AtuinOutput",
            Self::LoadSkill(_) => "LoadSkill",
        }
    }

    /// The resolved file path for this tool call, if it's a file-based tool.
    /// Used to build scoped permission rules like `Write(/abs/path/to/file)`.
    pub(crate) fn resolved_file_path(&self) -> Option<PathBuf> {
        match self {
            Self::Read(tool) => Some(tool.resolved_path()),
            Self::Edit(tool) => Some(tool.resolved_path()),
            Self::Write(tool) => Some(tool.resolved_path()),
            Self::Shell(_) | Self::AtuinHistory(_) | Self::AtuinOutput(_) | Self::LoadSkill(_) => {
                None
            }
        }
    }
}

/// A trait for tool calls that can be checked against permission rules.
#[enum_dispatch]
pub trait PermissibleToolCall {
    /// Checks if this tool call matches the given permission rule.
    fn matches_rule(&self, rule: &Rule) -> bool;

    /// Check if every part of this tool call is covered by at least one rule in
    /// the set.  For compound operations (e.g. shell pipelines), each sub-part
    /// must be individually covered.  The default treats the call as atomic —
    /// any single matching rule is sufficient.
    fn all_covered_by(&self, rules: &[Rule]) -> bool {
        rules.iter().any(|r| self.matches_rule(r))
    }

    /// Returns the target directory of this tool call, if applicable, for checking against directory-based rules.
    fn target_dir(&self) -> Option<&Path> {
        None
    }
}

/// Returns true if this tool call should bypass the permission system entirely.
impl ClientToolCall {
    pub(crate) fn is_auto_approved(&self) -> bool {
        matches!(self, Self::LoadSkill(_))
    }
}

/// Expand shell constructs (`~`, `$HOME`, etc.) in a path string.
///
/// Tool call paths arrive as raw strings from the API without shell
/// expansion. Uses `shellexpand` (same as `atuin-client`).
fn expand_path(path: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(path).into_owned())
}

#[derive(Debug, Clone)]
pub struct ReadToolCall {
    pub path: PathBuf,
    pub offset: u64,
    pub limit: u64,
}

impl TryFrom<&serde_json::Value> for ReadToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let path =
            value.get("file_path").and_then(|v| v.as_str()).ok_or(eyre::eyre!("Missing path"))?;

        let offset = value.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
        let limit = value
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_FILE_READ_LINES)
            .min(MAX_FILE_READ_LINES);

        Ok(Self {
            path: expand_path(path),
            offset,
            limit,
        })
    }
}

impl ReadToolCall {
    pub fn resolved_path(&self) -> PathBuf {
        if self.path.is_relative() {
            std::env::current_dir()
                .map(|cwd| cwd.join(&self.path))
                .unwrap_or_else(|_| self.path.clone())
        } else {
            self.path.clone()
        }
    }

    #[must_use]
    pub fn execute(&self) -> ToolOutcome {
        let path = self.resolved_path();

        if !path.exists() {
            return ToolOutcome::Error(format!("Error: file does not exist: {}", path.display()));
        }

        if path.is_dir() {
            let Some(files) = std::fs::read_dir(&path).ok().and_then(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.file_name().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .into()
            }) else {
                return ToolOutcome::Error(format!(
                    "Error: could not read directory: {}",
                    path.display()
                ));
            };

            return ToolOutcome::Success(format!("Directory contents:\n{}", files.join("\n")));
        }

        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) => return ToolOutcome::Error(format!("Error opening file: {e}")),
        };
        let reader = std::io::BufReader::new(file);

        let raw_lines = reader
            .lines()
            .skip(usize::conv(self.offset))
            .take(usize::conv(self.limit))
            .collect::<Result<Vec<_>, _>>();

        match raw_lines {
            Ok(lines) => {
                let first_line_no = usize::conv(self.offset) + 1;
                let last_line_no = first_line_no + lines.len().saturating_sub(1);
                let width = usize::conv(last_line_no.max(1).ilog10()) + 1;

                let numbered: String = lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:>width$}\t{line}", first_line_no + i))
                    .collect::<Vec<_>>()
                    .join("\n");

                if numbered.len() > 100_000 {
                    ToolOutcome::Error(format!(
                        "Error: file is too large to read ({} bytes in {} lines); use view_range \
                         to read a subset of the file",
                        numbered.len(),
                        lines.len()
                    ))
                } else {
                    ToolOutcome::Success(numbered)
                }
            }
            Err(e) => ToolOutcome::Error(format!("Error reading file: {e}")),
        }
    }
}

impl PermissibleToolCall for ReadToolCall {
    fn target_dir(&self) -> Option<&Path> {
        Some(&self.path)
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        // Write implies Read — a Write permission on a path also permits reading it.
        if rule.tool != "Read" && rule.tool != "Write" {
            return false;
        }

        match rule.scope.as_deref() {
            None | Some("*") => true,
            Some(scope) => path_matches_scope(&self.path, scope),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EditToolCall {
    pub path: PathBuf,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}

impl TryFrom<&serde_json::Value> for EditToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let path = value
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing file_path"))?;

        let old_string = value
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing old_string"))?;

        let new_string = value
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing new_string"))?;

        let replace_all = value.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);

        Ok(Self {
            path: expand_path(path),
            old_string: old_string.to_string(),
            new_string: new_string.to_string(),
            replace_all,
        })
    }
}

impl EditToolCall {
    /// Resolve the edit path to an absolute path.
    pub fn resolved_path(&self) -> PathBuf {
        if self.path.is_relative() {
            std::env::current_dir()
                .map(|cwd| cwd.join(&self.path))
                .unwrap_or_else(|_| self.path.clone())
        } else {
            self.path.clone()
        }
    }

    /// Execute the edit against the filesystem.
    ///
    /// Checks freshness via the provided tracker, validates matches, applies
    /// the replacement, and writes atomically. Returns the outcome and (on
    /// success) the new file content bytes for tracker updates.
    ///
    /// Callers should snapshot the file before calling this method and
    /// update the file tracker after a successful return.
    #[must_use]
    pub fn execute(
        &self,
        resolved_path: &Path,
        file_tracker: &crate::file_tracker::FileReadTracker,
    ) -> (ToolOutcome, Option<Vec<u8>>) {
        use crate::file_tracker::FreshnessCheck;

        // 1. Basic validation
        if !resolved_path.exists() {
            return (
                ToolOutcome::Error(format!(
                    "Error: file does not exist: {}",
                    resolved_path.display()
                )),
                None,
            );
        }
        if resolved_path.is_dir() {
            return (
                ToolOutcome::Error(format!(
                    "Error: path is a directory, not a file: {}",
                    resolved_path.display()
                )),
                None,
            );
        }
        if self.old_string.is_empty() {
            return (
                ToolOutcome::Error(
                    "old_string must not be empty. To create a new file, use write_file instead."
                        .to_string(),
                ),
                None,
            );
        }

        // 2. Freshness check
        match file_tracker.check_freshness(resolved_path) {
            Ok(FreshnessCheck::NotRead) => {
                return (
                    ToolOutcome::Error(
                        "File has not been read yet. Read it first before editing.".to_string(),
                    ),
                    None,
                );
            }
            Ok(FreshnessCheck::Stale) => {
                return (
                    ToolOutcome::Error(
                        "File has been modified since read, either by the user or by a linter. \
                         Read it again before attempting to edit it."
                            .to_string(),
                    ),
                    None,
                );
            }
            Err(e) => {
                return (ToolOutcome::Error(format!("Error checking file state: {e}")), None);
            }
            Ok(FreshnessCheck::Fresh) => {}
        }

        // 3. Read current contents
        let content = match std::fs::read_to_string(resolved_path) {
            Ok(c) => c,
            Err(e) => return (ToolOutcome::Error(format!("Error reading file: {e}")), None),
        };

        // 4. Find and validate matches
        let match_count = content.matches(&self.old_string).count();

        if match_count == 0 {
            return (
                ToolOutcome::Error(format!(
                    "old_string not found in {}. Make sure it matches exactly, including \
                     whitespace and indentation.",
                    resolved_path.display()
                )),
                None,
            );
        }

        if match_count > 1 && !self.replace_all {
            return (
                ToolOutcome::Error(format!(
                    "Found {match_count} matches of old_string in {}, but replace_all is false. \
                     Either provide more context to make the match unique, or set replace_all to \
                     true.",
                    resolved_path.display()
                )),
                None,
            );
        }

        // 5. Apply replacement
        let new_content = if self.replace_all {
            content.replace(&self.old_string, &self.new_string)
        } else {
            content.replacen(&self.old_string, &self.new_string, 1)
        };

        // 6. Write atomically
        let new_bytes = new_content.into_bytes();
        if let Err(e) = crate::snapshots::atomic_write_file(resolved_path, &new_bytes) {
            return (ToolOutcome::Error(format!("Error writing file: {e}")), None);
        }

        // 7. Success
        let verb = if match_count == 1 {
            "occurrence"
        } else {
            "occurrences"
        };
        (
            ToolOutcome::Success(format!(
                "Edited {}: replaced {match_count} {verb} of old_string with new_string.",
                resolved_path.display()
            )),
            Some(new_bytes),
        )
    }
}

impl PermissibleToolCall for EditToolCall {
    fn target_dir(&self) -> Option<&Path> {
        Some(&self.path)
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        if rule.tool != "Write" {
            return false;
        }

        match rule.scope.as_deref() {
            None | Some("*") => true,
            Some(scope) => path_matches_scope(&self.path, scope),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WriteToolCall {
    pub path: PathBuf,
    pub content: String,
    pub overwrite: bool,
}

impl TryFrom<&serde_json::Value> for WriteToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let path = value
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing file_path"))?;

        let content =
            value.get("content").and_then(|v| v.as_str()).ok_or(eyre::eyre!("Missing content"))?;

        let overwrite = value.get("overwrite").and_then(|v| v.as_bool()).unwrap_or(false);

        Ok(Self {
            path: expand_path(path),
            content: content.to_string(),
            overwrite,
        })
    }
}

impl WriteToolCall {
    /// Resolve the write path to an absolute path.
    pub fn resolved_path(&self) -> PathBuf {
        if self.path.is_relative() {
            std::env::current_dir()
                .map(|cwd| cwd.join(&self.path))
                .unwrap_or_else(|_| self.path.clone())
        } else {
            self.path.clone()
        }
    }

    /// Execute the write operation.
    ///
    /// Creates a new file or overwrites an existing one (if `overwrite` is set).
    /// Returns the outcome and the written bytes (for tracker updates).
    #[must_use]
    pub fn execute(&self, resolved_path: &Path) -> (ToolOutcome, Option<Vec<u8>>) {
        if resolved_path.is_dir() {
            return (
                ToolOutcome::Error(format!(
                    "Error: path is a directory, not a file: {}",
                    resolved_path.display()
                )),
                None,
            );
        }
        if resolved_path.exists() && !self.overwrite {
            return (
                ToolOutcome::Error(format!(
                    "File already exists: {}. Set overwrite to true to replace it, or use \
                     edit_file to make targeted changes.",
                    resolved_path.display()
                )),
                None,
            );
        }

        // Capture before the write — after atomic_write the file always exists.
        let existed = resolved_path.exists();

        // Write atomically
        let content_bytes = self.content.as_bytes().to_vec();
        if let Err(e) = crate::snapshots::atomic_write_file(resolved_path, &content_bytes) {
            return (ToolOutcome::Error(format!("Error writing file: {e}")), None);
        }

        let line_count = self.content.lines().count();
        let verb = if existed {
            "Overwrote"
        } else {
            "Created"
        };
        (
            ToolOutcome::Success(format!(
                "{verb} {} ({line_count} lines).",
                resolved_path.display()
            )),
            Some(content_bytes),
        )
    }
}

impl PermissibleToolCall for WriteToolCall {
    fn target_dir(&self) -> Option<&Path> {
        Some(&self.path)
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        if rule.tool != "Write" {
            return false;
        }

        match rule.scope.as_deref() {
            None | Some("*") => true,
            Some(scope) => path_matches_scope(&self.path, scope),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShellToolCall {
    pub dir: Option<PathBuf>,
    pub command: String,
    pub shell: String,
    /// Maximum execution time in seconds (from LLM). Clamped to 1..=600, default 30.
    pub timeout_secs: u64,
    // allow dead code here; this will be tied into o11y and user-facing descriptions
    #[expect(dead_code)]
    pub description: Option<String>,
}

impl TryFrom<&serde_json::Value> for ShellToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let dir = value.get("dir").and_then(|v| v.as_str());

        let command =
            value.get("command").and_then(|v| v.as_str()).ok_or(eyre::eyre!("Missing command"))?;

        let shell = value.get("shell").and_then(|v| v.as_str()).unwrap_or("bash").to_string();

        let timeout_secs =
            value.get("timeout").and_then(|v| v.as_u64()).filter(|&v| v > 0).unwrap_or(30).min(600);

        let description = value.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());

        Ok(Self {
            dir: dir.map(expand_path),
            command: command.to_string(),
            shell,
            timeout_secs,
            description,
        })
    }
}

impl PermissibleToolCall for ShellToolCall {
    fn target_dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        if rule.tool != "Shell" {
            return false;
        }

        let Some(scope) = rule.scope.as_ref() else {
            // Shell without scope matches all shell commands
            return true;
        };

        let shell_kind = crate::permissions::shell::ShellKind::from_shell_name(&self.shell);
        let parsed = crate::permissions::shell::parse_shell_command(&self.command, shell_kind);
        // Deny/ask path: prefix_bare = true so `deny = ["Shell(rm)"]` blocks `rm -rf /`
        crate::permissions::shell::any_subcommand_matches(&parsed.subcommands, true, scope)
    }

    /// For compound shell commands, every subcommand must be individually
    /// covered by at least one rule.  This ensures that `allow = ["Shell(git *)"]`
    /// does not silently permit `git add . && rm -rf /`.
    fn all_covered_by(&self, rules: &[Rule]) -> bool {
        use crate::permissions::shell;

        let shell_kind = shell::ShellKind::from_shell_name(&self.shell);
        let parsed = shell::parse_shell_command(&self.command, shell_kind);

        // If parsing yields nothing, don't vacuously allow — fall through to ask.
        !parsed.subcommands.is_empty()
            && parsed.subcommands.iter().all(|subcmd| {
                rules.iter().any(|rule| {
                    if rule.tool != "Shell" {
                        return false;
                    }
                    match rule.scope.as_deref() {
                        None | Some("*") => true,
                        // Allow path: prefix_bare = false so `Shell(git commit)`
                        // only allows exactly `git commit`, not `git commit --amend`
                        Some(scope) => shell::any_subcommand_matches(
                            std::slice::from_ref(subcmd),
                            false,
                            scope,
                        ),
                    }
                })
            })
    }
}

/// Preview viewport height for VT100 emulation.
const PREVIEW_HEIGHT: NonZeroU16 = NonZeroU16::new(10).unwrap();

/// Default terminal width for VT100 emulation.
const PREVIEW_WIDTH: NonZeroU16 = NonZeroU16::new(120).unwrap();

/// Extract plain text lines from a VT100 screen buffer.
///
/// Strips trailing blank lines so the result only contains rows with actual
/// content. Without this, the fixed-size VT100 screen (PREVIEW_HEIGHT rows)
/// would always return that many lines, and downstream components that use
/// tail-mode display (like the Viewport) would show the blank padding rows
/// instead of the real output.
fn vt100_screen_lines(screen: &vt100::Screen) -> Vec<String> {
    let (rows, cols) = screen.size();
    let rows = rows.get();
    let cols = cols.get();

    let mut lines = Vec::with_capacity(usize::conv(rows));
    for row in 0..rows {
        let mut line = String::with_capacity(usize::conv(cols));
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                line.push_str(cell.contents());
            }
        }
        lines.push(line.trim_end().to_string());
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// Execute a shell command with VT100 emulation and streaming output.
///
/// Feeds stdout+stderr into a `vt100::Parser` so that ANSI escape sequences,
/// progress bars (`\r`), and cursor movement are handled correctly. Periodically
/// sends the current screen state as `Vec<String>` through `output_tx` for the
/// live preview.
///
/// Captures the FULL stdout and stderr separately for the tool result sent to the LLM.
/// Returns a `ToolOutcome::Structured` with full output, exit code, and duration.
pub async fn execute_shell_command_streaming(
    shell_call: &ShellToolCall,
    output_tx: tokio::sync::mpsc::Sender<Vec<String>>,
    mut interrupt_rx: tokio::sync::oneshot::Receiver<()>,
) -> ToolOutcome {
    use tokio::io::AsyncReadExt;

    let start = std::time::Instant::now();

    // TODO: check if this is proper for all shells we support
    let mut cmd = tokio::process::Command::new(&shell_call.shell);
    cmd.arg("-c").arg(&shell_call.command);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    if let Some(ref dir) = shell_call.dir {
        cmd.current_dir(dir);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return ToolOutcome::Error(format!("Failed to spawn command: {e}")),
    };

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // VT100 emulator for the live preview (viewport-sized)
    let mut parser = vt100::Parser::new(PREVIEW_HEIGHT, PREVIEW_WIDTH, 0);

    let mut stdout_reader = tokio::io::BufReader::new(stdout);
    let mut stderr_reader = tokio::io::BufReader::new(stderr);

    let mut stdout_buf = [0u8; 4096];
    let mut stderr_buf = [0u8; 4096];
    let mut stdout_done = false;
    let mut stderr_done = false;

    // Full output buffers (for the LLM, not the preview)
    let mut full_stdout = Vec::<u8>::new();
    let mut full_stderr = Vec::<u8>::new();

    let mut interval = tokio::time::interval(Duration::from_millis(50));

    // Send initial empty screen
    let initial_lines = vt100_screen_lines(parser.screen());
    let _ = output_tx.send(initial_lines).await;

    let mut interrupted = false;

    loop {
        tokio::select! {
            biased;

            // Check for interrupt signal
            _ = &mut interrupt_rx, if !interrupted => {
                interrupted = true;
                let _ = child.start_kill();
            }

            // Read stdout
            result = stdout_reader.read(&mut stdout_buf), if !stdout_done => {
                match result {
                    Ok(0) => stdout_done = true,
                    Ok(n) => {
                        full_stdout.extend_from_slice(&stdout_buf[..n]);
                        let normalized = ansi::onlcr(&stdout_buf[..n]).collect::<Vec<u8>>();
                        parser.process(&normalized);
                    }
                    Err(_) => stdout_done = true,
                }
            }

            // Read stderr
            result = stderr_reader.read(&mut stderr_buf), if !stderr_done => {
                match result {
                    Ok(0) => stderr_done = true,
                    Ok(n) => {
                        full_stderr.extend_from_slice(&stderr_buf[..n]);
                        // Feed stderr to the preview parser too, so it shows in the VT100 screen
                        let normalized = ansi::onlcr(&stderr_buf[..n]).collect::<Vec<u8>>();
                        parser.process(&normalized);
                    }
                    Err(_) => stderr_done = true,
                }
            }

            // Periodic screen snapshot for preview
            _ = interval.tick() => {
                let lines = vt100_screen_lines(parser.screen());
                let _ = output_tx.send(lines).await;
            }
        }

        // Exit when both streams are done
        if stdout_done && stderr_done {
            break;
        }
    }

    // Wait for process to finish
    let exit_code = match child.wait().await {
        Ok(status) => status.code(),
        Err(e) => {
            if interrupted {
                None
            } else {
                return ToolOutcome::Error(format!("Failed to wait for command: {e}"));
            }
        }
    };

    let duration = start.elapsed();

    // Send final screen state
    let final_lines = vt100_screen_lines(parser.screen());
    let _ = output_tx.send(final_lines).await;

    // Strip ANSI escape sequences for clean LLM output by running
    // the raw bytes through a VT100 parser and extracting plain text.
    let cols = PREVIEW_WIDTH;
    let stdout_text = ansi::to_plain_text(&full_stdout, cols);
    let stderr_text = ansi::to_plain_text(&full_stderr, cols);

    ToolOutcome::Structured {
        stdout: stdout_text,
        stderr: stderr_text,
        exit_code,
        duration_ms: u64::conv(duration.as_millis()),
        interrupted,
    }
}

#[derive(Debug, Clone)]
pub struct AtuinHistoryToolCall {
    pub filter_modes: Vec<HistorySearchFilterMode>,
    pub query: String,
    pub limit: i64,
    pub only_failed: bool,
    pub authors: OrFilter<Vec<AuthorPattern>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, EnumString, IntoStaticStr)]
#[strum(serialize_all = "lowercase")]
pub enum HistorySearchFilterMode {
    Global,
    Host,
    Session,
    Directory,
    Workspace,
}

impl HistorySearchFilterMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

impl From<HistorySearchFilterMode> for FilterMode {
    fn from(mode: HistorySearchFilterMode) -> Self {
        match mode {
            HistorySearchFilterMode::Global => Self::Global,
            HistorySearchFilterMode::Host => Self::Host,
            HistorySearchFilterMode::Session => Self::Session,
            HistorySearchFilterMode::Directory => Self::Directory,
            HistorySearchFilterMode::Workspace => Self::Workspace,
        }
    }
}

impl TryFrom<&serde_json::Value> for AtuinHistoryToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        // Optional; JSON null counts as omitted because models often send
        // null for optional params. A missing scope means a global search —
        // evals showed that forcing the model to pick a scope on every call
        // made it stop calling the tool at all.
        let filter_modes = match value.get("filter_modes") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or_else(|| eyre::eyre!("filter_modes must be an array"))?
                .iter()
                .map(|v| {
                    let mode = v.as_str().ok_or_else(|| eyre::eyre!("Invalid filter mode"))?;
                    mode.parse::<HistorySearchFilterMode>()
                        .map_err(|_| eyre::eyre!("Invalid filter mode: {mode}"))
                })
                .collect::<Result<Vec<HistorySearchFilterMode>>>()?,
        };

        let query = value
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("Missing query"))?;

        let limit = value
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(DEFAULT_HISTORY_RESULTS)
            .clamp(1, MAX_HISTORY_RESULTS);

        let only_failed = value.get("only_failed").and_then(|v| v.as_bool()).unwrap_or(false);

        let authors = match value.get("authors") {
            Some(authors) => authors
                .as_array()
                .ok_or_else(|| eyre::eyre!("authors must be an array of strings"))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(AuthorPattern::from)
                        .ok_or_else(|| eyre::eyre!("authors entries must be strings"))
                })
                .collect::<Result<Vec<AuthorPattern>>>()?,
            None => Vec::new(),
        };
        // An omitted or empty `authors` array means no author filtering.
        let authors = OrFilter::from_list(authors).unwrap_or_default();

        Ok(Self {
            filter_modes,
            query: query.to_string(),
            limit,
            only_failed,
            authors,
        })
    }
}

impl PermissibleToolCall for AtuinHistoryToolCall {
    fn target_dir(&self) -> Option<&Path> {
        None
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        rule.tool == "AtuinHistory"
    }
}

impl AtuinHistoryToolCall {
    pub(crate) async fn execute(&self, db: &atuin_client::database::Sqlite) -> ToolOutcome {
        use atuin_client::database::{self, DbSearchMode, OptFilters};

        // query_context rather than current_context: when running outside an
        // atuin-hooked shell (e.g. as an MCP server) there is no ATUIN_SESSION.
        let context = match database::query_context().await {
            Ok(ctx) => ctx,
            Err(e) => return ToolOutcome::Error(format!("Failed to get history context: {e}")),
        };

        let search_mode =
            self.filter_modes.first().copied().unwrap_or(HistorySearchFilterMode::Global);

        // An empty session would silently match nothing; error instead so a
        // missing $ATUIN_SESSION (e.g. MCP server launched outside a hooked
        // shell) isn't mistaken for empty history.
        if matches!(search_mode, HistorySearchFilterMode::Session) && context.session.is_empty() {
            return ToolOutcome::Error(
                "Session-scoped search is unavailable: $ATUIN_SESSION is not set, so there is no \
                 shell session to scope to. Use another filter mode."
                    .to_string(),
            );
        }

        let filter_options = OptFilters {
            // Fetch one row beyond the requested limit so the truncation
            // notice appended to the results below can say "more exist" as a
            // fact rather than a guess.
            limit: Some(self.limit + 1),
            only_failed: self.only_failed,
            authors: self.authors.as_slice_filter(),
            ..Default::default()
        };

        let mut results = match db
            .search(DbSearchMode::Fuzzy, search_mode.into(), &context, &self.query, filter_options)
            .await
        {
            Ok(results) => results,
            Err(e) => return ToolOutcome::Error(format!("History search failed: {e}")),
        };
        // The clamp keeps this in 1..=MAX_HISTORY_RESULTS, so the conversion
        // never fails; the fallback only exists to keep it infallible.
        let page_size = usize::try_from(self.limit.clamp(1, MAX_HISTORY_RESULTS)).unwrap_or(1);
        let truncated = results.len() > page_size;
        results.truncate(page_size);

        if results.is_empty() {
            // An unadorned "no results" reads to the model as "this tool is
            // useless". List which search parameters are worth loosening so
            // its retry has somewhere to go.
            let mut hints = Vec::new();
            if !self.query.is_empty() {
                hints.push("query terms are AND-ed, so try fewer or shorter terms");
            }
            if !matches!(search_mode, HistorySearchFilterMode::Global) {
                hints.push("widen the scope to 'global'");
            }
            if self.only_failed {
                hints.push("drop only_failed (the command may have succeeded)");
            }
            if !self.authors.is_all() {
                hints.push("drop the authors filter");
            }
            let mut msg = format!(
                "No history entries matched query {query:?} (scope: {scope}).",
                query = self.query,
                scope = search_mode.as_str(),
            );
            if !hints.is_empty() {
                msg.push_str(&format!(" To find more: {}.", hints.join("; ")));
            }
            return ToolOutcome::Success(msg);
        }

        let local_offset = time::UtcOffset::local_or_utc();

        let mut formatted: Vec<String> = results
            .iter()
            .enumerate()
            .map(|(i, history)| {
                crate::history_format::format_history_search_result(i + 1, history, local_offset)
            })
            .collect();

        if truncated {
            // The parser clamps `limit` to MAX_HISTORY_RESULTS, so a model
            // that retries with a larger limit at the cap would just get the
            // identical page back; only suggest raising it below the cap.
            let advice = if self.limit < MAX_HISTORY_RESULTS {
                "Refine the query or raise `limit` to see others."
            } else {
                "That is the maximum page size; refine the query to narrow them."
            };
            formatted
                .push(format!("[Showing the first {page_size} matches; more exist. {advice}]"));
        }

        ToolOutcome::Success(formatted.join("\n"))
    }
}

#[derive(Debug, Clone)]
pub struct AtuinOutputToolCall {
    pub history_id: HistoryId,
    /// The MCP protocol specifies that ranges should be Python-style, ie. array indices can be
    /// expressed as `[0, -1]` -- where `-1` refers to the element with cursor offset 1 from the end
    /// of the slice.
    pub ranges: Vec<PyStyleIdxRange>,
    /// The command the history entry ran, resolved from the local history
    /// db after parsing (`Effect::ResolveOutputCommand`). Display-only:
    /// `None` until the lookup lands, or when the id isn't known locally.
    pub command: Option<String>,
}

impl TryFrom<&serde_json::Value> for AtuinOutputToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let history_id: HistoryId = value
            .get("history_id")
            .and_then(|v| v.as_str())
            .and_then(|s| HistoryId::from_str(s).ok())
            .ok_or_else(|| eyre::eyre!("Missing or invalid history ID"))?;

        let ranges =
            value.get("ranges").and_then(|v| v.as_array()).map(Vec::as_slice).unwrap_or(&[]);

        let ranges = ranges
            .iter()
            .map(|r| {
                let range = r
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .ok_or_else(|| eyre::eyre!("Each range must be a [start, end] array"))?;

                let start = range[0]
                    .as_i64()
                    .ok_or_else(|| eyre::eyre!("Range start must be an integer"))?;
                let end =
                    range[1].as_i64().ok_or_else(|| eyre::eyre!("Range end must be an integer"))?;

                Ok(PyStyleIdxRange::new(start, end))
            })
            .collect::<Result<Vec<PyStyleIdxRange>, eyre::Error>>()?;

        Ok(Self {
            history_id,
            ranges,
            command: None,
        })
    }
}

impl PermissibleToolCall for AtuinOutputToolCall {
    fn target_dir(&self) -> Option<&Path> {
        None
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        rule.tool == "AtuinOutput"
    }
}

/// Render `ChunkedOutputLineView`s as `read_file`-style numbered output for the LLM, inserting
/// `[...skipped N lines...]` markers wherever the line numbers jump.
fn format_chunked_output_line_views_for_llm<'a>(
    lines: impl Iterator<Item = ChunkedOutputLineView<'a>> + Clone,
) -> String {
    let Some(max_line_no) = lines.clone().map(|line| line.line + 1).max() else {
        return String::new();
    };

    let width = usize::conv(max_line_no.max(1).ilog10()) + 1;

    let mut formatted = Vec::new();
    let mut previous_idx = None;
    for line in lines {
        if let Some(previous) = previous_idx {
            let skipped = line.line.saturating_sub(previous + 1);
            if skipped > 0 {
                formatted.push(format!("[...skipped {skipped} lines...]"));
            }
        }
        formatted.push(format!("{:>width$}\t{}", line.line + 1, line.content));
        previous_idx = Some(line.line);
    }
    formatted.join("\n")
}

impl AtuinOutputToolCall {
    pub(crate) async fn execute(&self) -> ToolOutcome {
        let settings = match atuin_client::settings::Settings::new() {
            Ok(settings) => settings,
            Err(e) => return ToolOutcome::Error(format!("Failed to load Atuin settings: {e}")),
        };

        let mut client = match atuin_daemon::HistoryClient::from_settings(&settings).await {
            Ok(client) => client,
            Err(e) => {
                return ToolOutcome::Error(format!(
                    "Captured output is unavailable: could not connect to the Atuin daemon ({e}). \
                     History search still works. {NO_OUTPUT_ADVICE}"
                ));
            }
        };

        let history_id = self.history_id;

        let not_found = || {
            ToolOutcome::Success(format!(
                "No captured output found for history ID {history_id}. Output is only captured \
                 for commands run in an Atuin-enabled terminal while the daemon was running; \
                 older output may also have been dropped. {NO_OUTPUT_ADVICE}"
            ))
        };

        // An empty request means "give me everything": a single `[0, -1]` range spans the whole
        // output.
        let ranges = if self.ranges.is_empty() {
            vec![PyStyleIdxRange::new(0, -1)]
        } else {
            self.ranges.clone()
        };

        let response = match client.get_command_output(history_id, ranges).await {
            Ok(Some(response)) => response,
            Ok(None) => return not_found(),
            Err(e) => return ToolOutcome::Error(format!("Failed to fetch command output: {e}")),
        };

        let body = format_chunked_output_line_views_for_llm(response.lines());
        if body.is_empty() {
            return ToolOutcome::Success(if self.ranges.is_empty() {
                format!("Captured output for history ID {history_id} is empty.")
            } else {
                format!("No lines selected from captured output for history ID {history_id}.")
            });
        }
        let totals = format!("{} bytes, {} lines", response.total_bytes, response.total_lines);
        let meta = response.meta.unwrap_or_default();

        let total_output = if meta.output_truncated {
            format!("{totals} ({} bytes observed before truncation)", meta.output_observed_bytes)
        } else {
            totals
        };

        ToolOutcome::Success(format!(
            "History ID: {history_id}\nTotal output: {total_output}\nSelected output:\n{body}"
        ))
    }
}

#[derive(Debug, Clone)]
pub struct LoadSkillToolCall {
    pub name: String,
}

impl TryFrom<&serde_json::Value> for LoadSkillToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let name =
            value.get("name").and_then(|v| v.as_str()).ok_or(eyre::eyre!("Missing skill name"))?;

        Ok(Self {
            name: name.to_string(),
        })
    }
}

impl PermissibleToolCall for LoadSkillToolCall {
    fn target_dir(&self) -> Option<&Path> {
        None
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        rule.tool == "LoadSkill"
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::filter;
    use atuin_daemon::grpc::history::pb::{
        CommandCapture, CommandCaptureMeta, GetCommandOutputResponse,
    };
    use rstest::*;

    use super::*;

    fn read_rule(scope: Option<&str>) -> Rule {
        Rule {
            tool: "Read".to_string(),
            scope: scope.map(String::from),
        }
    }

    fn write_rule(scope: Option<&str>) -> Rule {
        Rule {
            tool: "Write".to_string(),
            scope: scope.map(String::from),
        }
    }

    fn read_tool(path: &str) -> ReadToolCall {
        ReadToolCall {
            path: expand_path(path),
            offset: 0,
            limit: 100,
        }
    }

    fn write_tool(path: &str) -> WriteToolCall {
        WriteToolCall {
            path: expand_path(path),
            content: String::new(),
            overwrite: false,
        }
    }

    // ── Cross-platform tests ──

    #[rstest]
    #[case::omitted(serde_json::json!({ "query": "cargo" }))]
    #[case::null(serde_json::json!({ "query": "cargo", "filter_modes": null }))]
    fn atuin_history_filter_modes_are_optional(#[case] input: serde_json::Value) {
        let call = AtuinHistoryToolCall::try_from(&input).unwrap();
        assert!(call.filter_modes.is_empty());
    }

    #[rstest]
    fn filter_mode_names_round_trip() {
        use strum::IntoEnumIterator;
        for mode in HistorySearchFilterMode::iter() {
            assert_eq!(mode.as_str().parse::<HistorySearchFilterMode>(), Ok(mode));
        }
    }

    #[rstest]
    fn atuin_history_filter_modes_reject_non_arrays() {
        let input = serde_json::json!({ "query": "cargo", "filter_modes": "global" });
        assert!(AtuinHistoryToolCall::try_from(&input).is_err());
    }

    #[rstest]
    fn atuin_history_author_and_failure_filters_parse() {
        let input = serde_json::json!({ "query": "cargo" });

        let call = AtuinHistoryToolCall::try_from(&input).unwrap();
        assert!(!call.only_failed);
        assert!(call.authors.is_all());

        let input = serde_json::json!({
            "query": "cargo",
            "only_failed": true,
            "authors": ["$all-agent"],
        });

        let call = AtuinHistoryToolCall::try_from(&input).unwrap();
        assert!(call.only_failed);
        assert_eq!(call.authors.items(), filter::Items::Some([AuthorPattern::AllAgent].as_slice()));
    }

    #[rstest]
    fn atuin_output_ranges_are_optional() -> eyre::Result<()> {
        let input = serde_json::json!({
            "history_id": "018f0000000070008000000000000000"
        });

        let call = AtuinOutputToolCall::try_from(&input)?;

        assert_eq!(call.history_id.to_string(), "018f0000000070008000000000000000");
        assert!(call.ranges.is_empty());
        Ok(())
    }

    #[rstest]
    fn atuin_output_parses_line_ranges() -> eyre::Result<()> {
        let input = serde_json::json!({
            "history_id": "018f0000000070008000000000000000",
            "ranges": [[0, 30], [-100, -1]]
        });

        let call = AtuinOutputToolCall::try_from(&input)?;

        assert_eq!(call.ranges, vec![PyStyleIdxRange::new(0, 30), PyStyleIdxRange::new(-100, -1),]);
        Ok(())
    }

    #[rstest]
    fn atuin_output_formats_lines_like_read_file() {
        // 0-based line indices 97 and 99 render as line numbers 98 and 100, with a gap marker.
        let lines = [
            ChunkedOutputLineView {
                line: 97,
                content: "near end",
            },
            ChunkedOutputLineView {
                line: 99,
                content: "end",
            },
        ];

        assert_eq!(
            format_chunked_output_line_views_for_llm(lines.into_iter()),
            " 98\tnear end\n[...skipped 1 lines...]\n100\tend"
        );
    }

    #[rstest]
    fn atuin_output_renders_a_blank_line_instead_of_widening_the_gap() {
        // Line 1 of the output is blank. Selecting it alongside the last two lines must render it
        // as a blank numbered line and report exactly the two lines genuinely left out, "charlie"
        // and "delta". Reconstructing chunk contents with `str::lines` used to swallow the blank
        // line and inflate the marker to three.
        let capture = CommandCapture {
            output: "alpha\n\ncharlie\ndelta\necho\nfoxtrot".to_string(),
            meta: Some(CommandCaptureMeta {
                output_truncated: false,
                output_observed_bytes: 0,
            }),
        };
        let chunked = GetCommandOutputResponse::build(capture, &[
            PyStyleIdxRange::new(0, 1),
            PyStyleIdxRange::new(4, 5),
        ]);

        assert_eq!(
            format_chunked_output_line_views_for_llm(chunked.lines()),
            "1\talpha\n2\t\n[...skipped 2 lines...]\n5\techo\n6\tfoxtrot"
        );
    }

    #[rstest]
    #[case::read_rule_none(read_rule(None), true)]
    #[case::write_implies_read(write_rule(None), true)]
    fn read_tool_rule_name(#[case] rule: Rule, #[case] expected: bool) {
        assert_eq!(read_tool("foo.txt").matches_rule(&rule), expected);
    }

    #[rstest]
    #[case::write_rule_none(write_rule(None), true)]
    #[case::read_does_not_imply_write(read_rule(None), false)]
    fn write_tool_rule_name(#[case] rule: Rule, #[case] expected: bool) {
        assert_eq!(write_tool("foo.txt").matches_rule(&rule), expected);
    }

    #[rstest]
    #[case::edit_uses_write(write_rule(None), true)]
    #[case::edit_rejects_read(read_rule(None), false)]
    fn edit_tool_rule_name(#[case] rule: Rule, #[case] expected: bool) {
        let edit = EditToolCall {
            path: expand_path("/home/user/config.toml"),
            old_string: "x".into(),
            new_string: "y".into(),
            replace_all: false,
        };
        assert_eq!(edit.matches_rule(&rule), expected);
    }

    #[rstest]
    #[case::wildcard_star("foo/bar.rs", "*", true)]
    #[case::extension_glob_matches("notes.md", "*.md", true)]
    #[case::extension_glob_rejects("notes.txt", "*.md", false)]
    #[cfg_attr(
        unix,
        case::unix_absolute_glob_matches("/home/user/src/main.rs", "/home/user/src/*.rs", true)
    )]
    #[cfg_attr(
        unix,
        case::unix_absolute_glob_rejects(
            "/home/user/docs/readme.md",
            "/home/user/src/*.rs",
            false
        )
    )]
    #[cfg_attr(
        unix,
        case::unix_double_star_matches(
            "/project/crates/foo/src/lib.rs",
            "/project/crates/**/*.rs",
            true
        )
    )]
    #[cfg_attr(
        unix,
        case::unix_double_star_rejects(
            "/project/crates/foo/src/lib.py",
            "/project/crates/**/*.rs",
            false
        )
    )]
    #[cfg_attr(
        windows,
        case::windows_absolute_glob_matches(
            r"C:\Users\dev\src\main.rs",
            "C:/Users/dev/src/*.rs",
            true
        )
    )]
    #[cfg_attr(
        windows,
        case::windows_absolute_glob_rejects(
            r"C:\Users\dev\docs\readme.md",
            "C:/Users/dev/src/*.rs",
            false
        )
    )]
    #[cfg_attr(
        windows,
        case::windows_double_star_matches(
            r"C:\project\crates\foo\src\lib.rs",
            "C:/project/crates/**/*.rs",
            true
        )
    )]
    #[cfg_attr(
        windows,
        case::windows_double_star_rejects(
            r"C:\project\crates\foo\src\lib.py",
            "C:/project/crates/**/*.rs",
            false
        )
    )]
    fn read_scope_glob(#[case] path: &str, #[case] scope: &str, #[case] expected: bool) {
        assert_eq!(read_tool(path).matches_rule(&read_rule(Some(scope))), expected);
    }

    #[rstest]
    #[case("crates/**/*.rs", true)]
    #[case("crates/**/*.py", false)]
    fn relative_multi_segment_glob(#[case] scope: &str, #[case] expected: bool) {
        // This matches against the path relative to cwd
        let cwd = std::env::current_dir().unwrap();
        let abs = cwd.join("crates").join("atuin-ai").join("src").join("lib.rs");
        let tool = read_tool(abs.to_str().unwrap());
        assert_eq!(tool.matches_rule(&read_rule(Some(scope))), expected);
    }

    // ── all_covered_by tests (compound shell command semantics) ──

    fn shell_rule(scope: Option<&str>) -> Rule {
        Rule {
            tool: "Shell".to_string(),
            scope: scope.map(String::from),
        }
    }

    fn shell_tool(command: &str) -> ShellToolCall {
        ShellToolCall {
            dir: None,
            command: command.to_string(),
            shell: "bash".to_string(),
            timeout_secs: 30,
            description: None,
        }
    }

    #[rstest]
    #[case::git_scope_allows(vec![shell_rule(Some("git *"))], "git add .", true)]
    #[case::git_scope_rejects_npm(vec![shell_rule(Some("git *"))], "npm test", false)]
    #[case::compound_all_covered(
        vec![shell_rule(Some("git *")), shell_rule(Some("npm *"))],
        "git add . && npm test",
        true
    )]
    #[case::compound_partially_covered(
        vec![shell_rule(Some("git *"))],
        "git add . && npm test",
        false
    )]
    #[case::unscoped_covers_all(vec![shell_rule(None)], "git add . && rm -rf /", true)]
    #[case::wildcard_covers_all(vec![shell_rule(Some("*"))], "git add . && npm test", true)]
    fn shell_all_covered_by(
        #[case] rules: Vec<Rule>,
        #[case] command: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(shell_tool(command).all_covered_by(&rules), expected);
    }

    #[rstest]
    fn all_covered_by_non_shell_tool_unchanged() {
        // Non-shell tools use the default (any single rule matches)
        let rules = vec![read_rule(Some("*.md"))];
        assert!(read_tool("notes.md").all_covered_by(&rules));
        assert!(!read_tool("notes.txt").all_covered_by(&rules));
    }

    #[rstest]
    fn matches_rule_still_uses_any_semantics() {
        // matches_rule (used for deny/ask) still triggers on any subcommand
        let rule = shell_rule(Some("rm *"));
        assert!(shell_tool("git add . && rm -rf /").matches_rule(&rule));
    }

    #[rstest]
    fn bare_pattern_asymmetry() {
        // Deny (matches_rule, prefix_bare=true): bare "rm" blocks "rm -rf /"
        let deny_rule = shell_rule(Some("rm"));
        assert!(shell_tool("rm -rf /").matches_rule(&deny_rule));

        // Allow (all_covered_by, prefix_bare=false): bare "rm" only allows exactly "rm"
        let allow_rules = vec![shell_rule(Some("rm"))];
        assert!(shell_tool("rm").all_covered_by(&allow_rules));
        assert!(!shell_tool("rm -rf /").all_covered_by(&allow_rules));

        // Bare prefix match is word-boundary, not substring — "rm" must not match "rmbackup"
        assert!(!shell_tool("rmbackup").matches_rule(&deny_rule));
        assert!(!shell_tool("rmbackup /tmp").matches_rule(&deny_rule));
    }

    // ── edit_file execution tests ──

    mod edit {
        use super::*;
        use crate::file_tracker::FileReadTracker;

        /// Helper: create a temp file (with a closed handle), record it in a tracker.
        /// Returns the TempDir (keeps the path alive) and tracker.
        /// The file handle is closed so atomic_write_file can rename over it on Windows.
        fn setup_tracked_file(content: &str) -> (tempfile::TempDir, PathBuf, FileReadTracker) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test_file.toml");
            std::fs::write(&path, content).unwrap();

            let file_content = std::fs::read(&path).unwrap();
            let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

            let mut tracker = FileReadTracker::default();
            tracker.record_read(path.clone(), &file_content, mtime);

            (dir, path, tracker)
        }

        fn edit_call(path: &Path, old: &str, new: &str, replace_all: bool) -> EditToolCall {
            EditToolCall {
                path: path.to_path_buf(),
                old_string: old.to_string(),
                new_string: new.to_string(),
                replace_all,
            }
        }

        #[rstest]
        #[case::single(
            "[section]\nkey = old_value\n",
            "old_value",
            "new_value",
            false,
            "[section]\nkey = new_value\n",
            None
        )]
        #[case::replace_all(
            "aaa bbb aaa ccc aaa",
            "aaa",
            "xxx",
            true,
            "xxx bbb xxx ccc xxx",
            Some("3 occurrences")
        )]
        #[case::multiline(
            "[section]\nkey1 = val1\nkey2 = val2\n[other]\n",
            "key1 = val1\nkey2 = val2",
            "key1 = new1\nkey2 = new2",
            false,
            "[section]\nkey1 = new1\nkey2 = new2\n[other]\n",
            None
        )]
        fn edit_success(
            #[case] content: &str,
            #[case] old: &str,
            #[case] new: &str,
            #[case] replace_all: bool,
            #[case] expected: &str,
            #[case] success_substr: Option<&str>,
        ) {
            let (_dir, path, tracker) = setup_tracked_file(content);

            let call = edit_call(&path, old, new, replace_all);
            let (outcome, new_bytes) = call.execute(&path, &tracker);

            assert!(matches!(outcome, ToolOutcome::Success(_)));
            if let Some(s) = success_substr {
                assert!(matches!(outcome, ToolOutcome::Success(ref out) if out.contains(s)));
            }
            assert!(new_bytes.is_some());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        }

        #[rstest]
        fn error_file_not_read() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("unread.txt");
            std::fs::write(&path, "content").unwrap();
            let tracker = FileReadTracker::default(); // empty — never read

            let call = edit_call(&path, "x", "y", false);
            let (outcome, new_bytes) = call.execute(&path, &tracker);

            assert!(new_bytes.is_none());
            match outcome {
                ToolOutcome::Error(msg) => {
                    assert!(msg.contains("not been read yet"), "got: {msg}");
                }
                _ => panic!("expected error"),
            }
        }

        #[rstest]
        fn error_file_modified_since_read() {
            let (_dir, path, tracker) = setup_tracked_file("original");

            // Modify the file after the read was recorded
            std::thread::sleep(std::time::Duration::from_millis(10));
            std::fs::write(&path, "modified externally").unwrap();

            let call = edit_call(&path, "original", "replaced", false);
            let (outcome, new_bytes) = call.execute(&path, &tracker);

            assert!(new_bytes.is_none());
            match outcome {
                ToolOutcome::Error(msg) => {
                    assert!(msg.contains("modified since read"), "got: {msg}");
                }
                _ => panic!("expected error"),
            }
        }

        #[rstest]
        #[case::no_match("hello world", "nonexistent", "replacement", false, &["not found"], false)]
        #[case::multiple_without_replace_all(
            "foo bar foo baz foo",
            "foo",
            "qux",
            false,
            &["3 matches", "replace_all"],
            true
        )]
        #[case::empty_old_string("content", "", "something", false, &[], true)]
        #[case::preserves_on_no_match(
            "[config]\nport = 8080\nhost = localhost\n",
            "port = 9090",
            "port = 3000",
            false,
            &[],
            true
        )]
        fn edit_error(
            #[case] content: &str,
            #[case] old: &str,
            #[case] new: &str,
            #[case] replace_all: bool,
            #[case] expect_substrings: &[&str],
            #[case] check_unchanged: bool,
        ) {
            let (_dir, path, tracker) = setup_tracked_file(content);

            let (outcome, new_bytes) =
                edit_call(&path, old, new, replace_all).execute(&path, &tracker);

            assert!(new_bytes.is_none());
            let ToolOutcome::Error(msg) = outcome else {
                panic!("expected error")
            };
            for s in expect_substrings {
                assert!(msg.contains(*s), "got: {msg}");
            }
            if check_unchanged {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
            }
        }

        #[rstest]
        fn error_file_does_not_exist() {
            let tracker = FileReadTracker::default();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("nonexistent.txt");

            let call = edit_call(&path, "x", "y", false);
            let (outcome, new_bytes) = call.execute(&path, &tracker);

            assert!(new_bytes.is_none());
            match outcome {
                ToolOutcome::Error(msg) => {
                    assert!(msg.contains("does not exist"), "got: {msg}");
                }
                _ => panic!("expected error"),
            }
        }
    }

    // ── Integration tests: full edit lifecycle ──
    //
    // These exercise the cross-component flow that dispatch orchestrates:
    // FileReadTracker → SnapshotStore → EditToolCall.execute → tracker update

    mod edit_integration {
        use super::*;
        use crate::edit_permissions::EditPermissionCache;
        use crate::file_tracker::FileReadTracker;
        use crate::snapshots::SnapshotStore;

        /// Simulate a file read (what dispatch does after ReadToolCall.execute).
        fn simulate_read(tracker: &mut FileReadTracker, path: &std::path::Path) {
            let content = std::fs::read(path).unwrap();
            let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
            tracker.record_read(path.to_path_buf(), &content, mtime);
        }

        /// Simulate a tracker update after edit (what dispatch does after execute).
        fn simulate_tracker_update(
            tracker: &mut FileReadTracker,
            path: &std::path::Path,
            new_bytes: &[u8],
        ) {
            let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
            tracker.update_after_edit(path, new_bytes, mtime);
        }

        #[rstest]
        fn full_read_snapshot_edit_cycle() {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("config.toml");
            std::fs::write(&file_path, "[db]\nhost = localhost\nport = 5432\n").unwrap();

            let snapshot_dir = dir.path().join("snapshots").join("session-1");
            let mut tracker = FileReadTracker::default();
            let mut store = SnapshotStore::open(snapshot_dir.clone()).unwrap();

            // 1. Simulate reading the file
            simulate_read(&mut tracker, &file_path);

            // 2. Snapshot before edit
            let original = std::fs::read(&file_path).unwrap();
            store.ensure_snapshot(&file_path, &original).unwrap();

            // 3. Execute edit
            let call = EditToolCall {
                path: file_path.clone(),
                old_string: "host = localhost".to_string(),
                new_string: "host = 10.0.0.1".to_string(),
                replace_all: false,
            };
            let (outcome, new_bytes) = call.execute(&file_path, &tracker);
            assert!(matches!(outcome, ToolOutcome::Success(_)));
            let new_bytes = new_bytes.unwrap();

            // 4. Update tracker (simulating what dispatch does)
            simulate_tracker_update(&mut tracker, &file_path, &new_bytes);

            // Verify: file was edited
            assert_eq!(
                std::fs::read_to_string(&file_path).unwrap(),
                "[db]\nhost = 10.0.0.1\nport = 5432\n"
            );

            // Verify: snapshot has original content
            assert!(store.has_snapshot(&file_path));
            let snapshot_name = crate::snapshots::sanitize_path(&file_path);
            let snapshot_content =
                std::fs::read_to_string(snapshot_dir.join(snapshot_name)).unwrap();
            assert_eq!(snapshot_content, "[db]\nhost = localhost\nport = 5432\n");
        }

        #[rstest]
        fn second_edit_without_reread() {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("config.toml");
            std::fs::write(&file_path, "key1 = aaa\nkey2 = bbb\n").unwrap();

            let mut tracker = FileReadTracker::default();

            // Read the file
            simulate_read(&mut tracker, &file_path);

            // First edit
            let call1 = EditToolCall {
                path: file_path.clone(),
                old_string: "key1 = aaa".to_string(),
                new_string: "key1 = xxx".to_string(),
                replace_all: false,
            };
            let (outcome, new_bytes) = call1.execute(&file_path, &tracker);
            assert!(matches!(outcome, ToolOutcome::Success(_)));
            simulate_tracker_update(&mut tracker, &file_path, &new_bytes.unwrap());

            // Second edit — should work without re-reading because tracker was updated
            let call2 = EditToolCall {
                path: file_path.clone(),
                old_string: "key2 = bbb".to_string(),
                new_string: "key2 = yyy".to_string(),
                replace_all: false,
            };
            let (outcome, new_bytes) = call2.execute(&file_path, &tracker);
            assert!(matches!(outcome, ToolOutcome::Success(_)));
            assert!(new_bytes.is_some());
            assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "key1 = xxx\nkey2 = yyy\n");
        }

        #[rstest]
        fn external_modification_between_edits() {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("config.toml");
            std::fs::write(&file_path, "value = original\n").unwrap();

            let mut tracker = FileReadTracker::default();
            simulate_read(&mut tracker, &file_path);

            // First edit succeeds
            let call1 = EditToolCall {
                path: file_path.clone(),
                old_string: "value = original".to_string(),
                new_string: "value = edited".to_string(),
                replace_all: false,
            };
            let (outcome, new_bytes) = call1.execute(&file_path, &tracker);
            assert!(matches!(outcome, ToolOutcome::Success(_)));
            simulate_tracker_update(&mut tracker, &file_path, &new_bytes.unwrap());

            // External modification (e.g., user edits the file)
            std::thread::sleep(std::time::Duration::from_millis(10));
            std::fs::write(&file_path, "value = user_changed\n").unwrap();

            // Second edit should fail (stale)
            let call2 = EditToolCall {
                path: file_path.clone(),
                old_string: "value = edited".to_string(),
                new_string: "value = second_edit".to_string(),
                replace_all: false,
            };
            let (outcome, new_bytes) = call2.execute(&file_path, &tracker);
            assert!(new_bytes.is_none());
            match outcome {
                ToolOutcome::Error(msg) => assert!(msg.contains("modified since read")),
                _ => panic!("expected stale error"),
            }

            // File should be unchanged (the user's edit preserved)
            assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "value = user_changed\n");
        }

        #[rstest]
        fn snapshot_only_created_once_per_file() {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("config.toml");
            std::fs::write(&file_path, "a = 1\nb = 2\n").unwrap();

            let snapshot_dir = dir.path().join("snapshots").join("session-1");
            let mut tracker = FileReadTracker::default();
            let mut store = SnapshotStore::open(snapshot_dir).unwrap();

            simulate_read(&mut tracker, &file_path);

            // First edit — snapshot should be created
            let original = std::fs::read(&file_path).unwrap();
            let created = store.ensure_snapshot(&file_path, &original).unwrap();
            assert!(created);

            let call1 = EditToolCall {
                path: file_path.clone(),
                old_string: "a = 1".to_string(),
                new_string: "a = 10".to_string(),
                replace_all: false,
            };
            let (_, new_bytes) = call1.execute(&file_path, &tracker);
            simulate_tracker_update(&mut tracker, &file_path, &new_bytes.unwrap());

            // Second edit — snapshot should NOT be recreated
            let content_before_second = std::fs::read(&file_path).unwrap();
            let created = store.ensure_snapshot(&file_path, &content_before_second).unwrap();
            assert!(!created); // idempotent — already snapshotted
        }

        #[rstest]
        fn permission_cache_grant_and_check() {
            let mut cache = EditPermissionCache::default();
            let path = std::path::PathBuf::from("/Users/me/.config/atuin/config.toml");

            // Initially no grant
            assert!(!cache.has_valid_grant(&path));

            // Grant permission
            cache.grant(path.clone());
            assert!(cache.has_valid_grant(&path));

            // Different file has no grant
            assert!(!cache.has_valid_grant(std::path::Path::new("/other/file.toml")));

            // Roundtrip through JSON (simulates session persistence)
            let json = cache.to_json().unwrap();
            let restored = EditPermissionCache::from_json(&json).unwrap();
            assert!(restored.has_valid_grant(&path));
        }
    }

    // ── write_file execution tests ──

    mod write {
        use super::*;

        #[fixture]
        fn tempdir() -> tempfile::TempDir {
            tempfile::tempdir().unwrap()
        }

        #[rstest]
        fn creates_new_file(tempdir: tempfile::TempDir) {
            let path = tempdir.path().join("new_file.txt");

            let call = WriteToolCall {
                path: path.clone(),
                content: "hello\nworld\n".to_string(),
                overwrite: false,
            };
            let (outcome, new_bytes) = call.execute(&path);

            assert!(matches!(outcome, ToolOutcome::Success(ref s) if s.contains("Created")));
            assert!(new_bytes.is_some());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\nworld\n");
        }

        #[rstest]
        #[case::rejects_without_overwrite(false, "new content", true, "original")]
        #[case::overwrites_with_flag(true, "replaced content\n", false, "replaced content\n")]
        fn write_over_existing(
            tempdir: tempfile::TempDir,
            #[case] overwrite: bool,
            #[case] new_content: &str,
            #[case] expect_error: bool,
            #[case] expected_final: &str,
        ) {
            let path = tempdir.path().join("existing.txt");
            std::fs::write(&path, "original").unwrap();

            let call = WriteToolCall {
                path: path.clone(),
                content: new_content.to_string(),
                overwrite,
            };
            let (outcome, new_bytes) = call.execute(&path);

            if expect_error {
                assert!(new_bytes.is_none());
                match outcome {
                    ToolOutcome::Error(msg) => {
                        assert!(msg.contains("already exists"), "got: {msg}");
                        assert!(msg.contains("overwrite"), "got: {msg}");
                    }
                    _ => panic!("expected error"),
                }
            } else {
                assert!(matches!(outcome, ToolOutcome::Success(_)));
                assert!(new_bytes.is_some());
            }
            assert_eq!(std::fs::read_to_string(&path).unwrap(), expected_final);
        }

        #[rstest]
        fn creates_parent_directories(tempdir: tempfile::TempDir) {
            let path = tempdir.path().join("sub").join("dir").join("file.txt");

            let call = WriteToolCall {
                path: path.clone(),
                content: "nested\n".to_string(),
                overwrite: false,
            };
            let (outcome, _) = call.execute(&path);

            assert!(matches!(outcome, ToolOutcome::Success(_)));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested\n");
        }

        #[rstest]
        fn error_path_is_directory(tempdir: tempfile::TempDir) {
            let path = tempdir.path().to_path_buf();

            let call = WriteToolCall {
                path: path.clone(),
                content: "content".to_string(),
                overwrite: false,
            };
            let (outcome, new_bytes) = call.execute(&path);

            assert!(new_bytes.is_none());
            assert!(matches!(outcome, ToolOutcome::Error(ref msg) if msg.contains("directory")));
        }
    }
}
