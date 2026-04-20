//! Agent conversation FSM.
//!
//! Pure state machine that returns effects as data.
//! The driver is responsible for executing effects and feeding events back.
//!
//! The FSM owns the conversation event log and tool lifecycle state.
//! It never performs IO directly.

pub(crate) mod effects;
pub(crate) mod events;
pub(crate) mod tools;

#[cfg(test)]
mod tests;

use serde_json::Value;

use crate::context_window::ContextWindowBuilder;
use crate::tui::state::ConversationEvent;

use effects::{Effect, ExitAction};
use events::{Event, PermissionChoice, PermissionResponse};
use tools::{ToolManager, ToolState};

// ============================================================================
// State
// ============================================================================

/// The discrete states of the agent FSM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentState {
    /// Waiting for user input.
    Idle {
        confirmation: Option<PendingConfirmation>,
    },

    /// A conversation turn is in progress.
    Turn { stream: StreamPhase },

    /// Unrecoverable error. User can retry or exit.
    Error(String),
}

/// Stream connection lifecycle within a Turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamPhase {
    /// Request sent, awaiting first stream frame.
    Connecting,
    /// Actively receiving streamed response.
    Streaming { status: Option<StreamingStatus> },
    /// Stream connection has ended (Done received).
    Done,
}

/// Streaming status indicators from server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamingStatus {
    Processing,
    Searching,
    Thinking,
    WaitingForTools,
}

impl StreamingStatus {
    pub(crate) fn from_str(s: &str) -> Self {
        match s {
            "processing" => Self::Processing,
            "searching" => Self::Searching,
            "waiting_for_tools" => Self::WaitingForTools,
            _ => Self::Thinking,
        }
    }
}

/// Pending dangerous command confirmation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingConfirmation {
    pub command: String,
    pub timeout_id: u64,
}

// ============================================================================
// Context
// ============================================================================

/// Shared context owned by the FSM.
#[derive(Debug, Clone)]
pub(crate) struct AgentContext {
    /// The full conversation event log (source of truth for API + persistence).
    pub events: Vec<ConversationEvent>,
    /// Server-assigned session ID.
    pub session_id: Option<String>,
    /// Accumulated text from current stream (committed to events on tool call or stream end).
    pub current_response: String,
    /// Per-tool lifecycle state and cached render data.
    pub tools: ToolManager,
    /// Counter for generating unique timeout IDs.
    next_timeout_id: u64,
    /// Capabilities advertised to the server.
    pub capabilities: Vec<String>,
    /// Unique invocation ID for this CLI invocation.
    pub invocation_id: String,
}

impl AgentContext {
    fn next_timeout_id(&mut self) -> u64 {
        let id = self.next_timeout_id;
        self.next_timeout_id += 1;
        id
    }
}

// ============================================================================
// The Agent FSM
// ============================================================================

/// The agent finite state machine.
///
/// Pure state machine — `handle()` takes an event, mutates internal state,
/// and returns effects as data for the driver to execute.
#[derive(Debug, Clone)]
pub(crate) struct AgentFsm {
    pub state: AgentState,
    pub ctx: AgentContext,
}

impl AgentFsm {
    /// Create a new FSM in Idle state.
    pub fn new(capabilities: Vec<String>, invocation_id: String) -> Self {
        Self {
            state: AgentState::Idle { confirmation: None },
            ctx: AgentContext {
                events: Vec::new(),
                session_id: None,
                current_response: String::new(),
                tools: ToolManager::new(),
                next_timeout_id: 0,
                capabilities,
                invocation_id,
            },
        }
    }

    /// Create an FSM from saved session state (for resume).
    pub fn from_session(
        events: Vec<ConversationEvent>,
        session_id: Option<String>,
        capabilities: Vec<String>,
        invocation_id: String,
    ) -> Self {
        Self {
            state: AgentState::Idle { confirmation: None },
            ctx: AgentContext {
                events,
                session_id,
                current_response: String::new(),
                tools: ToolManager::new(),
                next_timeout_id: 0,
                capabilities,
                invocation_id,
            },
        }
    }

