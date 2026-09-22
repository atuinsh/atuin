use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::distro::detect_linux_distribution;
use atuin_client::history::History;
use atuin_client::settings::AiCapabilities;
use atuin_common::time::UtcOffsetExt;

use crate::tools::descriptor;

/// Session-scoped context for the AI chat session.
/// Holds the API configuration and client settings needed by the event loop and stream task.
#[derive(Clone, Debug)]
pub struct AppContext {
    pub endpoint: reqwest::Url,
    /// Bearer token for `endpoint`. Empty means unauthenticated — no
    /// Authorization header is sent (an OSS server may not require auth).
    pub token: String,
    /// Whether `endpoint` is an Atuin Hub instance. Hub endpoints report
    /// credit usage; OSS endpoints (e.g. atuin-ai-server) don't have the
    /// usage API, so usage fetching and caching are skipped.
    pub endpoint_is_hub: bool,
    /// Whether `token` came from the stored Hub session rather than
    /// `ai.api_token` or the CLI flag. Only a session-sourced token may be
    /// cleared (logging the user out) when the server rejects it.
    pub token_from_hub_session: bool,
    pub send_cwd: bool,
    pub last_command: Option<History>,
    pub history_db: Arc<atuin_client::database::Sqlite>,
    /// Git root of the current working directory, if inside a git repo.
    /// Resolves through worktrees to the main repo root.
    pub git_root: Option<PathBuf>,
    pub capabilities: AiCapabilities,
    pub daemon_enabled: bool,
    pub yolo: bool,
}

pub fn history_output_capability_available(daemon_enabled: bool) -> bool {
    cfg!(feature = "daemon") && daemon_enabled
}

/// The `client_v1_*` capability strings this client sends with every AI
/// request to the Hub (`ai.endpoint`), which runs the model and sends back
/// tool calls. The Hub reads the list to decide which tools it may ask this
/// client to run: a tool whose capability string is missing here is never
/// offered to the model. Requests are built in more than one place, but they
/// all get their capabilities from this function — separately maintained
/// lists could drift apart, and the model would then see different tools
/// depending on how the user launched the AI.
pub fn capability_strings(capabilities: &AiCapabilities, daemon_enabled: bool) -> Vec<String> {
    // Each tool's capability string lives on its ToolDescriptor. Reuse it
    // rather than retyping the string: the fsm later accepts or rejects the
    // server's tool calls by comparing against those same descriptors, so a
    // mismatched literal here would silently disable a tool.
    let cap = |d: &descriptor::ToolDescriptor| {
        d.capability.expect("client-side tools declare a capability").to_string()
    };
    let mut caps = vec!["client_invocations".to_string(), cap(descriptor::LOAD_SKILL)];
    if capabilities.enable_history_search.unwrap_or(true) {
        caps.push(cap(descriptor::ATUIN_HISTORY));
        caps.push(descriptor::ATUIN_HISTORY_V2_CAPABILITY.to_string());
    }
    if capabilities.enable_file_tools.unwrap_or(true) {
        caps.push(cap(descriptor::READ));
        caps.push(cap(descriptor::EDIT));
        caps.push(cap(descriptor::WRITE));
    }
    if capabilities.enable_command_execution.unwrap_or(true) {
        caps.push(cap(descriptor::SHELL));
    }
    if history_output_capability_available(daemon_enabled)
        && capabilities.enable_history_output.unwrap_or(true)
    {
        caps.push(cap(descriptor::ATUIN_OUTPUT));
    }
    if let Ok(extra) = std::env::var("ATUIN_AI__ADDITIONAL_CAPS") {
        caps.extend(extra.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    }
    caps
}

impl AppContext {
    pub(crate) fn capabilities_as_strings(&self) -> Vec<String> {
        capability_strings(&self.capabilities, self.daemon_enabled)
    }
}

/// Machine identity — computed once per session.
#[derive(Clone, Debug)]
pub struct ClientContext {
    pub os: String,
    pub shell: Option<String>,
    pub distro: Option<String>,
}

impl ClientContext {
    pub(crate) fn detect() -> Self {
        let os = detect_os();
        let shell = Some(crate::commands::detect_shell());
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
    pub(crate) fn to_json(
        &self,
        send_cwd: bool,
        last_command: Option<&History>,
    ) -> serde_json::Value {
        let mut ctx = serde_json::json!({
            "os": self.os,
            "shell": self.shell,
            "pwd": if send_cwd {
                std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned())
            } else {
                None
            },
        });

        if let Some(history) = last_command {
            ctx["last_command"] = serde_json::json!(crate::history_format::format_last_command(
                history,
                time::UtcOffset::local_or_utc(),
            ));
        }

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
