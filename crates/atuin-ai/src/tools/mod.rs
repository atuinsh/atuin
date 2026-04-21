use std::{
    io::BufRead,
    path::{Path, PathBuf},
    time::Duration,
};

use eyre::Result;

const DEFAULT_FILE_READ_LINES: u64 = 100;
const MAX_FILE_READ_LINES: u64 = 1000;

pub(crate) mod descriptor;

use crate::permissions::rule::Rule;

/// Check whether a file path matches a scope glob pattern.
///
/// Resolves relative paths against the current directory before matching so
/// that `./foo.md` and `/cwd/foo.md` match the same glob. Supports `*`, `**`,
/// `?`, and `[...]` via `glob_match`.
fn path_matches_scope(path: &Path, scope: &str) -> bool {
    let path = if path.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
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
pub(crate) enum ToolOutcome {
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
            ToolOutcome::Success(s) => s.clone(),
            ToolOutcome::Error(e) => e.clone(),
            ToolOutcome::Structured {
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

                if !stdout.is_empty() {
                    parts.push(format!("stdout:\n{stdout}"));
                } else {
                    parts.push("stdout: (empty)".to_string());
                }

                if !stderr.is_empty() {
                    parts.push(format!("stderr:\n{stderr}"));
                } else {
                    parts.push("stderr: (empty)".to_string());
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
            ToolOutcome::Error(_) => true,
            ToolOutcome::Structured {
                exit_code: Some(code),
                ..
            } if *code != 0 => true,
            _ => false,
        }
    }
}

/// Cached VT100 preview data for a shell tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolPreview {
    pub lines: Vec<String>,
    pub exit_code: Option<i32>,
    pub interrupted: Option<crate::fsm::tools::InterruptReason>,
}

/// A tool call from the server, with parsed input parameters.
#[derive(Debug, Clone)]
pub(crate) enum ClientToolCall {
    Read(ReadToolCall),
    Edit(EditToolCall),
    Write(WriteToolCall),
    Shell(ShellToolCall),
    AtuinHistory(AtuinHistoryToolCall),
}

impl TryFrom<(&str, &serde_json::Value)> for ClientToolCall {
    type Error = eyre::Error;

    fn try_from((name, input): (&str, &serde_json::Value)) -> Result<Self, Self::Error> {
        match name {
            "read_file" => Ok(ClientToolCall::Read(ReadToolCall::try_from(input)?)),
            "edit_file" => Ok(ClientToolCall::Edit(EditToolCall::try_from(input)?)),
            "write_file" => Ok(ClientToolCall::Write(WriteToolCall::try_from(input)?)),
            "execute_shell_command" => Ok(ClientToolCall::Shell(ShellToolCall::try_from(input)?)),
            "atuin_history" => Ok(ClientToolCall::AtuinHistory(
                AtuinHistoryToolCall::try_from(input)?,
            )),
            _ => Err(eyre::eyre!("Unknown tool call: {name}")),
        }
    }
}

