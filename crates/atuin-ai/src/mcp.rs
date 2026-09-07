//! An MCP (Model Context Protocol) server over stdio, built on the official
//! `rmcp` SDK.
//!
//! This exposes the same history tools the AI assistant uses (`atuin_history`
//! and `atuin_output`) to external MCP clients such as Claude Code or Cursor.
//!
//! History search reads the sqlite database directly and works without the
//! daemon; output retrieval talks to the daemon and returns a tool error when
//! it is not running.

use std::sync::LazyLock;

use atuin_client::database::Sqlite;
use atuin_client::history::{AUTHOR_FILTER_ALL_AGENT, AUTHOR_FILTER_ALL_USER, KNOWN_AGENTS};
use eyre::Result;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ErrorData, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use strum::IntoEnumIterator;

use crate::tools::{
    AtuinHistoryToolCall, AtuinOutputToolCall, DEFAULT_HISTORY_RESULTS, HistorySearchFilterMode,
    MAX_HISTORY_RESULTS, ToolOutcome,
};

struct AtuinMcp {
    db: Sqlite,
}

/// Server-level instructions, surfaced by MCP clients (Claude Code injects
/// them into the model's system prompt). This is the main lever for making
/// agents reach for Atuin instead of guessing: it frames the history as the
/// ground truth for "what actually happened in the terminal".
const SERVER_INSTRUCTIONS: &str =
    "\
Atuin is the ground truth for what actually happened in the user's terminal. It records shell \
     history from every session and machine: each command with its timestamp, working directory, \
     exit code, and duration — including commands run by AI agents (tagged with the agent's name \
     and intent) — and, where output capture is enabled, the full terminal output of each command.

Search atuin_history instead of guessing, asking the user, or re-running things whenever past \
     terminal activity is relevant: how the user last invoked something ('what flags did I use'), \
     whether and when something ran and if it succeeded, why a command failed (search with \
     only_failed: true, then read the actual error with atuin_output), or what an AI agent ran. \
     When debugging, checking recent history early often reveals what the user already tried.

When a question is about the user themselves — 'what do I use', 'how do I connect', 'what's my \
     setup' — run one atuin_history search BEFORE searching the filesystem. It is a single cheap \
     call, and 'it is not in the repository' is not an answer: personal habits live in shell \
     history, not in checked-in config.

Do not use `history`, ~/.bash_history, or ~/.zsh_history: they are typically empty or stale in \
     non-interactive shells and lack exit codes and output. Atuin is the reliable source. Prefer \
     atuin_output over re-running an expensive or side-effectful command just to see its output \
     again.";

/// The initialize result, separated from the handler so tests can assert on
/// it without constructing a database-backed server.
fn server_info() -> ServerInfo {
    ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation::new("atuin", env!("CARGO_PKG_VERSION")))
        .with_instructions(SERVER_INSTRUCTIONS)
}

