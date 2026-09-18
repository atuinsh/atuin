//! Shared install logic for harnesses that register atuin through a Claude-Code-style JSON hook
//! config.
//!
//! TODO(markovejnovic): This is a little bit of legacy slop and should be revisited.

use std::path::Path;

use serde_json::{Value, json};

use super::InstallHookError;

/// Hook lifecycle events atuin registers itself for.
const HOOK_EVENT_TYPES: &[&str] = &["PreToolUse", "PostToolUse", "PostToolUseFailure"];

/// Merge atuin's command hooks into a Claude-Code-style JSON hook config at `config_path`, leaving
/// the user's existing keys, values, and ordering intact.
pub async fn install(
    config_path: &Path,
    matcher: &str,
    hook_command: &str,
) -> Result<(), InstallHookError> {
    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut root: Value = match tokio::fs::read_to_string(config_path).await {
        Ok(content) => serde_json::from_str(&content)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Value::Object(serde_json::Map::new())
        }
        Err(err) => return Err(err.into()),
    };

    let hooks = root
        .as_object_mut()
        .ok_or(InstallHookError::Malformed("root is not an object"))?
        .entry("hooks")
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or(InstallHookError::Malformed("`hooks` is not an object"))?;

    let mut installed_any = false;
    for event_type in HOOK_EVENT_TYPES {
        let entries = hooks
            .entry(*event_type)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or(InstallHookError::Malformed("a hook event is not an array"))?;

        let already_installed = entries.iter().any(|entry| {
            entry.get("hooks").and_then(Value::as_array).is_some_and(|hooks| {
                hooks
                    .iter()
                    .any(|hook| hook.get("command").and_then(Value::as_str) == Some(hook_command))
            })
        });

        if already_installed {
            continue;
        }

        entries.push(json!({
            "matcher": matcher,
            "hooks": [{"type": "command", "command": hook_command}],
        }));
        installed_any = true;
    }

    if !installed_any {
        return Err(InstallHookError::AlreadyInstalled);
    }

    tokio::fs::write(config_path, serde_json::to_string_pretty(&root)?).await?;

    Ok(())
}
