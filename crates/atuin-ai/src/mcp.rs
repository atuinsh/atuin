//! An MCP (Model Context Protocol) server over stdio, built on the official
//! `rmcp` SDK.
//!
//! This exposes the same history tools the AI assistant uses (`atuin_history`
//! and `atuin_output`) to external MCP clients such as Claude Code or Cursor.
//!
//! History search reads the sqlite database directly and works without the
//! daemon; output retrieval talks to the daemon and returns a tool error when
//! it is not running.

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

use crate::tools::{AtuinHistoryToolCall, AtuinOutputToolCall, ToolOutcome};

struct AtuinMcp {
    db: Sqlite,
}

impl ServerHandler for AtuinMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("atuin", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tool_definitions()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let outcome = match request.name.as_ref() {
            "atuin_history" => AtuinHistoryToolCall::try_from(&arguments)
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
                .execute(&self.db)
                .await,
            "atuin_output" => AtuinOutputToolCall::try_from(&arguments)
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
                .execute()
                .await,
            name => {
                return Err(ErrorData::invalid_params(
                    format!("unknown tool: {name}"),
                    None,
                ));
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
    let server = AtuinMcp {
        db: Sqlite {
            pool: db.pool.clone(),
        },
    }
    .serve(rmcp::transport::stdio())
    .await?;
    server.waiting().await?;
    Ok(())
}

/// Tool metadata for `tools/list`. The input schemas mirror what the
/// `TryFrom<&serde_json::Value>` impls in [`crate::tools`] accept.
fn tool_definitions() -> Vec<Tool> {
    let Value::Object(history_schema) = json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Fuzzy search query matched against past commands. \
                    An empty string returns the most recent commands. Supports \
                    fzf-style operators per space-separated term: ^prefix, suffix$, \
                    'exact-substring, !negate, and r/regex/.",
            },
            "filter_modes": {
                "type": "array",
                "items": {
                    "type": "string",
                    "enum": ["global", "host", "session", "directory", "workspace"],
                },
                "description": "Search scope; the first entry is used. 'global' \
                    searches all history, 'host' only commands run on this machine, \
                    'directory' only commands run in the current working directory, \
                    'workspace' only commands run inside the current git repository, \
                    'session' only commands from the shell session that launched \
                    this server (errors when it was not launched from a shell).",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 50,
                "default": 10,
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
                     by AI agents, or a literal agent name (one of: {}). Multiple \
                     entries are OR-ed. Omit for everything.",
                    KNOWN_AGENTS.join(", ")
                ),
            },
        },
        "required": ["query", "filter_modes"],
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
                    of the output, e.g. [-50, -1] is the last 50 lines. Defaults \
                    to the first 1000 lines.",
            },
        },
        "required": ["history_id"],
    }) else {
        unreachable!()
    };

    vec![
        Tool::new(
            "atuin_history",
            "Search the user's shell command history, recorded by Atuin. \
             Fuzzy-matches the query against past commands and returns the most relevant \
             entries, each with a history ID, timestamp, working directory, exit code, \
             and duration. Commands run by AI agents are annotated with the agent's name \
             and stated intent. Pass a history ID to atuin_output to see what a command \
             printed.",
            history_schema,
        )
        .annotate(ToolAnnotations::new().read_only(true)),
        Tool::new(
            "atuin_output",
            "Fetch the captured terminal output of a previously executed \
             command, identified by a history ID from atuin_history results. Output \
             capture requires the Atuin daemon; output is only available for recent \
             commands captured while the daemon was running.",
            output_schema,
        )
        .annotate(ToolAnnotations::new().read_only(true)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_list_both_tools_as_read_only() {
        let tools = tool_definitions();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, ["atuin_history", "atuin_output"]);

        for tool in &tools {
            assert_eq!(
                tool.annotations.as_ref().unwrap().read_only_hint,
                Some(true)
            );
            assert!(tool.input_schema.contains_key("required"));
        }
    }
}