impl ServerHandler for AtuinMcp {
    fn get_info(&self) -> ServerInfo {
        server_info()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(TOOLS.clone()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let outcome = match request.name.as_ref() {
            "atuin_history" => {
                AtuinHistoryToolCall::try_from(&arguments)
                    .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
                    .execute(&self.db)
                    .await
            }
            "atuin_output" => {
                AtuinOutputToolCall::try_from(&arguments)
                    .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
                    .execute()
                    .await
            }
            name => {
                return Err(ErrorData::invalid_params(format!("unknown tool: {name}"), None));
            }
        };

        Ok(match outcome {
            ToolOutcome::Success(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            ToolOutcome::Error(text) => CallToolResult::error(vec![ContentBlock::text(text)]),
            // The atuin tools only produce Success/Error; fall back to the
            // generic formatting should that ever change.
            outcome @ ToolOutcome::Structured { .. } => {
                CallToolResult::success(vec![ContentBlock::text(outcome.format_for_llm(None))])
            }
        })
    }
}

/// Serve MCP over stdio until the client disconnects.
///
/// stdout carries only JSON-RPC messages; anything else (logs, errors) must
/// go to stderr or it will corrupt the protocol stream.
pub async fn run(db: &Sqlite) -> Result<()> {
    let server = AtuinMcp { db: db.clone() }.serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}

/// Tool metadata for `tools/list`, built once: the schemas and descriptions
/// are assembled from consts and the filter-mode enum, none of which change
/// at runtime. The input schemas mirror what the `TryFrom<&serde_json::Value>`
/// impls in [`crate::tools`] accept.
static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(tool_definitions);

fn tool_definitions() -> Vec<Tool> {
    let Value::Object(history_schema) = json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Fuzzy search query matched against past commands. \
                    Prefer a few distinctive terms (e.g. 'ffmpeg av1'), not a \
                    sentence; terms are AND-ed. An empty string returns the most \
                    recent commands. Supports fzf-style operators per \
                    space-separated term: ^prefix, suffix$, 'exact-substring, \
                    !negate, and r/regex/.",
            },
            "filter_modes": {
                "type": "array",
                "items": {
                    "type": "string",
                    "enum": HistorySearchFilterMode::iter().map(|m| m.as_str()).collect::<Vec<_>>(),
                },
                "description": "Optional search scope; the first entry is used and \
                    the default is 'global' (all history). 'workspace' limits to \
                    commands run inside the current git repository, 'directory' to \
                    the exact current working directory, 'host' to this machine, \
                    'session' to the shell session that launched this server \
                    (errors when it was not launched from a shell). Start global \
                    and narrow only if results are noisy.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_HISTORY_RESULTS,
                "default": DEFAULT_HISTORY_RESULTS,
                "description": "Maximum number of results.",
            },
            "only_failed": {
                "type": "boolean",
                "default": false,
                "description": "Only return commands that recorded a non-zero exit \
                    code. Commands still running (no exit recorded yet) are excluded.",
            },
            "authors": {
                "type": "array",
                "items": { "type": "string" },
                "description": format!(
                    "Filter by who ran the command: '{AUTHOR_FILTER_ALL_USER}' for \
                     human-run commands, '{AUTHOR_FILTER_ALL_AGENT}' for commands run \
                     by AI agents, or any literal author name (well-known agents: {}). \
                     Multiple entries are OR-ed. Omit for everything.",
                    KNOWN_AGENTS.join(", ")
                ),
            },
        },
        "required": ["query"],
    }) else {
        unreachable!()
    };

    let Value::Object(output_schema) = json!({
        "type": "object",
        "properties": {
            "history_id": {
                "type": "string",
                "description": "The history entry ID (UUID), as returned by \
                    atuin_history.",
            },
            "ranges": {
                "type": "array",
                "items": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "minItems": 2,
                    "maxItems": 2,
                },
                "description": "Optional [start, end] line ranges to fetch \
                    (0-based, end-inclusive). Negative indices count from the end \
                    of the output, e.g. [[-80, -1]] is the last 80 lines — fetch \
                    that first when investigating a failure, since errors usually \
                    print at the end. Defaults to the full output.",
            },
        },
        "required": ["history_id"],
    }) else {
        unreachable!()
    };

    vec![
        Tool::new(
            "atuin_history",
            format!(
                "Search the user's shell history, recorded by Atuin across all their terminal \
                 sessions and machines. Each result includes the command, timestamp, working \
                 directory, exit code, duration, and a history ID; commands run by AI agents \
                 carry the agent's name and stated intent. Set only_failed: true when \
                 investigating failures, and authors: [\"{AUTHOR_FILTER_ALL_AGENT}\"] to see what \
                 agents ran. Pass a history ID to atuin_output to read what the command printed."
            ),
            history_schema,
        )
        .annotate(ToolAnnotations::with_title("Search shell history").read_only(true)),
        Tool::new(
            "atuin_output",
            "Read the terminal output that a previously executed command actually printed, \
             identified by a history ID from atuin_history results. Use it to see an error \
             exactly as the user saw it, or to re-read the output of an expensive or \
             side-effectful command without re-running it. Output capture requires the Atuin \
             daemon; output is only available for recent commands captured while the daemon was \
             running.",
            output_schema,
        )
        .annotate(ToolAnnotations::with_title("Read past command output").read_only(true)),
    ]
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// MCP clients inject the instructions into the model's system prompt on
    /// every session, so the block must stay small.
    const MAX_INSTRUCTIONS_LEN: usize = 2_000;

    #[rstest]
    fn tool_definitions_list_both_tools_as_read_only() {
        let tools = tool_definitions();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, ["atuin_history", "atuin_output"]);

        for tool in &tools {
            assert_eq!(tool.annotations.as_ref().unwrap().read_only_hint, Some(true));
            assert!(tool.input_schema.contains_key("required"));
        }

        // Everything except `query` is optional — see the filter_modes
        // comment in AtuinHistoryToolCall::try_from for why.
        let required = tools[0].input_schema.get("required").unwrap();
        assert_eq!(required, &json!(["query"]));
    }

    #[rstest]
    fn server_info_carries_instructions() {
        let instructions = server_info().instructions.expect("initialize result has instructions");
        assert!(instructions.contains("atuin_history"));
        assert!(instructions.contains("atuin_output"));
        assert!(instructions.len() < MAX_INSTRUCTIONS_LEN, "instructions should stay concise");
    }
}
