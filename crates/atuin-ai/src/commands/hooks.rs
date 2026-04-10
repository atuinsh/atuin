use std::io::Read;
use std::path::PathBuf;

use atuin_common::utils::home_dir;
use eyre::{Result, bail};
use serde_json::Value;

enum Agent {
    ClaudeCode,
    Codex,
}

impl Agent {
    fn from_name(name: &str) -> Result<Self> {
        match name {
            "claude-code" | "claude" => Ok(Self::ClaudeCode),
            "codex" => Ok(Self::Codex),
            _ => bail!("unknown agent: {name}. Supported agents: claude-code, codex"),
        }
    }

    fn actor_name(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    fn config_path(&self) -> PathBuf {
        let home = home_dir();
        match self {
            Self::ClaudeCode => home.join(".claude").join("settings.json"),
            Self::Codex => home.join(".codex").join("hooks.json"),
        }
    }

    fn hook_command(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "atuin ai hook claude-code",
            Self::Codex => "atuin ai hook codex",
        }
    }
}

#[derive(Debug)]
enum HookEvent {
    Start {
        command: String,
        intent: Option<String>,
        tool_use_id: String,
    },
    End {
        tool_use_id: String,
        exit: i64,
    },
    Skip,
}

fn parse_hook_stdin(input: &str) -> Result<HookEvent> {
    let v: Value = serde_json::from_str(input)?;

    if v.get("tool_name").and_then(|t| t.as_str()) != Some("Bash") {
        return Ok(HookEvent::Skip);
    }

    let tool_use_id = match v.get("tool_use_id").and_then(|t| t.as_str()) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return Ok(HookEvent::Skip),
    };

    match v.get("hook_event_name").and_then(|e| e.as_str()) {
        Some("PreToolUse") => {
            let tool_input = v.get("tool_input");
            let command = tool_input
                .and_then(|ti| ti.get("command"))
                .and_then(|c| c.as_str())
                .unwrap_or("");

            if command.is_empty() {
                return Ok(HookEvent::Skip);
            }

            let intent = tool_input
                .and_then(|ti| ti.get("description"))
                .and_then(|d| d.as_str())
                .map(String::from);

            Ok(HookEvent::Start {
                command: command.to_string(),
                intent,
                tool_use_id,
            })
        }
        Some(event @ ("PostToolUse" | "PostToolUseFailure")) => {
            let exit = if event == "PostToolUseFailure" {
                1
            } else {
                v.get("tool_response")
                    .and_then(|tr| tr.get("exitCode"))
                    .and_then(|e| e.as_i64())
                    .unwrap_or(0)
            };

            Ok(HookEvent::End { tool_use_id, exit })
        }
        _ => Ok(HookEvent::Skip),
    }
}

fn id_file_path(tool_use_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("atuin-hook-{tool_use_id}"))
}

pub async fn handle(agent_name: &str, _settings: &atuin_client::settings::Settings) -> Result<()> {
    let agent = Agent::from_name(agent_name)?;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    if input.trim().is_empty() {
        return Ok(());
    }

    let event = parse_hook_stdin(&input)?;
    let atuin_bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("atuin"));

    match event {
        HookEvent::Start {
            command,
            intent,
            tool_use_id,
        } => {
            let actor = agent.actor_name();
            let mut args = vec!["history", "start", "--author", actor];

            if let Some(ref i) = intent {
                args.extend(["--intent", i.as_str()]);
            }

            args.extend(["--", &command]);

            let output = tokio::process::Command::new(&atuin_bin)
                .args(&args)
                .output()
                .await?;

            let history_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !history_id.is_empty() {
                std::fs::write(id_file_path(&tool_use_id), &history_id)?;
            }
        }
        HookEvent::End { tool_use_id, exit } => {
            let id_path = id_file_path(&tool_use_id);

            if let Ok(history_id) = std::fs::read_to_string(&id_path) {
                let history_id = history_id.trim();
                if !history_id.is_empty() {
                    let exit_str = exit.to_string();
                    let _ = tokio::process::Command::new(&atuin_bin)
                        .args(["history", "end", "--exit", &exit_str, "--", history_id])
                        .output()
                        .await;
                }
                let _ = std::fs::remove_file(&id_path);
            }
        }
        HookEvent::Skip => {}
    }

    Ok(())
}