impl ClientToolCall {
    pub(crate) fn descriptor(&self) -> &'static descriptor::ToolDescriptor {
        match self {
            ClientToolCall::Read(_) => descriptor::READ,
            ClientToolCall::Edit(_) => descriptor::EDIT,
            ClientToolCall::Write(_) => descriptor::WRITE,
            ClientToolCall::Shell(_) => descriptor::SHELL,
            ClientToolCall::AtuinHistory(_) => descriptor::ATUIN_HISTORY,
        }
    }

    /// The permission rule name for this tool category.
    ///
    /// Edit and Write share the `"Write"` rule name — a Write permission
    /// covers both str_replace edits and full file creates. Write also
    /// implies Read (checked in `ReadToolCall::matches_rule`).
    pub(crate) fn rule_name(&self) -> &'static str {
        match self {
            ClientToolCall::Read(_) => "Read",
            ClientToolCall::Edit(_) => "Write",
            ClientToolCall::Write(_) => "Write",
            ClientToolCall::Shell(_) => "Shell",
            ClientToolCall::AtuinHistory(_) => "AtuinHistory",
        }
    }

    /// The resolved file path for this tool call, if it's a file-based tool.
    /// Used to build scoped permission rules like `Write(/abs/path/to/file)`.
    pub(crate) fn resolved_file_path(&self) -> Option<PathBuf> {
        match self {
            ClientToolCall::Read(tool) => Some(tool.resolved_path()),
            ClientToolCall::Edit(tool) => Some(tool.resolved_path()),
            ClientToolCall::Write(tool) => Some(tool.resolved_path()),
            _ => None,
        }
    }

    pub(crate) fn matches_rule(&self, rule: &Rule) -> bool {
        match self {
            ClientToolCall::Read(tool) => tool.matches_rule(rule),
            ClientToolCall::Edit(tool) => tool.matches_rule(rule),
            ClientToolCall::Write(tool) => tool.matches_rule(rule),
            ClientToolCall::Shell(tool) => tool.matches_rule(rule),
            ClientToolCall::AtuinHistory(tool) => tool.matches_rule(rule),
        }
    }

    pub(crate) fn target_dir(&self) -> Option<&Path> {
        match self {
            ClientToolCall::Read(tool) => tool.target_dir(),
            ClientToolCall::Edit(tool) => tool.target_dir(),
            ClientToolCall::Write(tool) => tool.target_dir(),
            ClientToolCall::Shell(tool) => tool.target_dir(),
            ClientToolCall::AtuinHistory(tool) => tool.target_dir(),
        }
    }

    /// Execute this client-side tool and return the result.
    pub async fn execute(&self, db: &atuin_client::database::Sqlite) -> ToolOutcome {
        match self {
            ClientToolCall::Read(tool) => tool.execute(),
            ClientToolCall::AtuinHistory(tool) => tool.execute(db).await,
            _ => ToolOutcome::Error("Client-side tool execution not yet implemented".to_string()),
        }
    }
}

/// A trait for tool calls that can be checked against permission rules.
pub(crate) trait PermissableToolCall {
    /// Checks if this tool call matches the given permission rule.
    fn matches_rule(&self, rule: &Rule) -> bool;
    /// Returns the target directory of this tool call, if applicable, for checking against directory-based rules.
    fn target_dir(&self) -> Option<&Path> {
        None
    }
}

impl PermissableToolCall for ClientToolCall {
    fn matches_rule(&self, rule: &Rule) -> bool {
        self.matches_rule(rule)
    }

    fn target_dir(&self) -> Option<&Path> {
        self.target_dir()
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
pub(crate) struct ReadToolCall {
    pub path: PathBuf,
    pub offset: u64,
    pub limit: u64,
}

impl TryFrom<&serde_json::Value> for ReadToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let path = value
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing path"))?;

        let offset = value.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
        let limit = value
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_FILE_READ_LINES)
            .min(MAX_FILE_READ_LINES);

