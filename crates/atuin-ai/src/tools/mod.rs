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
    pub fn format_for_llm(&self) -> String {
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
                    parts.push("[Interrupted by user]".to_string());
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
    pub interrupted: bool,
}

/// Lifecycle phase of a tracked tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolPhase {
    CheckingPermissions,
    AskingForPermission,
    #[expect(dead_code)]
    Denied(String),
    #[expect(dead_code)]
    Executing,
    /// Shell command is executing with live preview output.
    ExecutingWithPreview {
        command: String,
        /// Current VT100 screen lines (plain text, viewport-sized).
        output_lines: Vec<String>,
        /// Exit code once the process completes.
        exit_code: Option<i32>,
        /// Whether the command was interrupted by the user.
        interrupted: bool,
    },
    /// Tool execution has completed. Preview is cached for rendering history.
    Completed {
        preview: Option<ToolPreview>,
    },
}

/// A tracked tool call through its full lifecycle.
#[derive(Debug)]
pub(crate) struct TrackedTool {
    pub id: String,
    pub tool: ClientToolCall,
    pub phase: ToolPhase,
    /// Sender to interrupt a running shell command (only set during ExecutingWithPreview).
    pub abort_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TrackedTool {
    pub(crate) fn target_dir(&self) -> Option<&Path> {
        self.tool.target_dir()
    }

    pub fn mark_asking(&mut self) {
        self.phase = ToolPhase::AskingForPermission;
    }

    pub fn mark_executing_preview(&mut self, command: String) {
        self.phase = ToolPhase::ExecutingWithPreview {
            command,
            output_lines: Vec::new(),
            exit_code: None,
            interrupted: false,
        };
    }

    pub fn complete(&mut self, preview: Option<ToolPreview>) {
        self.phase = ToolPhase::Completed { preview };
        self.abort_tx = None;
    }

    /// Extract the current preview, whether live or completed.
    pub fn preview(&self) -> Option<ToolPreview> {
        match &self.phase {
            ToolPhase::ExecutingWithPreview {
                output_lines,
                exit_code,
                interrupted,
                ..
            } => Some(ToolPreview {
                lines: output_lines.clone(),
                exit_code: *exit_code,
                interrupted: *interrupted,
            }),
            ToolPhase::Completed { preview } => preview.clone(),
            _ => None,
        }
    }
}

/// Tracks all tool calls through their full lifecycle.
///
/// Single source of truth for tool execution state. Entries persist after
/// completion so cached previews remain available for rendering history.
#[derive(Debug)]
pub(crate) struct ToolTracker {
    tools: Vec<TrackedTool>,
}

impl ToolTracker {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Insert a new tool call in CheckingPermissions phase.
    pub fn insert(&mut self, id: String, tool: ClientToolCall) {
        self.tools.push(TrackedTool {
            id,
            tool,
            phase: ToolPhase::CheckingPermissions,
            abort_tx: None,
        });
    }

    pub fn get(&self, id: &str) -> Option<&TrackedTool> {
        self.tools.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut TrackedTool> {
        self.tools.iter_mut().find(|t| t.id == id)
    }

    /// Remove a tool by ID and return it.
    #[expect(dead_code)]
    pub fn remove(&mut self, id: &str) -> Option<TrackedTool> {
        let pos = self.tools.iter().position(|t| t.id == id)?;
        Some(self.tools.remove(pos))
    }

    /// True if any tool is still awaiting a permission decision.
    #[expect(dead_code)]
    pub fn has_unresolved(&self) -> bool {
        self.tools.iter().any(|t| {
            matches!(
                t.phase,
                ToolPhase::CheckingPermissions | ToolPhase::AskingForPermission
            )
        })
    }

    /// True if any tool has not yet reached the Completed phase.
    /// Use this to gate `ContinueAfterTools` — we must wait for all tools
    /// (including those still executing) before resuming the conversation.
    pub fn has_pending(&self) -> bool {
        self.tools
            .iter()
            .any(|t| !matches!(t.phase, ToolPhase::Completed { .. }))
    }

    /// True if any tool is currently executing with a preview.
    pub fn has_executing_preview(&self) -> bool {
        self.tools
            .iter()
            .any(|t| matches!(t.phase, ToolPhase::ExecutingWithPreview { .. }))
    }

    /// Find the first tool that is asking for permission.
    pub fn asking_for_permission(&self) -> Option<&TrackedTool> {
        self.tools
            .iter()
            .find(|t| t.phase == ToolPhase::AskingForPermission)
    }

    /// Find the first tool that is asking for permission (mutable).
    #[expect(dead_code)]
    pub fn asking_for_permission_mut(&mut self) -> Option<&mut TrackedTool> {
        self.tools
            .iter_mut()
            .find(|t| t.phase == ToolPhase::AskingForPermission)
    }

    /// Get the preview for a tool by ID (live or cached).
    pub fn preview_for(&self, id: &str) -> Option<ToolPreview> {
        self.get(id)?.preview()
    }

    /// Iterate mutably over all tracked tools.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut TrackedTool> {
        self.tools.iter_mut()
    }
}

/// A tool call from the server, with parsed input parameters.
#[derive(Debug, Clone)]
pub(crate) enum ClientToolCall {
    Read(ReadToolCall),
    Write(WriteToolCall),
    Shell(ShellToolCall),
    AtuinHistory(AtuinHistoryToolCall),
}

impl TryFrom<(&str, &serde_json::Value)> for ClientToolCall {
    type Error = eyre::Error;

