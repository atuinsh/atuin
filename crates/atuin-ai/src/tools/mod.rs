use std::path::{Path, PathBuf};

use eyre::Result;

use crate::permissions::rule::Rule;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolCallState {
    CheckingPermissions,
    AskingForPermission,
    Denied(String),
    Executing,
}

pub(crate) enum ClientToolCallType {
    Read,
    Write,
    Shell,
    AtuinHistory,
}

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
            "read" => Ok(ClientToolCall::Read(ReadToolCall::try_from(input)?)),
            "write" => Ok(ClientToolCall::Write(WriteToolCall::try_from(input)?)),
            "shell" => Ok(ClientToolCall::Shell(ShellToolCall::try_from(input)?)),
            "atuin_history" => Ok(ClientToolCall::AtuinHistory(
                AtuinHistoryToolCall::try_from(input)?,
            )),
            _ => Err(eyre::eyre!("Unknown tool call: {name}")),
        }
    }
}

impl ClientToolCall {
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
}

pub(crate) trait PermissableToolCall {
    fn matches_rule(&self, rule: &Rule) -> bool;
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
}

impl TryFrom<&serde_json::Value> for ReadToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let path = value
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or(eyre::eyre!("Missing path"))?;

        Ok(ReadToolCall {
            path: PathBuf::from(path),
        })
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
}

#[derive(Debug, Clone)]
pub(crate) enum HistorySearchFilterMode {
    Global,
    Host,
    Session,
    Directory,
    Workspace,
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

        Ok(AtuinHistoryToolCall {
            filter_modes,
            query: query.to_string(),
        })
    }
}

impl PermissableToolCall for AtuinHistoryToolCall {
    fn target_dir(&self) -> Option<&Path> {
        None
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        if rule.tool != "AtuinHistory" {
            return false;
        }

        todo!()
    }
}
