//! An MCP (Model Context Protocol) server over stdio, built on the official
//! `rmcp` SDK.
//!
//! This exposes the same history tools the AI assistant uses (`atuin_history`
//! and `atuin_output`), plus `atuin_output_search`, to external MCP clients
//! such as Claude Code or Cursor.
//!
//! History search reads the sqlite database directly and works without the
//! daemon; output retrieval and output search talk to the daemon and return a
//! tool error when it is not running.
//!
//! While the user uploads threads to Octavo, the thread tools are Octavo's own, forwarded to its
//! MCP server, in place of the local ones: they search every machine's threads and the threads
//! the user's organizations share.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime};

use atuin_client::database::Sqlite;
use atuin_client::history::{AUTHOR_FILTER_ALL_AGENT, AUTHOR_FILTER_ALL_USER, KNOWN_AGENTS};
use atuin_client::settings::Settings;
use eyre::Result;
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    Implementation, JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::{Peer, RequestContext, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Value, json};
use strum::IntoEnumIterator;
use tokio::sync::Mutex;

use crate::tools::output::search::AtuinOutputSearchToolCall;
use crate::tools::session::caller::{Caller, OwnSession};
use crate::tools::session::list::AtuinThreadsListToolCall;
use crate::tools::session::read::AtuinThreadsReadToolCall;
use crate::tools::session::search::AtuinThreadsSearchToolCall;
use crate::tools::{
    AtuinHistoryToolCall, AtuinOutputToolCall, DEFAULT_HISTORY_RESULTS, HistorySearchFilterMode,
    MAX_HISTORY_RESULTS, ToolOutcome,
};

struct AtuinMcp {
    db: Sqlite,
    settings: Arc<Settings>,
    own_session: OwnSession,
    octavo: Option<Arc<Remote>>,
}

/// This says where Octavo's MCP server is. It is given while the user uploads threads to it.
pub struct Octavo {
    pub url: String,
    pub token: SecretString,
    /// These are the organizations the server's directory shares its threads with.
    pub orgs: Vec<String>,
}

/// This is Octavo's MCP server, which is connected on first use. It holds Octavo's tools as the
/// last session listed them.
struct Remote {
    octavo: Octavo,
    client: Mutex<Option<RunningService<RoleClient, ()>>>,
    tools: Vec<Tool>,
    /// The listed tools are kept here for the next session, which starts from them.
    cache: PathBuf,
}

/// The local tools cover these Octavo tools better. Local history is complete and immediate,
/// while Octavo holds only what has been uploaded.
const LOCAL_ON_OCTAVO: [&str; 3] = ["search_history", "tally_history", "get_command_output"];

/// Returns the Octavo tools that are forwarded to it.
fn forwarded(mut tools: Vec<Tool>) -> Vec<Tool> {
    tools.retain(|t| !LOCAL_ON_OCTAVO.contains(&t.name.as_ref()));
    tools
}

/// Returns the tools to list. Octavo's tools take the place of the local thread tools.
fn listed_tools(octavo: Option<&[Tool]>) -> Vec<Tool> {
    match octavo {
        Some(octavo) => {
            let local = TOOLS.iter().filter(|t| !t.name.starts_with("atuin_threads_"));
            local.chain(octavo).cloned().collect()
        }
        None => TOOLS.clone(),
    }
}

/// These are how long Octavo has to connect, and then to answer a tool call.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A session trusts the cached tools for this long before it lists them again.
const CACHE_FRESH: Duration = Duration::from_hours(1);

/// A session that finds no cached tools waits this long for Octavo to list them.
const COLD_WAIT: Duration = Duration::from_secs(2);

impl Remote {
    /// Returns Octavo's server with the tools that the cache holds.
    fn new(settings: &Settings, octavo: Octavo) -> Self {
        let cache = settings.data_dir.join("octavo_mcp_tools.json");
        let tools = fs_err::read(&cache).ok().and_then(|json| serde_json::from_slice(&json).ok());
        Self {
            octavo,
            client: Mutex::new(None),
            tools: tools.unwrap_or_default(),
            cache,
        }
    }

