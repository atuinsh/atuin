use std::{
    io::BufRead,
    path::{Path, PathBuf},
};

use eyre::Result;

pub(crate) mod descriptor;

use crate::permissions::rule::Rule;

/// Result of executing a client-side tool.
pub(crate) enum ToolOutcome {
    Success(String),
    Error(String),
}

/// A pending tool call from the server, awaiting permissions or execution.
#[derive(Debug, Clone)]
pub(crate) struct PendingToolCall {
    pub id: String,
    pub state: ToolCallState,
    pub tool: ClientToolCall,
}

impl PendingToolCall {
    pub(crate) fn target_dir(&self) -> Option<&Path> {
        self.tool.target_dir()
    }

    /// Mark this tool call as waiting for user permission.
    pub fn mark_asking(&mut self) {
        self.state = ToolCallState::AskingForPermission;
    }

    /// Mark this tool call as currently executing.
    #[expect(dead_code)]
    pub fn mark_executing(&mut self) {
        self.state = ToolCallState::Executing;
    }

    /// Mark this tool call as denied.
    #[expect(dead_code)]
    pub fn mark_denied(&mut self, reason: String) {
        self.state = ToolCallState::Denied(reason);
    }
}

/// State of a pending tool call
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolCallState {
    CheckingPermissions,
    AskingForPermission,
    Denied(String),
    Executing,
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
            "shell" => Ok(ClientToolCall::Shell(ShellToolCall::try_from(input)?)),
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
}

impl TryFrom<&serde_json::Value> for ShellToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let dir = value.get("dir").and_then(|v| v.as_str());

        let command = value
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing command"))?;

        Ok(ShellToolCall {
            dir: dir.map(PathBuf::from),
            command: command.to_string(),
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

        if let Some(scope) = rule.scope.as_ref() {
            if scope == "*" {
                return true;
            }

            todo!("split command into subcommands, check each");
        }

        true
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
