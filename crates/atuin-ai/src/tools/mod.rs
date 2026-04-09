use std::{
    io::BufRead,
    path::{Path, PathBuf},
    time::Duration,
};

use eyre::Result;

pub(crate) mod descriptor;

use crate::permissions::rule::Rule;

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
    Denied(String),
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
    pub fn remove(&mut self, id: &str) -> Option<TrackedTool> {
        let pos = self.tools.iter().position(|t| t.id == id)?;
        Some(self.tools.remove(pos))
    }

    /// True if any tool is still in CheckingPermissions or AskingForPermission.
    pub fn has_unresolved(&self) -> bool {
        self.tools.iter().any(|t| {
            matches!(
                t.phase,
                ToolPhase::CheckingPermissions | ToolPhase::AskingForPermission
            )
        })
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
            // TODO: split these into separate tool calls, but rely on Write permissions for all
            "str_replace" => Ok(ClientToolCall::Write(WriteToolCall::try_from(input)?)),
            "file_create" => Ok(ClientToolCall::Write(WriteToolCall::try_from(input)?)),
            "file_insert" => Ok(ClientToolCall::Write(WriteToolCall::try_from(input)?)),
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
    pub view_range: Option<(u64, u64)>,
}

impl TryFrom<&serde_json::Value> for ReadToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let path = value
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing path"))?;

        let view_range = value.get("view_range").and_then(|v| v.as_array());

        let is_proper_size = view_range
            .map(|arr| arr.len() == 2 && arr.iter().all(|v| v.is_u64()))
            .unwrap_or(true);

        if !is_proper_size {
            return Err(eyre::eyre!(
                "Invalid view_range: must be an array of two integers"
            ));
        }

        let view_range = view_range.map(|arr| {
            // SAFETY: already checked that the array has two elements and they are both u64
            let start = arr[0].as_u64().unwrap();
            let end = arr[1].as_u64().unwrap();
            (start, end)
        });

        Ok(ReadToolCall {
            path: PathBuf::from(path),
            view_range,
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

        let relevent_lines = if let Some((start, end)) = self.view_range {
            reader
                .lines()
                .skip(start as usize)
                .take((end - start) as usize)
                .collect::<Result<Vec<_>, _>>()
        } else {
            reader.lines().collect::<Result<Vec<_>, _>>()
        };

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

        if let Some(scope) = rule.scope.as_ref() {
            if scope == "*" {
                return true;
            }

            todo!("check path vs scope glob");
        }

        true
    }
}

#[derive(Debug, Clone)]
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

        if let Some(scope) = rule.scope.as_ref() {
            if scope == "*" {
                return true;
            }

            todo!("check path vs scope glob");
        }

        true
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

    // Strip ANSI from the raw bytes for clean LLM output
    let stdout_text = String::from_utf8_lossy(&full_stdout).to_string();
    let stderr_text = String::from_utf8_lossy(&full_stderr).to_string();

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