pub async fn install(agent_name: &str) -> Result<()> {
    let agent = Agent::from_name(agent_name)?;
    let config_path = agent.config_path();

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut root: Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content)?
    } else {
        Value::Object(serde_json::Map::new())
    };

    let hooks = root
        .as_object_mut()
        .ok_or_else(|| eyre::eyre!("config is not a JSON object"))?
        .entry("hooks")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    add_hook_entries(hooks, &agent)?;

    let content = serde_json::to_string_pretty(&root)?;
    std::fs::write(&config_path, content)?;

    eprintln!(
        "\nAtuin hooks installed for {}. Config: {}",
        agent.actor_name(),
        config_path.display()
    );

    Ok(())
}

/// Shared logic: add PreToolUse + PostToolUse entries to a hooks object.
fn add_hook_entries(hooks: &mut Value, agent: &Agent) -> Result<()> {
    let hook_command = agent.hook_command();
    let matcher_str = match agent {
        Agent::ClaudeCode => "Bash",
        Agent::Codex => "^Bash$", // Codex uses regex matchers
    };

    for event_type in &["PreToolUse", "PostToolUse"] {
        let event_hooks = hooks
            .as_object_mut()
            .ok_or_else(|| eyre::eyre!("hooks is not a JSON object"))?
            .entry(*event_type)
            .or_insert_with(|| Value::Array(Vec::new()));

        let arr = event_hooks
            .as_array_mut()
            .ok_or_else(|| eyre::eyre!("hooks.{event_type} is not an array"))?;

        let already_installed = arr.iter().any(|entry| {
            entry["hooks"].as_array().is_some_and(|h| {
                h.iter()
                    .any(|hook| hook["command"].as_str() == Some(hook_command))
            })
        });

        if already_installed {
            eprintln!("hooks.{event_type}: already installed, skipping");
            continue;
        }

        arr.push(serde_json::json!({
            "matcher": matcher_str,
            "hooks": [{"type": "command", "command": hook_command}]
        }));
        eprintln!("hooks.{event_type}: installed atuin hook");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pre_tool_use() {
        let input = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello", "description": "Test greeting"},
            "tool_use_id": "toolu_abc123",
            "session_id": "sess1",
            "cwd": "/tmp"
        }"#;

        match parse_hook_stdin(input).unwrap() {
            HookEvent::Start {
                command,
                intent,
                tool_use_id,
            } => {
                assert_eq!(command, "echo hello");
                assert_eq!(intent.as_deref(), Some("Test greeting"));
                assert_eq!(tool_use_id, "toolu_abc123");
            }
            _ => panic!("expected Start event"),
        }
    }

    #[test]
    fn test_parse_post_tool_use() {
        let input = r#"{
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello"},
            "tool_response": {"exitCode": 0},
            "tool_use_id": "toolu_abc123"
        }"#;

        match parse_hook_stdin(input).unwrap() {
            HookEvent::End { tool_use_id, exit } => {
                assert_eq!(tool_use_id, "toolu_abc123");
                assert_eq!(exit, 0);
            }
            _ => panic!("expected End event"),
        }
    }

    #[test]
    fn test_parse_non_bash_tool_skipped() {
        let input = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/test.txt", "content": "hello"},
            "tool_use_id": "toolu_abc123"
        }"#;

        assert!(matches!(parse_hook_stdin(input).unwrap(), HookEvent::Skip));
    }

    #[test]
    fn test_parse_failure_event() {
        let input = r#"{
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": {"command": "false"},
            "tool_use_id": "toolu_abc123"
        }"#;

        match parse_hook_stdin(input).unwrap() {
            HookEvent::End { exit, .. } => assert_eq!(exit, 1),
            _ => panic!("expected End event"),
        }
    }
}