        Ok(ReadToolCall {
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
            .skip(self.offset as usize)
            .take(self.limit as usize)
            .collect::<Result<Vec<_>, _>>();

        match raw_lines {
            Ok(lines) => {
                let first_line_no = self.offset as usize + 1;
                let last_line_no = first_line_no + lines.len().saturating_sub(1);
                let width = last_line_no.max(1).ilog10() as usize + 1;

                let numbered: String = lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:>width$}\t{line}", first_line_no + i))
                    .collect::<Vec<_>>()
                    .join("\n");

                if numbered.len() > 100_000 {
                    ToolOutcome::Error(format!(
                        "Error: file is too large to read ({} bytes in {} lines); use view_range to read a subset of the file",
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

impl PermissableToolCall for ReadToolCall {
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
pub(crate) struct EditToolCall {
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

        let replace_all = value
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(EditToolCall {
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
                        "File has been modified since read, either by the user or by a linter. Read it again before attempting to edit it.".to_string(),
                    ),
                    None,
                );
            }
            Err(e) => {
                return (
                    ToolOutcome::Error(format!("Error checking file state: {e}")),
                    None,
                );
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
                    "old_string not found in {}. Make sure it matches exactly, including whitespace and indentation.",
                    resolved_path.display()
                )),
                None,
            );
        }

        if match_count > 1 && !self.replace_all {
            return (
                ToolOutcome::Error(format!(
                    "Found {match_count} matches of old_string in {}, but replace_all is false. Either provide more context to make the match unique, or set replace_all to true.",
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

impl PermissableToolCall for EditToolCall {
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
pub(crate) struct WriteToolCall {
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

        let content = value
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing content"))?;

        let overwrite = value
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(WriteToolCall {
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
                    "File already exists: {}. Set overwrite to true to replace it, or use edit_file to make targeted changes.",
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
        let verb = if existed { "Overwrote" } else { "Created" };
        (
            ToolOutcome::Success(format!(
                "{verb} {} ({line_count} lines).",
                resolved_path.display()
            )),
            Some(content_bytes),
        )
    }
}

impl PermissableToolCall for WriteToolCall {
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
pub(crate) struct ShellToolCall {
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

        let command = value
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing command"))?;

        let shell = value
            .get("shell")
            .and_then(|v| v.as_str())
            .unwrap_or("bash")
            .to_string();

        let timeout_secs = value
            .get("timeout")
            .and_then(|v| v.as_u64())
            .filter(|&v| v > 0)
            .unwrap_or(30)
            .min(600);

        let description = value
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(ShellToolCall {
            dir: dir.map(expand_path),
            command: command.to_string(),
            shell,
            timeout_secs,
            description,
        })
    }
}

impl PermissableToolCall for ShellToolCall {
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
        crate::permissions::shell::any_subcommand_matches(&parsed.subcommands, scope)
    }
}

/// Preview viewport height for VT100 emulation.
const PREVIEW_HEIGHT: u16 = 10;

/// Default terminal width for VT100 emulation.
const PREVIEW_WIDTH: u16 = 120;

/// Normalize newlines for VT100 processing.
///
/// When subprocess output is captured via pipes (no PTY), bare `\n` (LF) bytes
/// are not translated to `\r\n` (CR+LF) the way a kernel terminal driver would
/// with the `ONLCR` flag. In VT100, LF only moves the cursor down without
/// returning to column 0. This causes lines to start at progressively higher
/// column offsets and eventually wrap, producing garbled output.
///
/// This function inserts `\r` before any `\n` that isn't already preceded by
/// `\r`, mimicking the terminal driver's ONLCR behavior.
fn normalize_newlines_for_vt100(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 8);
    for (i, &b) in data.iter().enumerate() {
        if b == b'\n' && (i == 0 || data[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

/// Extract plain text lines from a VT100 screen buffer.
///
/// Strips trailing blank lines so the result only contains rows with actual
/// content. Without this, the fixed-size VT100 screen (PREVIEW_HEIGHT rows)
/// would always return that many lines, and downstream components that use
/// tail-mode display (like the Viewport) would show the blank padding rows
/// instead of the real output.
fn vt100_screen_lines(screen: &vt100::Screen) -> Vec<String> {
    let (rows, cols) = screen.size();
    let mut lines = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        let mut line = String::with_capacity(cols as usize);
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

/// Strip ANSI escape sequences from raw bytes using a VT100 parser.
///
/// Uses a large virtual screen so scrollback is preserved, then extracts
/// the plain text contents. This handles all escape sequences (colors,
/// cursor movement, progress bars, etc.) not just simple SGR codes.
fn strip_ansi_via_vt100(raw: &[u8]) -> String {
    if raw.is_empty() {
        return String::new();
    }
    // Normalize bare LF to CR+LF so lines start at column 0 in the VT100 screen.
    let normalized = normalize_newlines_for_vt100(raw);
    // Feed bytes into a VT100 parser large enough to hold all output, then
    // read back the plain text. We estimate rows from the number of newlines
    // (not total byte length) because real output typically has short lines
    // that would be severely under-counted by a bytes÷width estimate.
    let newline_count = normalized.iter().filter(|&&b| b == b'\n').count();
    let wrap_estimate = normalized.len() / PREVIEW_WIDTH as usize;
    let estimated_rows = (newline_count + wrap_estimate + 1).min(10_000) as u16;
    let mut parser = vt100::Parser::new(estimated_rows, PREVIEW_WIDTH, 0);
    parser.process(&normalized);
    let screen = parser.screen();
    // screen.contents() returns the full plain-text content with trailing
    // whitespace trimmed per line and trailing blank lines removed.
    screen.contents()
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
pub(crate) async fn execute_shell_command_streaming(
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
                        let normalized = normalize_newlines_for_vt100(&stdout_buf[..n]);
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
                        let normalized = normalize_newlines_for_vt100(&stderr_buf[..n]);
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
    let stdout_text = strip_ansi_via_vt100(&full_stdout);
    let stderr_text = strip_ansi_via_vt100(&full_stderr);

    ToolOutcome::Structured {
        stdout: stdout_text,
        stderr: stderr_text,
        exit_code,
        duration_ms: duration.as_millis() as u64,
        interrupted,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AtuinHistoryToolCall {
    pub filter_modes: Vec<HistorySearchFilterMode>,
    pub query: String,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub(crate) enum HistorySearchFilterMode {
    Global,
    Host,
    Session,
    Directory,
    Workspace,
}

impl From<&HistorySearchFilterMode> for atuin_client::settings::FilterMode {
    fn from(mode: &HistorySearchFilterMode) -> Self {
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
        let filter_modes = value
            .get("filter_modes")
            .and_then(|v| v.as_array())
            .ok_or(eyre::eyre!("Missing filter_modes"))?;

        let filter_modes = filter_modes
            .iter()
            .map(|v| {
                let mode = v.as_str().ok_or(eyre::eyre!("Invalid filter mode"))?;
                match mode {
                    "global" => Ok(HistorySearchFilterMode::Global),
                    "host" => Ok(HistorySearchFilterMode::Host),
                    "session" => Ok(HistorySearchFilterMode::Session),
                    "directory" => Ok(HistorySearchFilterMode::Directory),
                    "workspace" => Ok(HistorySearchFilterMode::Workspace),
                    _ => Err(eyre::eyre!("Invalid filter mode: {mode}")),
                }
            })
            .collect::<Result<Vec<HistorySearchFilterMode>>>()?;

        let query = value
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing query"))?;

        let limit = value
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(10)
            .clamp(1, 50);

        Ok(AtuinHistoryToolCall {
            filter_modes,
            query: query.to_string(),
            limit,
        })
    }
}

impl PermissableToolCall for AtuinHistoryToolCall {
    fn target_dir(&self) -> Option<&Path> {
        None
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        rule.tool == "AtuinHistory"
    }
}

impl AtuinHistoryToolCall {
    pub(crate) async fn execute(&self, db: &atuin_client::database::Sqlite) -> ToolOutcome {
        use atuin_client::database::{self, Database as _, OptFilters};
        use atuin_client::settings::SearchMode;
        use time::UtcOffset;

        let context = match database::current_context().await {
            Ok(ctx) => ctx,
            Err(e) => return ToolOutcome::Error(format!("Failed to get history context: {e}")),
        };

        let filter_mode = self
            .filter_modes
            .first()
            .map(atuin_client::settings::FilterMode::from)
            .unwrap_or(atuin_client::settings::FilterMode::Global);

        let filter_options = OptFilters {
            limit: Some(self.limit),
            ..Default::default()
        };

        let results = match db
            .search(
                SearchMode::Fuzzy,
                filter_mode,
                &context,
                &self.query,
                filter_options,
            )
            .await
        {
            Ok(results) => results,
            Err(e) => return ToolOutcome::Error(format!("History search failed: {e}")),
        };

        if results.is_empty() {
            return ToolOutcome::Success("No matching history entries found.".to_string());
        }

        let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

        let formatted: Vec<String> = results
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let ts = h.timestamp.to_offset(local_offset);
                let time_str = format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    ts.year(),
                    ts.month() as u8,
                    ts.day(),
                    ts.hour(),
                    ts.minute(),
                    ts.second(),
                );

                let duration_str = format_duration(h.duration);

                format!(
                    "{}. `{}` [{}] ({}, exit: {}){}",
                    i + 1,
                    h.command,
                    time_str,
                    h.cwd,
                    h.exit,
                    duration_str,
                )
            })
            .collect();

        ToolOutcome::Success(formatted.join("\n"))
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn no_scope_matches_everything() {
        assert!(read_tool("any/path.txt").matches_rule(&read_rule(None)));
        assert!(write_tool("any/path.txt").matches_rule(&write_rule(None)));
    }

    #[test]
    fn wildcard_star_matches_everything() {
        assert!(read_tool("foo/bar.rs").matches_rule(&read_rule(Some("*"))));
    }

    #[test]
    fn write_implies_read() {
        // A Write rule also permits reads on the same path
        assert!(read_tool("foo.txt").matches_rule(&write_rule(None)));
        // But a Read rule does not permit writes
        assert!(!write_tool("foo.txt").matches_rule(&read_rule(None)));
    }

    #[test]
    fn edit_uses_write_rule() {
        let edit = EditToolCall {
            path: expand_path("/home/user/config.toml"),
            old_string: "x".into(),
            new_string: "y".into(),
            replace_all: false,
        };
        assert!(edit.matches_rule(&write_rule(None)));
        assert!(!edit.matches_rule(&read_rule(None)));
    }

    #[test]
    fn extension_glob() {
        assert!(read_tool("notes.md").matches_rule(&read_rule(Some("*.md"))));
        assert!(!read_tool("notes.txt").matches_rule(&read_rule(Some("*.md"))));
    }

    #[test]
    fn relative_multi_segment_glob() {
        // This matches against the path relative to cwd
        let cwd = std::env::current_dir().unwrap();
        let abs = cwd
            .join("crates")
            .join("atuin-ai")
            .join("src")
            .join("lib.rs");
        let tool = read_tool(abs.to_str().unwrap());
        assert!(tool.matches_rule(&read_rule(Some("crates/**/*.rs"))));
        assert!(!tool.matches_rule(&read_rule(Some("crates/**/*.py"))));
    }

    // ── Unix-specific tests (absolute paths with forward slashes) ──

    #[cfg(unix)]
    mod unix {
        use super::*;

        #[test]
        fn absolute_glob() {
            assert!(
                read_tool("/home/user/src/main.rs")
                    .matches_rule(&read_rule(Some("/home/user/src/*.rs")))
            );
            assert!(
                !read_tool("/home/user/docs/readme.md")
                    .matches_rule(&read_rule(Some("/home/user/src/*.rs")))
            );
        }

        #[test]
        fn double_star_glob() {
            assert!(
                read_tool("/project/crates/foo/src/lib.rs")
                    .matches_rule(&read_rule(Some("/project/crates/**/*.rs")))
            );
            assert!(
                !read_tool("/project/crates/foo/src/lib.py")
                    .matches_rule(&read_rule(Some("/project/crates/**/*.rs")))
            );
        }
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

        #[test]
        fn successful_single_replacement() {
            let (_dir, path, tracker) = setup_tracked_file("[section]\nkey = old_value\n");

            let call = edit_call(&path, "old_value", "new_value", false);
            let (outcome, new_bytes) = call.execute(&path, &tracker);

            assert!(matches!(outcome, ToolOutcome::Success(_)));
            assert!(new_bytes.is_some());
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "[section]\nkey = new_value\n"
            );
        }

        #[test]
        fn successful_replace_all() {
            let (_dir, path, tracker) = setup_tracked_file("aaa bbb aaa ccc aaa");

            let call = edit_call(&path, "aaa", "xxx", true);
            let (outcome, _) = call.execute(&path, &tracker);

            assert!(matches!(outcome, ToolOutcome::Success(ref s) if s.contains("3 occurrences")));
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "xxx bbb xxx ccc xxx"
            );
        }

        #[test]
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

        #[test]
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

        #[test]
        fn error_no_match() {
            let (_dir, path, tracker) = setup_tracked_file("hello world");

            let call = edit_call(&path, "nonexistent", "replacement", false);
            let (outcome, new_bytes) = call.execute(&path, &tracker);

            assert!(new_bytes.is_none());
            match outcome {
                ToolOutcome::Error(msg) => {
                    assert!(msg.contains("not found"), "got: {msg}");
                }
                _ => panic!("expected error"),
            }
        }

        #[test]
        fn error_multiple_matches_without_replace_all() {
            let (_dir, path, tracker) = setup_tracked_file("foo bar foo baz foo");

            let call = edit_call(&path, "foo", "qux", false);
            let (outcome, new_bytes) = call.execute(&path, &tracker);

            assert!(new_bytes.is_none());
            match outcome {
                ToolOutcome::Error(msg) => {
                    assert!(msg.contains("3 matches"), "got: {msg}");
                    assert!(msg.contains("replace_all"), "got: {msg}");
                }
                _ => panic!("expected error"),
            }
            // File should be unchanged
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "foo bar foo baz foo"
            );
        }

        #[test]
        fn error_empty_old_string() {
            let (_dir, path, tracker) = setup_tracked_file("content");

            let call = edit_call(&path, "", "something", false);
            let (outcome, new_bytes) = call.execute(&path, &tracker);

            assert!(new_bytes.is_none());
            assert!(matches!(outcome, ToolOutcome::Error(_)));
        }

        #[test]
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

        #[test]
        fn preserves_file_when_no_match() {
            let original = "[config]\nport = 8080\nhost = localhost\n";
            let (_dir, path, tracker) = setup_tracked_file(original);

            let call = edit_call(&path, "port = 9090", "port = 3000", false);
            let (outcome, _) = call.execute(&path, &tracker);

            assert!(matches!(outcome, ToolOutcome::Error(_)));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }

        #[test]
        fn multiline_replacement() {
            let content = "[section]\nkey1 = val1\nkey2 = val2\n[other]\n";
            let (_dir, path, tracker) = setup_tracked_file(content);

            let call = edit_call(
                &path,
                "key1 = val1\nkey2 = val2",
                "key1 = new1\nkey2 = new2",
                false,
            );
            let (outcome, new_bytes) = call.execute(&path, &tracker);

            assert!(matches!(outcome, ToolOutcome::Success(_)));
            assert!(new_bytes.is_some());
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "[section]\nkey1 = new1\nkey2 = new2\n[other]\n"
            );
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

        #[test]
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

        #[test]
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
            assert_eq!(
                std::fs::read_to_string(&file_path).unwrap(),
                "key1 = xxx\nkey2 = yyy\n"
            );
        }

        #[test]
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
            assert_eq!(
                std::fs::read_to_string(&file_path).unwrap(),
                "value = user_changed\n"
            );
        }

        #[test]
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
            let created = store
                .ensure_snapshot(&file_path, &content_before_second)
                .unwrap();
            assert!(!created); // idempotent — already snapshotted
        }

        #[test]
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

        #[test]
        fn creates_new_file() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("new_file.txt");

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

        #[test]
        fn error_file_exists_without_overwrite() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("existing.txt");
            std::fs::write(&path, "original").unwrap();

            let call = WriteToolCall {
                path: path.clone(),
                content: "new content".to_string(),
                overwrite: false,
            };
            let (outcome, new_bytes) = call.execute(&path);

            assert!(new_bytes.is_none());
            match outcome {
                ToolOutcome::Error(msg) => {
                    assert!(msg.contains("already exists"), "got: {msg}");
                    assert!(msg.contains("overwrite"), "got: {msg}");
                }
                _ => panic!("expected error"),
            }
            // Original preserved
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        }

        #[test]
        fn overwrites_existing_file_when_flag_set() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("existing.txt");
            std::fs::write(&path, "original").unwrap();

            let call = WriteToolCall {
                path: path.clone(),
                content: "replaced content\n".to_string(),
                overwrite: true,
            };
            let (outcome, new_bytes) = call.execute(&path);

            assert!(matches!(outcome, ToolOutcome::Success(_)));
            assert!(new_bytes.is_some());
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "replaced content\n"
            );
        }

        #[test]
        fn creates_parent_directories() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sub").join("dir").join("file.txt");

            let call = WriteToolCall {
                path: path.clone(),
                content: "nested\n".to_string(),
                overwrite: false,
            };
            let (outcome, _) = call.execute(&path);

            assert!(matches!(outcome, ToolOutcome::Success(_)));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested\n");
        }

        #[test]
        fn error_path_is_directory() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().to_path_buf();

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

    // ── Windows-specific tests (absolute paths with drive letters) ──

    #[cfg(windows)]
    mod windows {
        use super::*;

        #[test]
        fn absolute_glob() {
            assert!(
                read_tool(r"C:\Users\dev\src\main.rs")
                    .matches_rule(&read_rule(Some("C:/Users/dev/src/*.rs")))
            );
            assert!(
                !read_tool(r"C:\Users\dev\docs\readme.md")
                    .matches_rule(&read_rule(Some("C:/Users/dev/src/*.rs")))
            );
        }

        #[test]
        fn double_star_glob() {
            assert!(
                read_tool(r"C:\project\crates\foo\src\lib.rs")
                    .matches_rule(&read_rule(Some("C:/project/crates/**/*.rs")))
            );
            assert!(
                !read_tool(r"C:\project\crates\foo\src\lib.py")
                    .matches_rule(&read_rule(Some("C:/project/crates/**/*.rs")))
            );
        }
    }
}

fn format_duration(nanos: i64) -> String {
    if nanos <= 0 {
        return String::new();
    }

    let total_secs = nanos / 1_000_000_000;
    let millis = (nanos % 1_000_000_000) / 1_000_000;

    if total_secs >= 3600 {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;
        format!(", {hours}h{mins}m{secs}s")
    } else if total_secs >= 60 {
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!(", {mins}m{secs}s")
    } else if total_secs > 0 {
        if millis > 0 {
            format!(", {total_secs}.{millis:03}s")
        } else {
            format!(", {total_secs}s")
        }
    } else {
        format!(", {millis}ms")
    }
}
