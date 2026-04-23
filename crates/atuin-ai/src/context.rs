use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::distro::detect_linux_distribution;
use atuin_client::settings::AiCapabilities;

/// Session-scoped context for the AI chat session.
/// Holds the API configuration and client settings needed by the event loop and stream task.
#[derive(Clone, Debug)]
pub(crate) struct AppContext {
    pub endpoint: String,
    pub token: String,
    pub send_cwd: bool,
    pub last_command: Option<String>,
    pub history_db: Arc<atuin_client::database::Sqlite>,
    /// Git root of the current working directory, if inside a git repo.
    /// Resolves through worktrees to the main repo root.
    pub git_root: Option<PathBuf>,
    pub capabilities: AiCapabilities,
}

impl AppContext {
    pub(crate) fn capabilities_as_strings(&self) -> Vec<String> {
        let mut caps = vec!["client_invocations".to_string()];
        if self.capabilities.enable_history_search.unwrap_or(true) {
            caps.push("client_v1_atuin_history".to_string());
        }
        if self.capabilities.enable_file_tools.unwrap_or(true) {
            caps.push("client_v1_read_file".to_string());
            caps.push("client_v1_edit_file".to_string());
            caps.push("client_v1_write_file".to_string());
        }
        if self.capabilities.enable_command_execution.unwrap_or(true) {
            caps.push("client_v1_execute_shell_command".to_string());
        }
        caps.push("client_v1_load_skill".to_string());
        if let Ok(extra) = std::env::var("ATUIN_AI__ADDITIONAL_CAPS") {
            caps.extend(
                extra
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            );
        }
        caps
    }
}

/// Machine identity — computed once per session.
#[derive(Clone, Debug)]
pub(crate) struct ClientContext {
    pub os: String,
    pub shell: Option<String>,
    pub distro: Option<String>,
}

impl ClientContext {
    pub(crate) fn detect() -> Self {
        let os = detect_os();
        let shell = crate::commands::detect_shell();
        let distro = if os == "linux" {
            Some(detect_linux_distribution())
        } else {
            None
        };
        Self { os, shell, distro }
    }

    /// Serialize to the JSON format the API expects for the "context" field.
    /// The `pwd` field is always dynamic (current working directory), so it's
    /// computed fresh on each call if `send_cwd` is true.
    pub(crate) fn to_json(&self, send_cwd: bool, last_command: Option<&str>) -> serde_json::Value {
        let mut ctx = serde_json::json!({
            "os": self.os,
            "shell": self.shell,
            "pwd": if send_cwd {
                std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned())
            } else {
                None
            },
            "last_command": last_command,
        });

        if let Some(ref distro) = self.distro {
            ctx["distro"] = serde_json::json!(distro);
        }

        ctx
    }
}

/// Move the `detect_os` function here since it's about client identity.
fn detect_os() -> String {
    match std::env::consts::OS {
        "macos" => "macos".to_string(),
        "linux" => "linux".to_string(),
        "windows" => "windows".to_string(),
        other => format!("Other: {other}"),
    }
}