    /// Handle an event, returning effects to execute.
    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        match (&self.state, event) {
            // ================================================================
            // Idle state
            // ================================================================
            (AgentState::Idle { confirmation: None }, Event::UserSubmit(msg)) => {
                self.start_turn(msg)
            }

            (
                AgentState::Idle {
                    confirmation: Some(_),
                },
                Event::UserSubmit(msg),
            ) => {
                let timeout_id = self.state_confirmation().unwrap().timeout_id;
                let mut effects = vec![Effect::CancelTimeout { timeout_id }];
                effects.extend(self.start_turn(msg));
                effects
            }

            (AgentState::Idle { confirmation: None }, Event::ExecuteCommand) => {
                let cmd = self.current_command();
                let Some(cmd) = cmd else {
                    return vec![];
                };
                if self.is_current_command_dangerous() {
                    let timeout_id = self.ctx.next_timeout_id();
                    self.state = AgentState::Idle {
                        confirmation: Some(PendingConfirmation {
                            command: cmd.clone(),
                            timeout_id,
                        }),
                    };
                    vec![Effect::ScheduleTimeout {
                        timeout_id,
                        duration: std::time::Duration::from_secs(5),
                    }]
                } else {
                    vec![Effect::ExitApp(ExitAction::Execute(cmd))]
                }
            }

            (
                AgentState::Idle {
                    confirmation: Some(_),
                },
                Event::ExecuteCommand,
            ) => {
                let confirm = self.state_confirmation().unwrap().clone();
                self.state = AgentState::Idle { confirmation: None };
                vec![
                    Effect::CancelTimeout {
                        timeout_id: confirm.timeout_id,
                    },
                    Effect::ExitApp(ExitAction::Execute(confirm.command)),
                ]
            }

            (AgentState::Idle { .. }, Event::InsertCommand) => {
                let cmd = self.current_command();
                match cmd {
                    Some(cmd) => vec![Effect::ExitApp(ExitAction::Insert(cmd))],
                    None => vec![],
                }
            }

            (
                AgentState::Idle {
                    confirmation: Some(_),
                },
                Event::Cancel,
            ) => {
                let timeout_id = self.state_confirmation().unwrap().timeout_id;
                self.state = AgentState::Idle { confirmation: None };
                vec![Effect::CancelTimeout { timeout_id }]
            }

            (AgentState::Idle { confirmation: None }, Event::Cancel) => {
                vec![Effect::ExitApp(ExitAction::Cancel)]
            }

            (AgentState::Idle { .. }, Event::ConfirmationTimeout { timeout_id }) => {
                if self
                    .state_confirmation()
                    .is_some_and(|c| c.timeout_id == timeout_id)
                {
                    self.state = AgentState::Idle { confirmation: None };
                }
                vec![]
            }

            (AgentState::Idle { .. }, Event::NewSession) => {
                self.ctx.events.clear();
                self.ctx.session_id = None;
                self.ctx.tools.clear();
                self.state = AgentState::Idle { confirmation: None };
                vec![Effect::ArchiveSession, Effect::Persist]
            }

            (AgentState::Idle { .. }, Event::SlashCommand(cmd)) => {
                self.handle_slash_command(&cmd);
                vec![]
            }

            // ================================================================
            // Turn — stream lifecycle
            // ================================================================
            (
                AgentState::Turn {
                    stream: StreamPhase::Connecting,
                },
                Event::StreamStarted,
            ) => {
                self.state = AgentState::Turn {
                    stream: StreamPhase::Streaming { status: None },
                };
                vec![]
            }

            (
                AgentState::Turn {
                    stream: StreamPhase::Connecting,
                },
                Event::StreamError(e),
            ) => {
                self.state = AgentState::Error(e);
                vec![]
            }

            (
                AgentState::Turn {
                    stream: StreamPhase::Streaming { .. },
                },
                Event::StreamChunk(text),
            ) => {
                self.ctx.current_response.push_str(&text);
                vec![]
            }

            (
                AgentState::Turn {
                    stream: StreamPhase::Streaming { .. },
                },
                Event::StreamStatusChanged(status),
            ) => {
                self.state = AgentState::Turn {
                    stream: StreamPhase::Streaming {
                        status: Some(StreamingStatus::from_str(&status)),
                    },
                };
                vec![]
            }

            (AgentState::Turn { .. }, Event::StreamToolCall { id, name, input }) => {
                self.commit_streaming_text();
                self.handle_stream_tool_call(id, name, input)
            }

            (
                AgentState::Turn {
                    stream: StreamPhase::Streaming { .. },
                },
                Event::SuggestCommand { id, input },
            ) => {
                self.commit_streaming_text();
                // Push the suggest_command as a ToolCall event (protocol requirement)
                self.ctx.events.push(ConversationEvent::ToolCall {
                    id,
                    name: "suggest_command".to_string(),
                    input,
                });
                self.state = AgentState::Idle { confirmation: None };
                vec![Effect::Persist]
            }

            (
                AgentState::Turn {
                    stream: StreamPhase::Streaming { .. },
                },
                Event::StreamServerToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    remote,
                    content_length,
                },
            ) => {
                self.ctx.events.push(ConversationEvent::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    remote,
                    content_length,
                });
                vec![]
            }

            (AgentState::Turn { .. }, Event::StreamDone { session_id }) => {
                self.commit_streaming_text();
                if !session_id.is_empty() {
                    self.ctx.session_id = Some(session_id);
                }
                self.state = AgentState::Turn {
                    stream: StreamPhase::Done,
                };
                self.check_turn_completion()
            }

            (
                AgentState::Turn {
                    stream: StreamPhase::Streaming { .. },
                },
                Event::StreamError(e),
            ) => {
                // Abort any executing tools on stream error
                let abort_effects: Vec<_> = self
                    .ctx
                    .tools
                    .executing_ids()
                    .into_iter()
                    .map(|tool_id| Effect::AbortTool { tool_id })
                    .collect();
                self.state = AgentState::Error(e);
                abort_effects
            }

            // ================================================================
            // Turn — tool lifecycle (any stream phase)
            // ================================================================
            (AgentState::Turn { .. }, Event::PermissionResolved { tool_id, response }) => {
                self.handle_permission_resolved(tool_id, response)
            }

            (AgentState::Turn { .. }, Event::PermissionUserChoice { tool_id, choice }) => {
                self.handle_permission_choice(tool_id, choice)
            }

            (AgentState::Turn { .. }, Event::ToolExecutionDone { tool_id, outcome }) => {
                self.handle_tool_done(tool_id, outcome)
            }

            (
                AgentState::Turn { .. },
                Event::ToolPreviewUpdate {
                    tool_id,
                    lines,
                    exit_code,
                },
            ) => {
                if let Some(tracked) = self.ctx.tools.get_mut(&tool_id) {
                    tracked.preview = Some(tools::ToolPreviewData::Shell {
                        lines,
                        exit_code,
                        interrupted: false,
                    });
                }
                vec![]
            }

            (AgentState::Turn { .. }, Event::InterruptTools) => {
                let ids = self.ctx.tools.executing_ids();
                ids.into_iter()
                    .map(|tool_id| Effect::AbortTool { tool_id })
                    .collect()
            }

            // ─── Cancel during Turn ─────────────────────────────────────
            (AgentState::Turn { stream }, Event::Cancel) => {
                let mut effects = Vec::new();

                // Abort stream if still active
                if !matches!(stream, StreamPhase::Done) {
                    effects.push(Effect::AbortStream);
                }

                // Cancel all pending tools
                let pending = self.ctx.tools.pending_ids();
                for id in &pending {
                    if let Some(tracked) = self.ctx.tools.get_mut(id) {
                        if tracked.state == ToolState::Executing {
                            effects.push(Effect::AbortTool {
                                tool_id: id.clone(),
                            });
                        }
                        tracked.state = ToolState::Completed;
                    }
                    self.ctx.events.push(ConversationEvent::ToolResult {
                        tool_use_id: id.clone(),
                        content: "Error: user cancelled this operation".to_string(),
                        is_error: true,
                        remote: false,
                        content_length: None,
                    });
                }

                // Commit any partial streaming text
                self.commit_streaming_text_as_cancelled();

                // Add context so the LLM knows what happened
                if !pending.is_empty() {
                    self.ctx.events.push(ConversationEvent::SystemContext {
                        content: "The user cancelled the previous generation. Tool calls that were in progress have been aborted.".to_string(),
                    });
                }

                self.state = AgentState::Idle { confirmation: None };
                effects.push(Effect::Persist);
                effects
            }

            // ================================================================
            // Error state
            // ================================================================
            (AgentState::Error(_), Event::Retry) => {
                let messages = self.build_messages();
                let session_id = self.ctx.session_id.clone();
                self.state = AgentState::Turn {
                    stream: StreamPhase::Connecting,
                };
                vec![Effect::StartStream {
                    messages,
                    session_id,
                }]
            }

            (AgentState::Error(_), Event::Cancel) => {
                vec![Effect::ExitApp(ExitAction::Cancel)]
            }

            // ================================================================
            // Fallthrough — ignore events with no valid transition
            // ================================================================
            (_, Event::SlashCommand(cmd)) => {
                self.handle_slash_command(&cmd);
                vec![]
            }

            _ => vec![],
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // Private helpers
    // ────────────────────────────────────────────────────────────────────

    /// Start a new turn: push user message, build messages, emit StartStream.
    fn start_turn(&mut self, msg: String) -> Vec<Effect> {
        self.ctx
            .events
            .push(ConversationEvent::UserMessage { content: msg });
        self.ctx.tools.clear();
        self.ctx.current_response.clear();

        let messages = self.build_messages();
        let session_id = self.ctx.session_id.clone();
        self.state = AgentState::Turn {
            stream: StreamPhase::Connecting,
        };
        vec![Effect::StartStream {
            messages,
            session_id,
        }]
    }

    /// Build API messages from the conversation event log.
    fn build_messages(&self) -> Vec<Value> {
        ContextWindowBuilder::with_default_budget().build(&self.ctx.events)
    }

    /// Commit accumulated streaming text to the event log.
    fn commit_streaming_text(&mut self) {
        let text = std::mem::take(&mut self.ctx.current_response);
        let trimmed = text.trim_start().to_string();
        if !trimmed.is_empty() {
            self.ctx
                .events
                .push(ConversationEvent::Text { content: trimmed });
        }
    }

    /// Commit streaming text with a cancellation suffix.
    fn commit_streaming_text_as_cancelled(&mut self) {
        let text = std::mem::take(&mut self.ctx.current_response);
        let trimmed = text.trim_start().to_string();
        if !trimmed.is_empty() {
            self.ctx.events.push(ConversationEvent::Text {
                content: format!("{trimmed}\n\n[User cancelled this generation]"),
            });
        }
    }

    /// Handle a client-side tool call from the stream.
    fn handle_stream_tool_call(&mut self, id: String, name: String, input: Value) -> Vec<Effect> {
        // Parse the tool call
        let tool = match crate::tools::ClientToolCall::try_from((name.as_str(), &input)) {
            Ok(tool) => tool,
            Err(_) => {
                // Unknown tool — push as event but don't track
                self.ctx
                    .events
                    .push(ConversationEvent::ToolCall { id, name, input });
                return vec![];
            }
        };

        // Capability gating
        if let Some(required_cap) = tool.descriptor().capability
            && !self.ctx.capabilities.iter().any(|c| c == required_cap)
        {
            self.ctx.events.push(ConversationEvent::ToolCall {
                id: id.clone(),
                name,
                input,
            });
            self.ctx.events.push(ConversationEvent::ToolResult {
                tool_use_id: id,
                content: format!(
                    "Tool not enabled: capability '{required_cap}' was not advertised by this client"
                ),
                is_error: true,
                remote: false,
                content_length: None,
            });
            return vec![];
        }

        // Track the tool and push ToolCall event
        let tool_for_effect = tool.clone();
        self.ctx.tools.insert(id.clone(), tool);
        self.ctx.events.push(ConversationEvent::ToolCall {
            id: id.clone(),
            name,
            input,
        });

        // Transition to Turn if we were Streaming
        if let AgentState::Turn {
            stream: StreamPhase::Streaming { .. },
        } = &self.state
        {
            self.state = AgentState::Turn {
                stream: StreamPhase::Streaming { status: None },
            };
        }

        vec![Effect::CheckPermission {
            tool_id: id,
            tool: tool_for_effect,
        }]
    }

    /// Handle permission resolver result.
    fn handle_permission_resolved(
        &mut self,
        tool_id: String,
        response: PermissionResponse,
    ) -> Vec<Effect> {
        let Some(tracked) = self.ctx.tools.get_mut(&tool_id) else {
            return vec![];
        };

        match response {
            PermissionResponse::Allowed | PermissionResponse::SessionGranted => {
                tracked.state = ToolState::Executing;
                let tool = tracked.tool.clone();
                vec![Effect::ExecuteTool { tool_id, tool }]
            }
            PermissionResponse::Ask => {
                tracked.state = ToolState::AwaitingPermission;
                vec![]
            }
            PermissionResponse::Denied => {
                tracked.state = ToolState::Denied;
                self.ctx.events.push(ConversationEvent::ToolResult {
                    tool_use_id: tool_id,
                    content: "Permission denied on the user's system".to_string(),
                    is_error: true,
                    remote: false,
                    content_length: None,
                });
                self.check_turn_completion()
            }
        }
    }

    /// Handle user's permission choice from the dialog.
    fn handle_permission_choice(
        &mut self,
        tool_id: String,
        choice: PermissionChoice,
    ) -> Vec<Effect> {
        let Some(tracked) = self.ctx.tools.get_mut(&tool_id) else {
            return vec![];
        };

        match choice {
            PermissionChoice::Allow => {
                tracked.state = ToolState::Executing;
                let tool = tracked.tool.clone();
                vec![Effect::ExecuteTool { tool_id, tool }]
            }
            PermissionChoice::AllowForSession => {
                tracked.state = ToolState::Executing;
                let tool = tracked.tool.clone();
                let mut effects = vec![Effect::ExecuteTool {
                    tool_id,
                    tool: tool.clone(),
                }];
                if let Some(path) = tool.resolved_file_path() {
                    effects.push(Effect::CacheSessionGrant { path });
                }
                effects
            }
            PermissionChoice::AlwaysAllowInProject => {
                tracked.state = ToolState::Executing;
                let tool = tracked.tool.clone();
                let rule = crate::permissions::rule::Rule {
                    tool: tool.rule_name().to_string(),
                    scope: None, // project file provides the scoping
                };
                vec![
                    Effect::ExecuteTool { tool_id, tool },
                    Effect::WritePermissionRule {
                        path: std::path::PathBuf::new(), // driver fills in project root
                        rule,
                        disposition: crate::permissions::writer::RuleDisposition::Allow,
                    },
                ]
            }
            PermissionChoice::AlwaysAllow => {
                tracked.state = ToolState::Executing;
                let tool = tracked.tool.clone();
                let scope = tool
                    .resolved_file_path()
                    .map(|p| p.to_string_lossy().to_string());
                let rule = crate::permissions::rule::Rule {
                    tool: tool.rule_name().to_string(),
                    scope,
                };
                vec![
                    Effect::ExecuteTool { tool_id, tool },
                    Effect::WritePermissionRule {
                        path: std::path::PathBuf::new(), // driver fills in global path
                        rule,
                        disposition: crate::permissions::writer::RuleDisposition::Allow,
                    },
                ]
            }
            PermissionChoice::Deny => {
                tracked.state = ToolState::Denied;
                self.ctx.events.push(ConversationEvent::ToolResult {
                    tool_use_id: tool_id,
                    content: "Permission denied by the user".to_string(),
                    is_error: true,
                    remote: false,
                    content_length: None,
                });
                self.check_turn_completion()
            }
        }
    }

    /// Handle tool execution completion.
    fn handle_tool_done(
        &mut self,
        tool_id: String,
        outcome: crate::tools::ToolOutcome,
    ) -> Vec<Effect> {
        let Some(tracked) = self.ctx.tools.get_mut(&tool_id) else {
            return vec![];
        };

        // If already completed (e.g. cancelled), ignore stale result
        if tracked.is_resolved() {
            return vec![];
        }

        tracked.state = ToolState::Completed;

        let content = outcome.format_for_llm();
        let is_error = outcome.is_error();
        self.ctx.events.push(ConversationEvent::ToolResult {
            tool_use_id: tool_id,
            content,
            is_error,
            remote: false,
            content_length: None,
        });

        self.check_turn_completion()
    }

    /// Check if the turn is complete (stream done + all tools resolved).
    /// If so, either continue the conversation or go Idle.
    fn check_turn_completion(&mut self) -> Vec<Effect> {
        // Stream must be done
        if !matches!(
            self.state,
            AgentState::Turn {
                stream: StreamPhase::Done
            }
        ) {
            return vec![];
        }

        // All tools must be resolved (empty = vacuously true)
        if !self.ctx.tools.is_empty() && !self.ctx.tools.all_resolved() {
            return vec![];
        }

        // Turn is complete. Check if we need to continue (tool results to send back).
        // We continue if there were any client tools that produced results (the LLM
        // needs to see them and respond).
        let has_tool_results = !self.ctx.tools.is_empty();

        if has_tool_results {
            // Continue conversation with tool results
            let messages = self.build_messages();
            let session_id = self.ctx.session_id.clone();
            self.ctx.tools.clear();
            self.ctx.current_response.clear();
            self.state = AgentState::Turn {
                stream: StreamPhase::Connecting,
            };
            vec![Effect::StartStream {
                messages,
                session_id,
            }]
        } else {
            // No tools — turn is done, go idle
            self.state = AgentState::Idle { confirmation: None };
            vec![Effect::Persist]
        }
    }

    /// Extract the current confirmation state (if any).
    fn state_confirmation(&self) -> Option<&PendingConfirmation> {
        if let AgentState::Idle {
            confirmation: Some(ref c),
        } = self.state
        {
            Some(c)
        } else {
            None
        }
    }

    /// Get the most recent suggested command from the conversation.
    fn current_command(&self) -> Option<String> {
        self.ctx
            .events
            .iter()
            .rev()
            .find_map(|e| e.as_command())
            .map(|s| s.to_string())
    }

    /// Check if the most recent command is dangerous.
    fn is_current_command_dangerous(&self) -> bool {
        self.ctx
            .events
            .iter()
            .rev()
            .find_map(|e| {
                if let ConversationEvent::ToolCall { name, input, .. } = e
                    && name == "suggest_command"
                {
                    let danger = input
                        .get("danger")
                        .and_then(|v| v.as_str())
                        .unwrap_or("low");
                    Some(danger == "high" || danger == "medium" || danger == "med")
                } else {
                    None
                }
            })
            .unwrap_or(false)
    }

    /// Handle a slash command by pushing an OOB event.
    fn handle_slash_command(&mut self, command: &str) {
        // /new is handled separately as Event::NewSession
        self.ctx.events.push(ConversationEvent::OutOfBandOutput {
            name: "System".to_string(),
            command: Some(command.to_string()),
            content: format!("Unknown command: {command}"),
        });
    }
}