    fn try_from((name, input): (&str, &serde_json::Value)) -> Result<Self, Self::Error> {
        match name {
            "read_file" => Ok(ClientToolCall::Read(ReadToolCall::try_from(input)?)),
            "create_file" => Ok(ClientToolCall::Write(WriteToolCall::try_from(input)?)),
            // "append_to_file" => Ok(ClientToolCall::Append(AppendToolCall::try_from(input)?)),
            // "str_replace" => Ok(ClientToolCall::StrReplace(StrReplaceToolCall::try_from(input)?)),
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
            ClientToolCall::Write(_) => descriptor::WRITE,
            ClientToolCall::Shell(_) => descriptor::SHELL,
            ClientToolCall::AtuinHistory(_) => descriptor::ATUIN_HISTORY,
        }
    }

    /// The permission rule name for this tool category (e.g. "Write" covers
    /// str_replace, file_create, file_insert).
    pub(crate) fn rule_name(&self) -> &'static str {
        match self {
            ClientToolCall::Read(_) => "Read",
            ClientToolCall::Write(_) => "Write",
            ClientToolCall::Shell(_) => "Shell",
            ClientToolCall::AtuinHistory(_) => "AtuinHistory",
        }
    }

    pub(crate) fn matches_rule(&self, rule: &Rule) -> bool {
        match self {
            ClientToolCall::Read(tool) => tool.matches_rule(rule),
            ClientToolCall::Write(tool) => tool.matches_rule(rule),
            ClientToolCall::Shell(tool) => tool.matches_rule(rule),
            ClientToolCall::AtuinHistory(tool) => tool.matches_rule(rule),
        }
    }

    pub(crate) fn target_dir(&self) -> Option<&Path> {
        match self {
            ClientToolCall::Read(tool) => tool.target_dir(),
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
            path: PathBuf::from(path),
            offset,
            limit,
        })
    }
}

impl ReadToolCall {
    fn execute(&self) -> ToolOutcome {
        let mut path = self.path.clone();

        if path.is_relative()
            && let Ok(current_dir) = std::env::current_dir()
        {
            path = current_dir.join(path);
        }

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

        let relevent_lines = reader
            .lines()
            .skip(self.offset as usize)
            .take(self.limit as usize)
            .collect::<Result<Vec<_>, _>>();

        match relevent_lines {
            Ok(lines) => {
                let joined = lines.join("\n");
                if joined.len() > 100_000 {
                    ToolOutcome::Error(format!(
                        "Error: file is too large to read ({} bytes in {} lines); use view_range to read a subset of the file",
                        joined.len(),
                        lines.len()
                    ))
                } else {
                    ToolOutcome::Success(joined)
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
        if rule.tool != "Read" {
            return false;
        }

        match rule.scope.as_deref() {
            None | Some("*") => true,
            Some(scope) => path_matches_scope(&self.path, scope),
        }
    }
}

#[derive(Debug, Clone)]
#[expect(dead_code)]
pub(crate) struct WriteToolCall {
    pub path: PathBuf,
    pub content: String,
}

impl TryFrom<&serde_json::Value> for WriteToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let path = value
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing path"))?;

        let content = value
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing content"))?;

        Ok(WriteToolCall {
            path: PathBuf::from(path),
            content: content.to_string(),
        })
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

        Ok(ShellToolCall {
            dir: dir.map(PathBuf::from),
            command: command.to_string(),
            shell,
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

/// Extract plain text lines from a VT100 screen buffer.
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
        // Trim trailing whitespace for cleaner display
        lines.push(line.trim_end().to_string());
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
    // Use the contents_formatted → screen approach: feed bytes into a parser
    // with enough rows to hold everything, then read back the plain text.
    // Estimate rows: one row per ~PREVIEW_WIDTH bytes, plus generous padding.
    let estimated_rows = (raw.len() / PREVIEW_WIDTH as usize + 1).min(10_000) as u16;
    let mut parser = vt100::Parser::new(estimated_rows, PREVIEW_WIDTH, 0);
    parser.process(raw);
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
                        parser.process(&stdout_buf[..n]);
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
                        parser.process(&stderr_buf[..n]);
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
            path: PathBuf::from(path),
            offset: 0,
            limit: 100,
        }
    }

    fn write_tool(path: &str) -> WriteToolCall {
        WriteToolCall {
            path: PathBuf::from(path),
            content: String::new(),
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
    fn wrong_tool_never_matches() {
        assert!(!read_tool("foo.txt").matches_rule(&write_rule(None)));
        assert!(!write_tool("foo.txt").matches_rule(&read_rule(None)));
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