    /// Returns the connection. It is made again when the last one has closed.
    async fn peer(&self) -> Result<Peer<RoleClient>> {
        let mut client = self.client.lock().await;
        if let Some(open) = client.as_ref().filter(|c| !c.is_transport_closed()) {
            return Ok(open.peer().clone());
        }
        let open = self.connect().await?;
        let peer = open.peer().clone();
        *client = Some(open);
        drop(client);
        Ok(peer)
    }

    /// Returns a new connection to Octavo.
    async fn connect(&self) -> Result<RunningService<RoleClient, ()>> {
        let config = StreamableHttpClientTransportConfig::with_uri(self.octavo.url.clone())
            .auth_header(self.octavo.token.expose_secret());
        let connect = ().serve(StreamableHttpClientTransport::from_config(config));
        Ok(tokio::time::timeout(CONNECT_TIMEOUT, connect).await??)
    }

    async fn call(&self, request: CallToolRequestParams) -> CallToolResult {
        let call = async { eyre::Ok(self.peer().await?.call_tool(request).await?) };
        match tokio::time::timeout(CALL_TIMEOUT, call).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => self.failed(format!("Octavo failed: {e:#}")).await,
            Err(_) => self.failed("Octavo did not answer in time".into()).await,
        }
    }

    /// Drops the connection and ages the cache, so the next session lists Octavo's tools again.
    /// One failed call does not drop the cache, because Octavo is usually back by then.
    async fn failed(&self, error: String) -> CallToolResult {
        tracing::warn!("{error}");
        self.client.lock().await.take();
        let cache = fs_err::File::options().write(true).open(&self.cache);
        let _ = cache.and_then(|file| file.set_modified(SystemTime::UNIX_EPOCH));
        CallToolResult::error(vec![ContentBlock::text(error)])
    }

    /// Lists Octavo's tools into the cache and returns them. It uses a connection of its own,
    /// because closing the shared one would fail a tool call in flight.
    async fn list(&self) -> Result<Vec<Tool>> {
        let octavo = self.connect().await?;
        let tools = forwarded(tokio::time::timeout(CALL_TIMEOUT, octavo.list_all_tools()).await??);
        // Written aside and renamed, so another session never reads half of it.
        let dir = self.cache.parent().unwrap_or(&self.cache);
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut file, &tools)?;
        file.persist(&self.cache)?;
        Ok(tools)
    }

    /// Lists Octavo's tools into the cache, unless the cache is fresh. It drops the cache when
    /// Octavo cannot list them, and the next session then starts with local threads.
    async fn refresh(self: Arc<Self>) {
        let age = fs_err::metadata(&self.cache).and_then(|m| m.modified()).ok();
        if age.and_then(|t| t.elapsed().ok()).is_some_and(|age| age < CACHE_FRESH) {
            return;
        }
        if let Err(e) = self.list().await {
            tracing::warn!("Octavo failed: {e:#}");
            let _ = fs_err::remove_file(&self.cache);
        }
    }

    fn instructions(&self) -> String {
        octavo_instructions(&self.octavo.orgs)
    }
}

/// Server-level instructions, surfaced by MCP clients (Claude Code injects
/// them into the model's system prompt). This is the main lever for making
/// agents reach for Atuin instead of guessing: it frames the history as the
/// ground truth for "what actually happened in the terminal".
const HISTORY_INSTRUCTIONS: &str =
    "\
Atuin is the ground truth for what actually happened in the user's terminal. It records shell \
     history from every session and machine: each command with its timestamp, working directory, \
     exit code, and duration — including commands run by AI agents (tagged with the agent's name \
     and intent) — and, where output capture is enabled, the full terminal output of each command.

Search atuin_history instead of guessing, asking the user, or re-running things whenever past \
     terminal activity is relevant: how the user last invoked something ('what flags did I use'), \
     whether and when something ran and if it succeeded, why a command failed (search with \
     only_failed: true, then read the actual error with atuin_output), or what an AI agent ran. \
     When debugging, checking recent history early often reveals what the user already tried. \
     When you know what was printed but not which command printed it — an error message, a \
     version, a hostname, a path — search the captured output itself with atuin_output_search.

