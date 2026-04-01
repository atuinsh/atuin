use std::path::{Path, PathBuf};

use eyre::Result;

use crate::permissions::rule::Rule;

pub(crate) enum ToolCall {
    Read(ReadToolCall),
    Write(WriteToolCall),
    Shell(ShellToolCall),
    AtuinHistory(AtuinHistoryToolCall),
}

impl TryFrom<(&str, &serde_json::Value)> for ToolCall {
    type Error = eyre::Error;

    fn try_from((name, input): (&str, &serde_json::Value)) -> Result<Self, Self::Error> {
        match name {
            "read" => Ok(ToolCall::Read(ReadToolCall::try_from(input)?)),
            "write" => Ok(ToolCall::Write(WriteToolCall::try_from(input)?)),
            "shell" => Ok(ToolCall::Shell(ShellToolCall::try_from(input)?)),
            "atuin_history" => Ok(ToolCall::AtuinHistory(AtuinHistoryToolCall::try_from(
                input,
            )?)),
            _ => Err(eyre::eyre!("Unknown tool call: {name}")),
        }
    }
}

pub(crate) trait PermissableToolCall {
    fn matches_rule(&self, rule: &Rule) -> bool;
    fn target_dir(&self) -> Option<&Path> {
        None
    }
}

pub(crate) struct ReadToolCall {
    path: PathBuf,
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

pub(crate) struct WriteToolCall {
    path: PathBuf,
    content: String,
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

pub(crate) struct ShellToolCall {
    dir: Option<PathBuf>,
    command: String,
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

pub(crate) struct AtuinHistoryToolCall {
    filter_modes: Vec<HistorySearchFilterMode>,
    query: String,
}

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