When a question is about the user themselves — 'what do I use', 'how do I connect', 'what's my \
     setup' — run one atuin_history search BEFORE searching the filesystem. It is a single cheap \
     call, and 'it is not in the repository' is not an answer: personal habits live in shell \
     history, not in checked-in config.

Do not use `history`, ~/.bash_history, or ~/.zsh_history: they are typically empty or stale in \
     non-interactive shells and lack exit codes and output. Atuin is the reliable source. Prefer \
     atuin_output over re-running an expensive or side-effectful command just to see its output \
     again.";

const LOCAL_THREADS_INSTRUCTIONS: &str =
    "\
Atuin also records AI agent sessions as threads. Use atuin_threads_search or atuin_threads_list, \
     then atuin_threads_read, whenever a task points at earlier work: 'last time', 'again', \
     'still', 'continue', what was decided, or whether something is a known issue. Otherwise do \
     one quick search only when earlier threads would clearly help; skip self-contained tasks.";

const OCTAVO_THREADS_INSTRUCTIONS: &str = "\
Octavo holds the user's AI agent threads from every machine, and those their organizations shared. \
                                           Before exploring or designing anything, however small, \
                                           search_threads for prior work. Without org_slug it \
                                           reads only the user's own. Read a hit with get_thread \
                                           from its message_id.";

/// Returns the instructions for when the thread tools are Octavo's. They name `orgs`, the
/// organizations this project shares its threads with.
fn octavo_instructions(orgs: &[String]) -> String {
    let mut text = format!("{HISTORY_INSTRUCTIONS}\n\n{OCTAVO_THREADS_INSTRUCTIONS}");
    if !orgs.is_empty() {
        let slugs = orgs.iter().map(|o| format!("\"{o}\"")).collect::<Vec<_>>().join(", ");
        text.push_str(&format!("\n\nThis project's organization is {slugs}: pass it as org_slug."));
    }
    text
}

fn local_instructions() -> String {
    format!("{HISTORY_INSTRUCTIONS}\n\n{LOCAL_THREADS_INSTRUCTIONS}")
}

/// The initialize result, separated from the handler so tests can assert on
/// it without constructing a database-backed server.
fn server_info(instructions: String) -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation::new("atuin", env!("CARGO_PKG_VERSION")))
        .with_instructions(instructions)
}

impl ServerHandler for AtuinMcp {
    fn get_info(&self) -> ServerConfig {
        server_info(self.octavo.as_deref().map_or_else(local_instructions, Remote::instructions))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = listed_tools(self.octavo.as_ref().map(|remote| &remote.tools[..]));
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if let Some(remote) = &self.octavo
            && remote.tools.iter().any(|t| t.name == request.name)
        {
            return Ok(remote.call(request).await.into());
        }
        let arguments = request.arguments.unwrap_or_default();
        // Stateless clients (protocol 2026-07-28, e.g. Claude Code) skip `initialize` and send
        // their client info in each request's `_meta`.
        let client = context.client_info();
        let caller = Caller {
            client: client.as_ref().map(|c| c.name.as_str()),
            own: &self.own_session,
        };
        // Bad arguments come back as a tool error, not a JSON-RPC one, so the model sees what was
        // wrong and can retry (MCP SEP-1303); many clients never show protocol errors to it.
        let outcome = match request.name.as_ref() {
            "atuin_history" => match AtuinHistoryToolCall::try_from(&Value::Object(arguments)) {
                Ok(call) => call.execute(&self.db, &self.settings).await,
                Err(e) => invalid_arguments("atuin_history", e),
            },
            "atuin_output" => match AtuinOutputToolCall::try_from(&Value::Object(arguments)) {
                Ok(call) => call.execute().await,
                Err(e) => invalid_arguments("atuin_output", e),
            },
            "atuin_output_search" => {
                match parse::<AtuinOutputSearchToolCall>("atuin_output_search", arguments) {
                    Ok(call) => call.execute(&self.db, &self.settings).await,
                    Err(outcome) => outcome,
                }
            }
            "atuin_threads_search" | "atuin_ai_session_search" => {
                match parse::<AtuinThreadsSearchToolCall>("atuin_threads_search", arguments) {
                    Ok(call) => call.execute(&self.settings, &caller).await,
                    Err(outcome) => outcome,
                }
            }
            "atuin_threads_list" | "atuin_ai_session_list" => {
                match parse::<AtuinThreadsListToolCall>("atuin_threads_list", arguments) {
                    Ok(call) => call.execute(&self.settings, &caller).await,
                    Err(outcome) => outcome,
                }
            }
            "atuin_threads_read" | "atuin_ai_session_read" => {
                match parse::<AtuinThreadsReadToolCall>("atuin_threads_read", arguments) {
                    Ok(call) => call.execute(&self.settings, &caller).await,
                    Err(outcome) => outcome,
                }
            }
            name => {
                return Err(ErrorData::invalid_params(format!("unknown tool: {name}"), None));
            }
        };

        let result = match outcome {
            ToolOutcome::Success(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            ToolOutcome::Error(text) => CallToolResult::error(vec![ContentBlock::text(text)]),
            // The atuin tools only produce Success/Error; fall back to the
            // generic formatting should that ever change.
            outcome @ ToolOutcome::Structured { .. } => {
                CallToolResult::success(vec![ContentBlock::text(outcome.format_for_llm(None))])
            }
        };
        Ok(result.into())
    }
}

fn parse<T: serde::de::DeserializeOwned>(
    tool: &str,
    arguments: JsonObject,
) -> Result<T, ToolOutcome> {
    serde_json::from_value(Value::Object(arguments)).map_err(|e| invalid_arguments(tool, e))
}

/// Carries the tool's input schema, because a model that guessed the arguments often never saw
/// it: clients like Claude Code defer MCP schemas until the model asks for them.
fn invalid_arguments(tool: &str, err: impl std::fmt::Display) -> ToolOutcome {
    let schema =
        TOOLS.iter().find(|t| t.name == tool).map(|t| Value::Object((*t.input_schema).clone()));
    ToolOutcome::Error(format!(
        "Invalid arguments: {err}. Retry with arguments matching this input schema: {}",
        schema.unwrap_or_default()
    ))
}

/// Lists Octavo's tools into the cache that [`run`] starts from. The daemon calls this to keep the
/// cache warm.
pub async fn cache_tools(settings: &Settings, octavo: Octavo) -> Result<()> {
    Remote::new(settings, octavo).list().await.map(drop)
}

/// Serve MCP over stdio until the client disconnects.
///
/// stdout carries only JSON-RPC messages; anything else (logs, errors) must
/// go to stderr or it will corrupt the protocol stream.
///
/// With `octavo`, the thread tools are Octavo's, as the cache lists them. The daemon keeps the
/// cache warm and a session refreshes it in the background, so the client does not wait on Octavo
/// to start. Only a session that finds no cache waits, and for two seconds at most.
pub async fn run(db: &Sqlite, settings: Arc<Settings>, octavo: Option<Octavo>) -> Result<()> {
    let octavo = match octavo {
        Some(octavo) => {
            let mut remote = Remote::new(&settings, octavo);
            if remote.tools.is_empty()
                && let Ok(Ok(tools)) = tokio::time::timeout(COLD_WAIT, remote.list()).await
            {
                remote.tools = tools;
            }
            let remote = Arc::new(remote);
            tokio::spawn(Arc::clone(&remote).refresh());
            Some(remote)
        }
        None => None,
    };
    let server = AtuinMcp {
        db: db.clone(),
        settings,
        own_session: OwnSession::default(),
        octavo: octavo.filter(|remote| !remote.tools.is_empty()),
    }
    .serve(rmcp::transport::stdio())
    .await?;
    server.waiting().await?;
    Ok(())
}

/// Tool metadata for `tools/list`, built once: the schemas and descriptions
/// are assembled from consts and the filter-mode enum, none of which change
/// at runtime. The history and output schemas are written by hand and must
/// mirror what the `TryFrom<&serde_json::Value>` impls in [`crate::tools`]
/// accept; the output-search schema is derived from its call struct.
static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(tool_definitions);

fn tool_definitions() -> Vec<Tool> {
    let Value::Object(history_schema) = json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query matched against past commands: commands \
                    containing every term come first, then fuzzy matches (each term's \
                    letters in order) fill the rest. Prefer a few distinctive terms (e.g. 'ffmpeg av1'), not a \
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
                    commands run inside the current repository, git or Jujutsu, \
                    'directory' to the exact current working directory, 'host' to \
                    this machine, 'session' to the shell session that launched this \
                    server (errors when it was not launched from a shell). Start \
                    global and narrow only if results are noisy.",
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
        Tool::new(
            "atuin_output_search",
            "Full-text search over the terminal output of every captured command. Use it when you \
             know what was printed but not which command printed it: an error message, a version \
             string, a hostname, a file path, a test name. Each result gives the command (with \
             its history ID, timestamp, directory and exit code) and the numbered output lines \
             around each match; pass the history ID and line numbers to atuin_output to read \
             more. Requires the Atuin daemon with output capture enabled.",
            schema_for_type::<AtuinOutputSearchToolCall>(),
        )
        .annotate(ToolAnnotations::with_title("Search past command output").read_only(true)),
        Tool::new(
            "atuin_threads_search",
            "Full-text search across the transcripts of the threads (AI coding-agent sessions) \
             Atuin has captured (Claude Code, Codex, opencode, pi). Reach for it when a task \
             points at earlier work ('again', 'still', 'continue', a known issue, what was \
             decided): a previous agent may already have diagnosed or decided it. Finds which \
             thread discussed a topic, hit an error, touched a file, or made a decision — it \
             searches what the user and agent wrote, tool names, and thread titles. Each result \
             gives the thread id, harness, last-active time, title, directory, and the best \
             matching message's number and snippet; pass the id to atuin_threads_read to read \
             around the match. Requires the Atuin daemon with thread capture enabled.",
            schema_for_type::<AtuinThreadsSearchToolCall>(),
        )
        .annotate(ToolAnnotations::with_title("Search agent threads").read_only(true)),
        Tool::new(
            "atuin_threads_list",
            "List captured threads, AI coding-agent sessions (Claude Code, Codex, opencode, pi), \
             most recently active first, optionally only those in a directory (cwd: '.' for this \
             project) or active since a time (since: 'today'). Use it to pick up where the \
             previous thread on this project left off ('continue', 'last time') or to see what \
             agents worked on recently, when you have no specific words to search for. Each entry \
             gives the thread id, harness, last-active time, message count, title, and directory. \
             Requires the Atuin daemon with thread capture enabled.",
            schema_for_type::<AtuinThreadsListToolCall>(),
        )
        .annotate(ToolAnnotations::with_title("List agent threads").read_only(true)),
        Tool::new(
            "atuin_threads_read",
            "Read a captured thread's transcript, a page of messages at a time: what the user \
             asked, what the agent replied, and which tools it used. Capture keeps the \
             conversation, not tool arguments or output, so runs of tool calls appear as one line \
             of tool names. Messages are numbered; page with start (negative counts from the end, \
             e.g. -10 for how the thread ended) and limit. Pages abridge long messages; read one \
             message (limit: 1) to see it in full; one over 20,000 characters comes in parts, \
             each giving the offset to read the next from. To find where something was said in a \
             long thread, pass query (only messages with every word) or roles (['user'] for what \
             the person said) instead of paging through it. Takes a thread id or unique prefix \
             from atuin_threads_list or atuin_threads_search, or 'latest'.",
            schema_for_type::<AtuinThreadsReadToolCall>(),
        )
        .annotate(ToolAnnotations::with_title("Read an agent thread").read_only(true)),
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
    fn tool_definitions_list_all_tools_as_read_only() {
        let tools = tool_definitions();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, [
            "atuin_history",
            "atuin_output",
            "atuin_output_search",
            "atuin_threads_search",
            "atuin_threads_list",
            "atuin_threads_read",
        ]);

        for tool in &tools {
            assert_eq!(tool.annotations.as_ref().unwrap().read_only_hint, Some(true));
        }

        // Everything except `query` is optional — see the filter_modes
        // comment in AtuinHistoryToolCall::try_from for why.
        let required = tools[0].input_schema.get("required").unwrap();
        assert_eq!(required, &json!(["query"]));
    }

    /// A misremembered option (`from_end`, say) is an error naming the real ones, not silently
    /// dropped so the call runs on defaults; the schema says so too.
    #[rstest]
    fn unknown_arguments_are_rejected() {
        let args = |v: Value| v.as_object().unwrap().clone();
        let Err(ToolOutcome::Error(err)) = parse::<AtuinThreadsReadToolCall>(
            "atuin_threads_read",
            args(json!({
                "session_id": "latest",
                "from_end": true,
            })),
        ) else {
            panic!("from_end was accepted");
        };
        assert!(err.contains("`from_end`") && err.contains("`start`"), "{err}");
        assert!(
            err.contains("Negative values count back from the end"),
            "the schema is in the error: {err}"
        );
        assert!(
            parse::<AtuinThreadsReadToolCall>(
                "atuin_threads_read",
                args(json!({"session_id": "x", "start": -10}))
            )
            .is_ok()
        );
        for tool in &tool_definitions()[2..] {
            assert_eq!(tool.input_schema["additionalProperties"], false, "{}", tool.name);
        }
    }

    /// The output-search schema is derived from the call struct, so this pins what the model
    /// sees rather than how the struct is annotated.
    #[rstest]
    fn output_search_schema_is_derived_from_the_call_struct() {
        let tools = tool_definitions();
        let schema = &tools[2].input_schema;
        assert_eq!(schema["required"], json!(["terms"]));
        let terms = &schema["properties"]["terms"];
        assert_eq!(terms["type"], "array");
        assert_eq!(terms["minItems"], 1);
        assert_eq!(terms["items"]["minLength"], 1);
        assert!(terms["description"].as_str().unwrap().contains("AND-ed"));
        let limit = &schema["properties"]["limit"];
        assert_eq!(limit["type"], "integer");
        assert_eq!(limit["minimum"], 1);
        assert_eq!(limit["maximum"], 20);
        assert_eq!(limit["default"], 5);
        assert!(limit["description"].as_str().unwrap().contains("most relevant first"));
    }

    #[rstest]
    fn ai_session_search_schema_matches_the_parser() {
        // The hand-written schema and the AtuinThreadsSearchToolCall parser are independent
        // sources of truth; drive the parser at the schema's advertised bounds so the two cannot
        // drift apart (e.g. widening the Clamped bounds without updating the model-visible schema).
        let tools = tool_definitions();
        let schema = &tools[3].input_schema;
        assert_eq!(schema["required"], json!(["terms"]));
        assert_eq!(schema["properties"]["terms"]["minItems"], 1);
        assert_eq!(schema["properties"]["terms"]["items"]["minLength"], 1);

        let limit = &schema["properties"]["limit"];
        let min = limit["minimum"].as_u64().unwrap();
        let max = limit["maximum"].as_u64().unwrap();
        let default = limit["default"].as_u64().unwrap();

        let parse = |v: Value| serde_json::from_value::<AtuinThreadsSearchToolCall>(v);
        let limit_of = |v: Value| u64::from(parse(v).unwrap().limit.get());
        assert_eq!(
            limit_of(json!({"terms": ["x"]})),
            default,
            "omitted limit uses the schema default"
        );
        assert_eq!(
            limit_of(json!({"terms": ["x"], "limit": max + 1})),
            max,
            "over-max clamps to the schema max"
        );
        assert_eq!(
            limit_of(json!({"terms": ["x"], "limit": 0})),
            min,
            "under-min clamps to the schema min"
        );
        assert!(
            parse(json!({"terms": []})).is_err(),
            "the parser enforces the advertised minItems"
        );
        assert!(
            parse(json!({"terms": ["   "]})).is_err(),
            "the parser enforces the advertised minLength"
        );

        // Follow the property's `$ref` rather than naming the type, so a rename cannot hide it.
        let reference = schema["properties"]["harness"]["anyOf"][0]["$ref"].as_str().unwrap();
        let name = reference.rsplit('/').next().unwrap();
        for harness in schema["$defs"][name]["enum"].as_array().unwrap() {
            let name = harness.as_str().unwrap();
            assert!(
                parse(json!({"terms": ["x"], "harness": name})).is_ok(),
                "advertised {name:?} must parse"
            );
        }
        assert!(
            parse(json!({"terms": ["x"], "harness": "cursor"})).is_err(),
            "an unadvertised harness is rejected"
        );
    }

    fn octavo_tool(name: &'static str) -> Tool {
        Tool::new(name, "", JsonObject::new())
    }

    #[rstest]
    fn octavo_thread_tools_replace_the_local_ones_and_its_history_tools_are_dropped() {
        let octavo = forwarded(
            [
                "search_threads",
                "get_thread",
                "search_history",
                "tally_history",
                "get_command_output",
            ]
            .map(octavo_tool)
            .to_vec(),
        );
        let names: Vec<_> = listed_tools(Some(&octavo)).into_iter().map(|t| t.name).collect();
        assert_eq!(names, [
            "atuin_history",
            "atuin_output",
            "atuin_output_search",
            "search_threads",
            "get_thread",
        ]);
        assert_eq!(listed_tools(None).len(), TOOLS.len());
    }

    #[rstest]
    #[tokio::test]
    async fn a_failed_call_ages_the_cache_and_a_failed_refresh_drops_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("octavo_mcp_tools.json");
        fs_err::write(&cache, "[]").unwrap();
        let remote = Arc::new(Remote {
            octavo: Octavo {
                // Nothing listens on port 1, so the connection is refused at once.
                url: "http://127.0.0.1:1/".into(),
                token: SecretString::from("atapi_test"),
                orgs: Vec::new(),
            },
            client: Mutex::new(None),
            tools: Vec::new(),
            cache: cache.clone(),
        });

        let result = remote.call(CallToolRequestParams::new("search_threads")).await;

        assert_eq!(result.is_error, Some(true));
        let age = fs_err::metadata(&cache).unwrap().modified().unwrap().elapsed().unwrap();
        assert!(age > CACHE_FRESH, "the next session lists the tools again");

        remote.refresh().await;

        assert!(!cache.exists());
    }

    #[rstest]
    fn octavo_instructions_name_the_project_orgs_and_stay_concise() {
        let org = "an-organization-slug-at-the-length-limit".to_owned();
        let instructions = octavo_instructions(std::slice::from_ref(&org));
        assert!(instructions.contains("search_threads"));
        assert!(
            instructions.ends_with(&format!("organization is \"{org}\": pass it as org_slug."))
        );
        assert!(!instructions.contains("atuin_threads_"));
        assert!(instructions.len() < MAX_INSTRUCTIONS_LEN, "instructions should stay concise");
        assert!(!octavo_instructions(&[]).contains("This project"));
    }

    #[rstest]
    fn server_info_carries_instructions() {
        let instructions = server_info(local_instructions())
            .instructions
            .expect("initialize result has instructions");
        assert!(instructions.contains("atuin_history"));
        assert!(instructions.contains("atuin_output"));
        assert!(instructions.contains("atuin_output_search"));
        assert!(instructions.contains("atuin_threads_search"));
        assert!(instructions.contains("atuin_threads_list"));
        assert!(instructions.contains("atuin_threads_read"));
        assert!(instructions.len() < MAX_INSTRUCTIONS_LEN, "instructions should stay concise");
    }
}
