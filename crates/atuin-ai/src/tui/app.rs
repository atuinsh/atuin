//! The eye-declare v2 app: the FSM is the model, `update` is the driver
//! loop, and the live tail is a view over the not-yet-committed turns.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crossterm::event::KeyCode;
use eye_declare::{
    App, Ctx, Element, ElementExt, Fluent, Focus, FocusHandle, InputEvent, Keymap, Task, col, key,
    keymap, text,
};
use ratatui_core::style::{Color, Modifier, Style};
use tokio::sync::mpsc::UnboundedSender;
use tui_textarea::TextArea;

use crate::fsm::effects::{Effect, ExitAction, PermissionTarget, TimeoutKind};
use crate::fsm::events::{Event, PermissionChoice, PermissionResponse};
use crate::fsm::tools::ToolPreviewData;
use crate::fsm::{AgentFsm, AgentState};
use crate::tools::ClientToolCall;
use crate::tui::events::PermissionResult;
use crate::tui::persist::PersistJob;
use crate::tui::select::{SelectMsg, SelectState};
use crate::tui::slash::{SlashCommandRegistry, SlashCommandSearchResult};
use crate::tui::state::ConversationEvent;
use crate::tui::view;
use crate::tui::view::turn::{TurnBuilder, UiTurn, UiTurnKind};
use crate::usage::UsageSnapshot;

/// What the TUI resolves to, for the shell hook.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ExitOutcome {
    Execute(String),
    Insert(String),
    #[default]
    Cancel,
}

/// IO resources the app's effects need. Not part of the FSM's state.
/// Session persistence goes through the [`crate::tui::persist`] worker,
/// which owns the `SessionManager`.
pub(crate) struct IoContext {
    pub app_ctx: crate::context::AppContext,
    pub client_ctx: crate::context::ClientContext,
    pub persist: UnboundedSender<PersistJob>,
    pub file_tracker: crate::file_tracker::FileReadTracker,
    pub edit_permissions: crate::edit_permissions::EditPermissionCache,
    pub snapshot_store: Option<crate::snapshots::SnapshotStore>,
    pub skill_registry: crate::skills::SkillRegistry,
    pub user_context_cache: crate::user_context::UserContextCache,
}

/// The policy vocabulary. Mode-dependent resolution happens in `keymap()`
/// (rebuilt from the model every update), so each variant means exactly one
/// thing by the time it reaches `update`.
#[derive(Debug, Clone)]
pub(crate) enum Msg {
    /// Unclaimed key/paste, routed to the editor.
    Input(InputEvent),
    /// Insert a newline (Shift+Enter / Ctrl+J).
    Newline,
    /// Submit the editor's text (Enter with non-blank input).
    Submit,
    /// Accept the top slash suggestion (Tab while suggesting).
    AcceptSlashSuggestion,
    /// Run the suggested command, or confirm a dangerous one (Enter).
    ExecuteCommand,
    /// Exit with the suggested command inserted, not executed (Tab).
    InsertCommand,
    /// Move the permission-prompt cursor (Up/Down while asking).
    PermissionSelect(SelectMsg),
    /// Answer the permission prompt with the highlighted option (Enter).
    ConfirmPermission,
    /// Cancel: generation, or a pending confirmation (Esc).
    Cancel,
    /// Interrupt a running tool execution (Ctrl+C / Esc while executing).
    Interrupt,
    /// Retry after an error (Enter / r).
    Retry,
    /// Leave the TUI without a command.
    Quit,
    /// An FSM event from spawned work (stream frames, timeouts).
    Fsm(Event),
    /// Fresh credit-usage snapshot. Never reaches the FSM.
    Usage(UsageSnapshot),
}

pub(crate) struct AiApp {
    pub fsm: AgentFsm,
    /// `None` only in headless tests; every effect that needs IO degrades
    /// to a debug log without it.
    io: Option<IoContext>,
    /// The in-flight stream turn; dropping it (Esc, replacement) cancels
    /// the HTTP request.
    streaming: Option<Task>,
    /// Interrupt senders for executing shell tools. Firing one kills the
    /// child process; the detached tool stream still reports its outcome.
    tool_interrupts: HashMap<String, tokio::sync::oneshot::Sender<()>>,
    /// Cursor for the permission prompt, reset when the asked tool changes.
    permission_select: SelectState,
    /// Which tool the current prompt (and cursor) belongs to.
    permission_prompt_for: Option<String>,
    in_git_project: bool,
    /// Latest known credit usage (cached at startup, refreshed from Done
    /// frames). Rendered by the status bar (pickers slice).
    usage: Option<UsageSnapshot>,
    /// "Continuing previous session…" banner, frozen at startup.
    resume_notice: Option<String>,
    /// The editor as a plain model value; see `view::input` for why RefCell.
    input: RefCell<TextArea<'static>>,
    /// Focus system; only the editor takes focus today, but keymap
    /// fallthrough is focus-scoped by design.
    #[allow(dead_code)]
    focus: Focus,
    input_focus: FocusHandle,
    slash_registry: SlashCommandRegistry,
    skill_names: HashSet<String>,
    slash_results: Vec<SlashCommandSearchResult>,
    /// Conversation events already committed to scrollback via `ctx.push`.
    pushed_events: usize,
    /// Turns already committed — 0 means the next turn is the first (no
    /// leading blank row).
    pushed_turns: usize,
}

impl AiApp {
    pub fn new(
        fsm: AgentFsm,
        io: IoContext,
        resume_notice: Option<String>,
        slash_registry: SlashCommandRegistry,
        skill_names: HashSet<String>,
        usage: Option<UsageSnapshot>,
    ) -> Self {
        Self {
            in_git_project: io.app_ctx.git_root.is_some(),
            io: Some(io),
            usage,
            ..Self::headless(fsm, resume_notice, slash_registry, skill_names)
        }
    }

    /// An app without IO: effects that need it degrade to debug logs.
    /// Tests drive stream/timeout flows by processing `Msg::Fsm` directly.
    #[cfg_attr(not(test), allow(dead_code))]
    fn headless(
        fsm: AgentFsm,
        resume_notice: Option<String>,
        slash_registry: SlashCommandRegistry,
        skill_names: HashSet<String>,
    ) -> Self {
        let focus = Focus::new();
        let input_focus = focus.handle();
        input_focus.focus();
        Self {
            fsm,
            io: None,
            streaming: None,
            tool_interrupts: HashMap::new(),
            permission_select: SelectState::default(),
            permission_prompt_for: None,
            in_git_project: false,
            usage: None,
            resume_notice,
            input: RefCell::new(view::input::new_textarea()),
            focus,
            input_focus,
            slash_registry,
            skill_names,
            slash_results: Vec::new(),
            pushed_events: 0,
            pushed_turns: 0,
        }
    }

    fn is_busy(&self) -> bool {
        matches!(self.fsm.state, AgentState::Turn { .. })
    }

    fn has_confirmation(&self) -> bool {
        matches!(
            self.fsm.state,
            AgentState::Idle {
                confirmation: Some(_)
            }
        )
    }

    fn input_active(&self) -> bool {
        matches!(self.fsm.state, AgentState::Idle { .. }) && !self.has_confirmation()
    }

    fn is_input_blank(&self) -> bool {
        self.input.borrow().is_empty()
    }

    fn has_executing_preview(&self) -> bool {
        self.fsm.ctx.tools.has_executing_preview()
    }

    /// Whether the visible conversation carries a suggested command.
    fn has_command(&self) -> bool {
        self.visible_events()
            .iter()
            .any(|e| e.as_command().is_some())
    }

    fn visible_events(&self) -> &[crate::tui::state::ConversationEvent] {
        let events = &self.fsm.ctx.events;
        let start = self.fsm.ctx.view_start_index.min(events.len());
        &events[start..]
    }

    fn footer_text(&self) -> &'static str {
        match &self.fsm.state {
            AgentState::Idle { confirmation: None } => {
                if self.has_command() && self.is_input_blank() {
                    "[Enter] Execute suggested command  [Tab] Insert Command"
                } else {
                    "[Enter] Send  [Shift+Enter] New line  [Esc] Exit"
                }
            }
            AgentState::Idle {
                confirmation: Some(_),
            } => "[Enter] Confirm dangerous command  [Esc] Cancel",
            AgentState::Turn { .. } => "[Esc] Cancel",
            AgentState::Error(_) => "[Enter]/[r] Retry  [Esc] Exit",
        }
    }

    /// Turns not yet committed to scrollback, built fresh from the FSM's
    /// events past the pushed frontier.
    fn live_turns(&self) -> Vec<UiTurn> {
        let events = &self.fsm.ctx.events;
        let start = self
            .fsm
            .ctx
            .view_start_index
            .max(self.pushed_events)
            .min(events.len());
        let mut builder = TurnBuilder::new(&self.fsm.ctx.tools);
        for event in &events[start..] {
            builder.add_event(event);
        }
        // Streaming text lives in current_response until the FSM commits it
        // on stream end; inject it so the live turn shows it as it arrives.
        let streaming_text = self.fsm.ctx.current_response.trim_start();
        if !streaming_text.is_empty() {
            builder.add_event(&ConversationEvent::Text {
                content: streaming_text.to_string(),
            });
        }
        builder.build()
    }

    /// Recompute slash suggestions from the editor's text.
    fn refresh_slash(&mut self) {
        let query = {
            let input = self.input.borrow();
            let text = input.lines().join("\n");
            text.strip_prefix('/').map(str::to_string)
        };
        match query {
            Some(query) => {
                let mut results = self.slash_registry.search_fuzzy(&query);
                results.sort_by(|a, b| {
                    b.relevance
                        .partial_cmp(&a.relevance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                self.slash_results = results;
            }
            None => self.slash_results.clear(),
        }
    }

    /// Feed an event to the FSM and act on its effects.
    fn handle_fsm(&mut self, event: Event, ctx: &mut Ctx<'_, Self>) {
        let effects = self.fsm.handle(event);
        tracing::trace!(?effects, state = ?self.fsm.state, "FSM transition");
        for effect in effects {
            match effect {
                Effect::ExitApp(action) => ctx.exit(match action {
                    ExitAction::Execute(cmd) => ExitOutcome::Execute(cmd),
                    ExitAction::Insert(cmd) => ExitOutcome::Insert(cmd),
                    ExitAction::Cancel => ExitOutcome::Cancel,
                }),
                Effect::StartStream {
                    messages,
                    session_id,
                } => self.start_stream(messages, session_id, ctx),
                // Dropping the Task drops the bridge's generator, which
                // drops the HTTP stream. Esc-cancels-generation is this.
                Effect::AbortStream => self.streaming = None,
                Effect::Persist => self.persist(),
                Effect::ArchiveSession => {
                    if let Some(io) = &self.io {
                        let _ = io.persist.send(PersistJob::Archive);
                    }
                }
                Effect::ScheduleTimeout {
                    timeout_id,
                    duration,
                    kind,
                } => {
                    // Stale firings are guarded by timeout_id in the FSM,
                    // so a detached sleep is safe.
                    let event = match kind {
                        TimeoutKind::Confirmation => Event::ConfirmationTimeout { timeout_id },
                        TimeoutKind::ToolExecution { tool_id } => Event::ToolExecutionTimeout {
                            timeout_id,
                            tool_id,
                        },
                    };
                    ctx.perform(async move {
                        tokio::time::sleep(duration).await;
                        Msg::Fsm(event)
                    })
                    .detach();
                }
                Effect::CheckPermission { tool_id, tool } => {
                    self.check_permission(tool_id, tool, ctx)
                }
                Effect::ExecuteTool { tool_id, tool } => self.execute_tool(tool_id, tool, ctx),
                Effect::AbortTool { tool_id } => {
                    if let Some(interrupt_tx) = self.tool_interrupts.remove(&tool_id) {
                        let _ = interrupt_tx.send(());
                    }
                }
                Effect::CacheSessionGrant { path } => {
                    if let Some(io) = &mut self.io {
                        io.edit_permissions.grant(path);
                    }
                }
                Effect::WritePermissionRule {
                    target,
                    rule,
                    disposition,
                } => {
                    let Some(io) = &self.io else { continue };
                    let file_path = match target {
                        PermissionTarget::Project => {
                            let project_root = io
                                .app_ctx
                                .git_root
                                .clone()
                                .or_else(|| std::env::current_dir().ok())
                                .unwrap_or_else(|| PathBuf::from("."));
                            crate::permissions::writer::project_permissions_path(&project_root)
                        }
                        PermissionTarget::Global => {
                            crate::permissions::writer::global_permissions_path()
                        }
                    };
                    tokio::spawn(async move {
                        if let Err(e) =
                            crate::permissions::writer::write_rule(&file_path, &rule, disposition)
                                .await
                        {
                            tracing::error!("Failed to write permission rule: {e}");
                        }
                    });
                }
                Effect::LoadSkill { name, arguments } => {
                    let Some(io) = &self.io else { continue };
                    let registry = io.skill_registry.clone();
                    let shell = io
                        .client_ctx
                        .shell
                        .clone()
                        .unwrap_or_else(|| "sh".to_string());
                    ctx.perform(async move {
                        let content = crate::tui::tools_exec::load_skill_content(
                            &registry,
                            &name,
                            &shell,
                            arguments.as_deref(),
                        )
                        .await;
                        Msg::Fsm(Event::SkillLoaded {
                            name,
                            arguments,
                            content,
                        })
                    })
                    .detach();
                }
                // TODO(v2 port, pickers slice): FetchModels + SaveModelSelection.
                other => tracing::debug!(effect = ?other, "effect deferred (v2 port)"),
            }
        }
        self.sync_permission_prompt();
        self.push_ready_turns(ctx);
    }

    /// Keep the prompt cursor tied to the tool being asked about: a new
    /// question starts from the top.
    fn sync_permission_prompt(&mut self) {
        let current = self
            .fsm
            .ctx
            .tools
            .awaiting_permission()
            .map(|t| t.id.clone());
        if current != self.permission_prompt_for {
            self.permission_prompt_for = current;
            self.permission_select = SelectState::default();
        }
    }

    /// Resolve a permission check: fast paths synchronously (yolo,
    /// auto-approved tools, cached per-session grants), the rules resolver
    /// as a spawned future. Without IO (headless), everything resolves to
    /// Ask — the same fallback the resolver uses on errors.
    fn check_permission(&mut self, tool_id: String, tool: ClientToolCall, ctx: &mut Ctx<'_, Self>) {
        let Some(io) = &self.io else {
            self.handle_fsm(
                Event::PermissionResolved {
                    tool_id,
                    response: PermissionResponse::Ask,
                },
                ctx,
            );
            return;
        };

        if io.app_ctx.yolo || tool.is_auto_approved() {
            self.handle_fsm(
                Event::PermissionResolved {
                    tool_id,
                    response: PermissionResponse::Allowed,
                },
                ctx,
            );
            return;
        }

        if let Some(resolved) = tool.resolved_file_path()
            && io.edit_permissions.has_valid_grant(&resolved)
        {
            self.handle_fsm(
                Event::PermissionResolved {
                    tool_id,
                    response: PermissionResponse::SessionGranted,
                },
                ctx,
            );
            return;
        }

        let working_dir = tool
            .target_dir()
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        ctx.perform(async move {
            use crate::permissions::check::PermissionResponse as Check;
            let response =
                match crate::permissions::resolver::PermissionResolver::new(working_dir).await {
                    Ok(resolver) => match resolver.check(&tool).await {
                        Ok(Check::Allowed) => PermissionResponse::Allowed,
                        Ok(Check::Denied) => PermissionResponse::Denied,
                        Ok(Check::Ask) | Err(_) => PermissionResponse::Ask,
                    },
                    Err(_) => PermissionResponse::Ask,
                };
            Msg::Fsm(Event::PermissionResolved { tool_id, response })
        })
        .detach();
    }

    /// Execute an approved tool. Shell commands stream previews from a
    /// detached task (interrupt ≠ cancel: the outcome must still arrive);
    /// file tools are fast and run inline, as in v1; the rest are one-shot
    /// futures.
    fn execute_tool(&mut self, tool_id: String, tool: ClientToolCall, ctx: &mut Ctx<'_, Self>) {
        match tool {
            ClientToolCall::Shell(shell_call) => {
                let (interrupt_tx, interrupt_rx) = tokio::sync::oneshot::channel();
                self.tool_interrupts.insert(tool_id.clone(), interrupt_tx);
                ctx.spawn(crate::tui::tools_exec::shell_stream(
                    tool_id,
                    shell_call,
                    interrupt_rx,
                ))
                .detach();
            }
            ClientToolCall::Edit(edit_call) => {
                let resolved = edit_call.resolved_path();

                let old_content = std::fs::read(&resolved).ok();
                if let Some(content) = &old_content
                    && let Some(io) = &mut self.io
                    && let Some(store) = &mut io.snapshot_store
                    && let Err(e) = store.ensure_snapshot(&resolved, content)
                {
                    tracing::warn!("Failed to snapshot before edit: {e}");
                }

                let (outcome, new_content) = match &self.io {
                    Some(io) => edit_call.execute(&resolved, &io.file_tracker),
                    None => edit_call.execute(&resolved, &Default::default()),
                };

                if let Some(new_bytes) = &new_content
                    && let Some(io) = &mut self.io
                    && let Ok(mtime) = std::fs::metadata(&resolved).and_then(|m| m.modified())
                {
                    io.file_tracker
                        .update_after_edit(&resolved, new_bytes, mtime);
                }

                let preview = match (&old_content, &new_content) {
                    (Some(old_bytes), Some(new_bytes)) => {
                        let old_str = String::from_utf8_lossy(old_bytes);
                        let new_str = String::from_utf8_lossy(new_bytes);
                        let diff = crate::diff::EditPreview::compute(&old_str, &new_str);
                        (!diff.hunks.is_empty()).then_some(ToolPreviewData::Edit(diff))
                    }
                    _ => None,
                };

                self.handle_fsm(
                    Event::ToolExecutionDone {
                        tool_id,
                        outcome,
                        preview,
                    },
                    ctx,
                );
            }
            ClientToolCall::Write(write_call) => {
                let resolved = write_call.resolved_path();

                if let Ok(content) = std::fs::read(&resolved)
                    && let Some(io) = &mut self.io
                    && let Some(store) = &mut io.snapshot_store
                    && let Err(e) = store.ensure_snapshot(&resolved, &content)
                {
                    tracing::warn!("Failed to snapshot before write: {e}");
                }

                let (outcome, written_bytes) = write_call.execute(&resolved);

                if let Some(new_bytes) = &written_bytes
                    && let Some(io) = &mut self.io
                    && let Ok(mtime) = std::fs::metadata(&resolved).and_then(|m| m.modified())
                {
                    io.file_tracker
                        .update_after_edit(&resolved, new_bytes, mtime);
                }

                let preview = (!outcome.is_error()).then(|| {
                    ToolPreviewData::Write(crate::diff::WritePreview::from_content(
                        &write_call.content,
                    ))
                });

                self.handle_fsm(
                    Event::ToolExecutionDone {
                        tool_id,
                        outcome,
                        preview,
                    },
                    ctx,
                );
            }
            ClientToolCall::Read(read_call) => {
                let outcome = read_call.execute();

                if !outcome.is_error() {
                    let resolved = read_call.resolved_path();
                    if resolved.is_file()
                        && let Some(io) = &mut self.io
                        && let Ok(content) = std::fs::read(&resolved)
                        && let Ok(mtime) = std::fs::metadata(&resolved).and_then(|m| m.modified())
                    {
                        io.file_tracker.record_read(resolved, &content, mtime);
                    }
                }

                self.handle_fsm(
                    Event::ToolExecutionDone {
                        tool_id,
                        outcome,
                        preview: None,
                    },
                    ctx,
                );
            }
            ClientToolCall::AtuinHistory(history_call) => {
                let Some(io) = &self.io else { return };
                let db = io.app_ctx.history_db.clone();
                ctx.perform(async move {
                    let outcome = history_call.execute(&db).await;
                    Msg::Fsm(Event::ToolExecutionDone {
                        tool_id,
                        outcome,
                        preview: None,
                    })
                })
                .detach();
            }
            ClientToolCall::AtuinOutput(output_call) => {
                ctx.perform(async move {
                    let outcome = output_call.execute().await;
                    Msg::Fsm(Event::ToolExecutionDone {
                        tool_id,
                        outcome,
                        preview: None,
                    })
                })
                .detach();
            }
            ClientToolCall::LoadSkill(skill_call) => {
                let Some(io) = &self.io else { return };
                let registry = io.skill_registry.clone();
                let shell = io
                    .client_ctx
                    .shell
                    .clone()
                    .unwrap_or_else(|| "sh".to_string());
                ctx.perform(async move {
                    let content = crate::tui::tools_exec::load_skill_content(
                        &registry,
                        &skill_call.name,
                        &shell,
                        None,
                    )
                    .await;
                    Msg::Fsm(Event::ToolExecutionDone {
                        tool_id,
                        outcome: crate::tools::ToolOutcome::Success(content),
                        preview: None,
                    })
                })
                .detach();
            }
        }
    }

    /// Launch the chat stream as a spawned message stream, replacing (and
    /// thereby cancelling) any previous one.
    fn start_stream(
        &mut self,
        messages: Vec<serde_json::Value>,
        session_id: Option<String>,
        ctx: &mut Ctx<'_, Self>,
    ) {
        let Some(io) = &self.io else {
            tracing::debug!("headless app: stream not started");
            return;
        };
        let (skill_summaries, skill_overflow) = io.skill_registry.server_skills();
        let request = crate::stream::ChatRequest::new(
            messages,
            session_id,
            &io.app_ctx.capabilities,
            io.app_ctx.daemon_enabled,
            self.fsm.ctx.invocation_id.clone(),
            self.fsm.ctx.model.clone(),
        );
        self.streaming = Some(ctx.spawn(crate::tui::bridge::stream_bridge(
            request,
            io.app_ctx.clone(),
            io.client_ctx.clone(),
            skill_summaries,
            skill_overflow,
            io.user_context_cache.clone(),
        )));
    }

    /// Queue a full-session snapshot on the persist worker. Jobs apply in
    /// channel order, so a later snapshot can never be overwritten by an
    /// earlier one.
    fn persist(&self) {
        let Some(io) = &self.io else { return };
        let _ = io.persist.send(PersistJob::Session {
            events: self.fsm.ctx.events.clone(),
            server_session_id: self.fsm.ctx.session_id.clone(),
            file_tracker: io.file_tracker.to_json().ok(),
            edit_permissions: io.edit_permissions.to_json().ok(),
        });
    }

    /// Commit sealed turns to scrollback. A turn is sealed when nothing can
    /// mutate it anymore; an in-flight agent turn stays live.
    fn push_ready_turns(&mut self, ctx: &mut Ctx<'_, Self>) {
        // In Error state the failed turn stays live: Retry continues the
        // same turn, and pushing a partial turn would split it.
        if matches!(self.fsm.state, AgentState::Error(_)) {
            return;
        }
        let turns = self.live_turns();
        if turns.is_empty() {
            return;
        }
        if self.is_busy()
            && matches!(
                turns.last().map(|t| &t.kind),
                Some(UiTurnKind::Agent { .. })
            )
        {
            // Mid-stream: the whole in-flight agent turn stays in the tail
            // (statuses and text still mutate). It seals when the FSM
            // returns to Idle. Frontier-within-a-turn pushing is a later
            // optimization if tall turns ever hurt.
            return;
        }

        if self.pushed_turns == 0
            && let Some(notice) = self.resume_notice.take()
        {
            ctx.push(
                text(notice).style(
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            );
        }
        for (i, turn) in turns.iter().enumerate() {
            ctx.push(view::turn_view(
                turn,
                self.pushed_turns == 0 && i == 0,
                false,
                false,
            ));
            self.pushed_turns += 1;
        }
        self.pushed_events = self.fsm.ctx.events.len();
    }

    /// Translate submitted text into an FSM event: built-in slash commands,
    /// skills, or a plain user message.
    fn dispatch_submit(&self, input: String) -> Event {
        if input == "/new" {
            Event::NewSession
        } else if input == "/model" || input.starts_with("/model ") {
            Event::OpenModelPicker
        } else if input.starts_with('/') {
            if let Some((name, arguments)) = self.resolve_skill(&input) {
                Event::RequestSkillLoad { name, arguments }
            } else {
                let content = self.resolve_slash_command(&input);
                Event::SlashCommand {
                    command: input,
                    content,
                }
            }
        } else {
            Event::UserSubmit(input)
        }
    }

    /// Whether a slash invocation names a registered skill. Built-in slash
    /// commands take precedence: a skill can't shadow them.
    fn resolve_skill(&self, input: &str) -> Option<(String, Option<String>)> {
        let after_slash = input.trim_start_matches('/');
        let name = after_slash.split_whitespace().next()?.to_string();

        if !self.skill_names.contains(&name) || self.slash_registry.contains_builtin(&name) {
            return None;
        }

        let args = after_slash
            .strip_prefix(&name)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        Some((name, args))
    }

    fn resolve_slash_command(&self, command: &str) -> String {
        match command.trim() {
            "/reload" => {
                if let Some(io) = &self.io {
                    io.user_context_cache.invalidate();
                }
                "Context files will be reloaded on the next request.".to_string()
            }
            "/help" => {
                let commands = self
                    .slash_registry
                    .get_commands()
                    .iter()
                    .map(|cmd| format!("- `/{}` — {}", cmd.name, cmd.description))
                    .collect::<Vec<_>>()
                    .join("\n");
                include_str!("content/help.md").replace("{commands}", &commands)
            }
            other => format!("Unknown command: {other}"),
        }
    }

    fn submit(&mut self, ctx: &mut Ctx<'_, Self>) {
        let input = {
            let editor = self.input.get_mut();
            let text = editor.lines().join("\n").trim().to_string();
            if text.is_empty() {
                return;
            }
            editor.clear();
            text
        };
        self.slash_results.clear();
        let event = self.dispatch_submit(input);
        self.handle_fsm(event, ctx);
    }
}

impl App for AiApp {
    type Msg = Msg;
    type Output = ExitOutcome;

    fn update(&mut self, msg: Msg, ctx: &mut Ctx<'_, Self>) {
        match msg {
            Msg::Input(event) => {
                if !self.input_active() {
                    return;
                }
                match event {
                    InputEvent::Key(k) => {
                        self.input.get_mut().input(k);
                    }
                    InputEvent::Paste(s) => {
                        self.input.get_mut().insert_str(s);
                    }
                }
                self.refresh_slash();
            }
            Msg::Newline => {
                if self.input_active() {
                    self.input.get_mut().insert_newline();
                }
            }
            Msg::AcceptSlashSuggestion => {
                if let Some(first) = self.slash_results.first() {
                    let name = first.command.name.clone();
                    let editor = self.input.get_mut();
                    editor.clear();
                    editor.insert_str(format!("/{name}"));
                    self.refresh_slash();
                }
            }
            Msg::Submit => self.submit(ctx),
            Msg::PermissionSelect(sel) => {
                let len = self
                    .fsm
                    .ctx
                    .tools
                    .awaiting_permission()
                    .map(|t| view::permission_options(&t.tool, self.in_git_project).len())
                    .unwrap_or(0);
                self.permission_select.handle(sel, len);
            }
            Msg::ConfirmPermission => {
                let Some(tool) = self.fsm.ctx.tools.awaiting_permission() else {
                    return;
                };
                let tool_id = tool.id.clone();
                let options = view::permission_options(&tool.tool, self.in_git_project);
                let Some((_, result)) = options.get(self.permission_select.cursor) else {
                    return;
                };
                let choice = match result {
                    PermissionResult::Allow => PermissionChoice::Allow,
                    PermissionResult::AllowFileForSession => PermissionChoice::AllowForSession,
                    PermissionResult::AlwaysAllowInDir => PermissionChoice::AlwaysAllowInProject,
                    PermissionResult::AlwaysAllow => PermissionChoice::AlwaysAllow,
                    PermissionResult::Deny => PermissionChoice::Deny,
                };
                self.handle_fsm(Event::PermissionUserChoice { tool_id, choice }, ctx);
            }
            Msg::ExecuteCommand => self.handle_fsm(Event::ExecuteCommand, ctx),
            Msg::InsertCommand => self.handle_fsm(Event::InsertCommand, ctx),
            Msg::Cancel => self.handle_fsm(Event::Cancel, ctx),
            Msg::Interrupt => self.handle_fsm(Event::InterruptTools, ctx),
            Msg::Retry => self.handle_fsm(Event::Retry, ctx),
            Msg::Quit => ctx.exit(ExitOutcome::Cancel),
            Msg::Fsm(event) => self.handle_fsm(event, ctx),
            Msg::Usage(snapshot) => {
                if let Some(io) = &self.io {
                    let _ = io.persist.send(PersistJob::Usage {
                        key: crate::usage::cache_key(&io.app_ctx.token),
                        snapshot: snapshot.clone(),
                    });
                }
                self.usage = Some(snapshot);
            }
        }
    }

    fn tail(&self) -> impl Element + '_ {
        let turns = self.live_turns();
        let busy = self.is_busy();
        let last = turns.len().saturating_sub(1);
        let asking = self.fsm.ctx.tools.awaiting_permission();
        let needs_pending_banner = busy
            && !matches!(
                turns.last().map(|t| &t.kind),
                Some(UiTurnKind::Agent { .. })
            );

        col()
            .when_some(
                (self.pushed_turns == 0)
                    .then_some(self.resume_notice.clone())
                    .flatten(),
                |c, notice| {
                    c.child(
                        text(notice).style(
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        ),
                    )
                },
            )
            .children(turns.iter().enumerate().map(|(i, turn)| {
                view::turn_view(
                    turn,
                    self.pushed_turns == 0 && i == 0,
                    busy && i == last,
                    asking.is_some(),
                )
            }))
            .when(needs_pending_banner, |c| {
                c.child(view::agent_turn_view(&[], true, asking.is_some()))
            })
            .when_some(
                match &self.fsm.state {
                    AgentState::Error(msg) => Some(msg.clone()),
                    _ => None,
                },
                |c, msg| {
                    c.child(
                        text("Error: ")
                            .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                            .span(msg, Style::default().fg(Color::Red))
                            .pad_left(2)
                            .pad_top(1),
                    )
                },
            )
            .when_some(asking, |c, tool| {
                c.child(view::permission_prompt_view(
                    tool,
                    self.in_git_project,
                    self.permission_select.cursor,
                ))
            })
            .when(asking.is_none(), |c| {
                c.child(view::input::input_area(
                    &self.input,
                    self.input_active(),
                    self.footer_text(),
                    self.is_input_blank() && self.has_command() && self.input_active(),
                    &self.slash_results,
                ))
            })
    }

    fn keymap(&self) -> Keymap<Msg> {
        let mut km = keymap();

        // Ctrl+C: interrupt a running tool execution, else exit.
        km = km.on_override(
            key(KeyCode::Char('c')).ctrl(),
            if self.has_executing_preview() {
                Msg::Interrupt
            } else {
                Msg::Quit
            },
        );

        // Esc: mode-dependent — the old AppMode match, as data.
        km = km.on(
            key(KeyCode::Esc),
            if self.is_busy() {
                Msg::Cancel
            } else if self.has_executing_preview() {
                Msg::Interrupt
            } else if self.has_confirmation() {
                Msg::Cancel
            } else {
                Msg::Quit
            },
        );

        if matches!(self.fsm.state, AgentState::Error(_)) {
            km = km
                .on(key(KeyCode::Enter), Msg::Retry)
                .on(key(KeyCode::Char('r')), Msg::Retry);
        }

        if self.has_confirmation() {
            km = km.on(key(KeyCode::Enter), Msg::ExecuteCommand);
        }

        if self.fsm.ctx.tools.awaiting_permission().is_some() {
            km = km
                .on(key(KeyCode::Enter), Msg::ConfirmPermission)
                .merge(SelectState::keymap().map(Msg::PermissionSelect));
        }

        if self.input_active() {
            if !self.slash_results.is_empty() {
                km = km.on(key(KeyCode::Tab), Msg::AcceptSlashSuggestion);
            }
            if self.is_input_blank() && self.has_command() {
                km = km
                    .on(key(KeyCode::Enter), Msg::ExecuteCommand)
                    .on(key(KeyCode::Tab), Msg::InsertCommand);
            } else {
                km = km.on(key(KeyCode::Enter), Msg::Submit);
            }
            km = km
                .on(key(KeyCode::Enter).shift(), Msg::Newline)
                .on(key(KeyCode::Char('j')).ctrl(), Msg::Newline)
                .fallthrough(&self.input_focus, Msg::Input);
        }

        km
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::ConversationEvent;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use eye_declare::Runtime;
    use eye_declare_engine::test_terminal::TestTerminal;

    fn fixture_app() -> AiApp {
        let mut fsm = AgentFsm::new(vec![], "test-invocation".into());
        fsm.ctx.events.push(ConversationEvent::UserMessage {
            content: "list my files".into(),
        });
        fsm.ctx.events.push(ConversationEvent::Text {
            content: "Use `ls` to list files.".into(),
        });
        app_with(fsm)
    }

    fn app_with(fsm: AgentFsm) -> AiApp {
        AiApp::headless(fsm, None, SlashCommandRegistry::default(), HashSet::new())
    }

    /// Runtime + VTE terminal pair. Every byte the runtime emits — including
    /// per-event output from `handle` — must reach the terminal, or its
    /// state diverges from what the diffing runtime believes is on screen.
    struct Harness {
        rt: Runtime<AiApp>,
        term: TestTerminal,
    }

    impl Harness {
        fn new(app: AiApp) -> Self {
            let mut rt = Runtime::new(app, 60, 24);
            let mut term = TestTerminal::new(60, 24);
            term.feed(&rt.present());
            Self { rt, term }
        }

        fn press(&mut self, code: KeyCode) -> Option<ExitOutcome> {
            self.press_mod(code, KeyModifiers::NONE)
        }

        fn press_mod(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<ExitOutcome> {
            let (bytes, exit) = self
                .rt
                .handle(InputEvent::Key(KeyEvent::new(code, modifiers)));
            self.term.feed(&bytes);
            exit
        }

        fn type_str(&mut self, s: &str) {
            for ch in s.chars() {
                self.press(KeyCode::Char(ch));
            }
        }

        /// Deliver a message as spawned work would (e.g. stream frames).
        fn process(&mut self, msg: Msg) -> Option<ExitOutcome> {
            let (bytes, exit) = self.rt.process(msg);
            self.term.feed(&bytes);
            exit
        }

        fn stream(&mut self, event: Event) {
            self.process(Msg::Fsm(event));
        }

        fn app(&self) -> &AiApp {
            self.rt.app()
        }

        fn screen(&self) -> String {
            self.term.viewport_lines().join("\n")
        }

        /// Scrollback + viewport, for content that has scrolled off.
        fn all_lines(&self) -> String {
            [self.term.scrollback_lines(), self.term.viewport_lines()]
                .concat()
                .join("\n")
        }
    }

    fn suggest_command_fsm(cmd: &str) -> AgentFsm {
        let mut fsm = AgentFsm::new(vec![], "test-invocation".into());
        fsm.ctx.events.push(ConversationEvent::ToolCall {
            id: "t1".into(),
            name: "suggest_command".into(),
            input: serde_json::json!({ "command": cmd }),
        });
        fsm
    }

    #[test]
    fn tail_renders_conversation_turns() {
        let h = Harness::new(fixture_app());

        let screen = h.screen();
        assert!(screen.contains(" You"), "user label missing:\n{screen}");
        assert!(
            screen.contains("list my files"),
            "user text missing:\n{screen}"
        );
        assert!(
            screen.contains(" Atuin AI"),
            "agent label missing:\n{screen}"
        );
        assert!(
            screen.contains("Use ls to list files."),
            "agent markdown missing:\n{screen}"
        );
    }

    #[test]
    fn esc_exits_with_cancel() {
        let mut h = Harness::new(fixture_app());
        assert_eq!(h.press(KeyCode::Esc), Some(ExitOutcome::Cancel));
    }

    #[test]
    fn typing_renders_in_the_editor() {
        let mut h = Harness::new(fixture_app());
        h.type_str("hi there");

        let screen = h.screen();
        assert!(screen.contains("hi there"), "typed text missing:\n{screen}");
        assert!(
            screen.contains("Generate a command or ask a question"),
            "panel title missing:\n{screen}"
        );
    }

    #[test]
    fn enter_submits_and_pushes_the_user_turn() {
        let mut h = Harness::new(fixture_app());

        h.type_str("hello agent");
        let exit = h.press(KeyCode::Enter);
        assert_eq!(exit, None);

        let app = h.app();
        assert!(matches!(
            app.fsm.ctx.events.last(),
            Some(ConversationEvent::UserMessage { content }) if content == "hello agent"
        ));
        assert!(app.is_input_blank(), "editor should clear on submit");
        assert!(app.is_busy(), "fsm should be in a turn");
        assert!(app.pushed_turns > 0, "turns should have been pushed");

        // The submitted turn is now committed content; the tail shows the
        // pending-agent banner.
        let all = h.all_lines();
        assert!(all.contains("hello agent"), "pushed turn missing:\n{all}");
        assert!(all.contains(" Atuin AI"), "pending banner missing:\n{all}");
    }

    #[test]
    fn blank_enter_executes_suggested_command() {
        let mut h = Harness::new(app_with(suggest_command_fsm("ls -la")));
        assert_eq!(
            h.press(KeyCode::Enter),
            Some(ExitOutcome::Execute("ls -la".into()))
        );
    }

    #[test]
    fn tab_inserts_suggested_command() {
        let mut h = Harness::new(app_with(suggest_command_fsm("ls -la")));
        assert_eq!(
            h.press(KeyCode::Tab),
            Some(ExitOutcome::Insert("ls -la".into()))
        );
    }

    #[test]
    fn typing_disarms_command_execution() {
        let mut h = Harness::new(app_with(suggest_command_fsm("rm -rf /")));
        h.type_str("actually, explain first");
        // Enter now submits the question instead of executing the command.
        assert_eq!(h.press(KeyCode::Enter), None);
        assert!(matches!(
            h.app().fsm.ctx.events.last(),
            Some(ConversationEvent::UserMessage { .. })
        ));
    }

    #[test]
    fn slash_suggestions_and_tab_accept() {
        let mut h = Harness::new(fixture_app());
        h.type_str("/he");

        let screen = h.screen();
        assert!(
            screen.contains("Show help information"),
            "suggestion missing:\n{screen}"
        );

        h.press(KeyCode::Tab);
        assert_eq!(h.app().input.borrow().lines(), ["/help".to_string()]);
    }

    #[test]
    fn ctrl_j_inserts_newline() {
        let mut h = Harness::new(fixture_app());
        h.type_str("a");
        h.press_mod(KeyCode::Char('j'), KeyModifiers::CONTROL);
        h.type_str("b");
        assert_eq!(
            h.app().input.borrow().lines(),
            ["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn shift_enter_inserts_newline() {
        let mut h = Harness::new(fixture_app());
        h.type_str("a");
        h.press_mod(KeyCode::Enter, KeyModifiers::SHIFT);
        h.type_str("b");
        assert_eq!(h.app().input.borrow().lines().len(), 2);
    }

    #[test]
    fn streaming_text_renders_live_and_seals_on_done() {
        let mut h = Harness::new(app_with(AgentFsm::new(vec![], "t".into())));
        h.type_str("what is atuin?");
        h.press(KeyCode::Enter);

        h.stream(Event::StreamStarted);
        h.stream(Event::StreamChunk("Atuin is a ".into()));
        h.stream(Event::StreamChunk("shell history tool.".into()));

        // Mid-stream: text is visible in the live tail, nothing new pushed.
        let pushed_mid = h.app().pushed_turns;
        assert!(h.app().is_busy());
        assert!(
            h.screen().contains("Atuin is a shell history tool."),
            "streaming text missing:\n{}",
            h.screen()
        );

        h.stream(Event::StreamDone {
            session_id: "s1".into(),
        });

        let app = h.app();
        assert!(!app.is_busy(), "turn should be over");
        assert!(
            app.pushed_turns > pushed_mid,
            "agent turn should seal to scrollback on Done"
        );
        assert_eq!(
            app.pushed_events,
            app.fsm.ctx.events.len(),
            "frontier should cover all events"
        );
        // The sealed turn is on the terminal, and the live tail no longer
        // repeats it.
        let all = h.all_lines();
        assert!(all.contains("Atuin is a shell history tool."));
        assert!(!h.app().fsm.ctx.current_response.contains("Atuin"));
    }

    #[test]
    fn esc_mid_stream_cancels_generation() {
        let mut h = Harness::new(app_with(AgentFsm::new(vec![], "t".into())));
        h.type_str("hello");
        h.press(KeyCode::Enter);
        h.stream(Event::StreamStarted);
        h.stream(Event::StreamChunk("partial".into()));

        assert_eq!(h.press(KeyCode::Esc), None, "Esc cancels, not exits");
        assert!(!h.app().is_busy(), "cancel should end the turn");
        // A second Esc from Idle exits.
        assert_eq!(h.press(KeyCode::Esc), Some(ExitOutcome::Cancel));
    }

    #[test]
    fn stream_error_offers_retry() {
        let mut h = Harness::new(app_with(AgentFsm::new(vec![], "t".into())));
        h.type_str("hello");
        h.press(KeyCode::Enter);
        h.stream(Event::StreamStarted);
        let pushed_before = h.app().pushed_turns;
        h.stream(Event::StreamError("connection lost".into()));

        assert!(matches!(h.app().fsm.state, AgentState::Error(_)));
        assert_eq!(
            h.app().pushed_turns,
            pushed_before,
            "failed turn must stay live for retry"
        );
        let screen = h.screen();
        assert!(
            screen.contains("Error: connection lost"),
            "error line missing:\n{screen}"
        );
        assert!(
            screen.contains("[Enter]/[r] Retry"),
            "retry footer:\n{screen}"
        );

        h.press(KeyCode::Char('r'));
        assert!(h.app().is_busy(), "retry should restart the turn");
    }

    #[test]
    fn usage_snapshot_is_stored() {
        let mut h = Harness::new(app_with(AgentFsm::new(vec![], "t".into())));
        let bucket = crate::usage::UsageBucket { used: 1, limit: 10 };
        let snapshot = crate::usage::UsageSnapshot {
            period: "calendar_monthly".into(),
            resets_at: "2026-08-01T00:00:00Z".into(),
            requests: bucket.clone(),
            input: bucket.clone(),
            output: bucket,
        };
        h.process(Msg::Usage(snapshot.clone()));
        assert_eq!(h.app().usage, Some(snapshot));
    }

    /// Drive a turn to the point where a shell tool awaits permission.
    /// Headless permission checks resolve to Ask (no rules to consult),
    /// which is exactly the prompt path.
    fn harness_awaiting_shell_permission() -> Harness {
        let mut h = Harness::new(app_with(AgentFsm::new(
            vec!["client_v1_execute_shell_command".into()],
            "t".into(),
        )));
        h.type_str("run something");
        h.press(KeyCode::Enter);
        h.stream(Event::StreamStarted);
        h.stream(Event::StreamToolCall {
            id: "tool-1".into(),
            name: "execute_shell_command".into(),
            input: serde_json::json!({ "command": "echo hi" }),
        });
        h.stream(Event::StreamDone {
            session_id: "s1".into(),
        });
        assert!(
            h.app().fsm.ctx.tools.awaiting_permission().is_some(),
            "fixture should end awaiting permission"
        );
        h
    }

    #[test]
    fn permission_prompt_renders_and_replaces_input() {
        let h = harness_awaiting_shell_permission();
        let screen = h.screen();
        assert!(
            screen.contains("Atuin AI would like to run: "),
            "prompt line missing:\n{screen}"
        );
        assert!(screen.contains("echo hi"), "tool desc missing:\n{screen}");
        assert!(screen.contains("Allow"), "options missing:\n{screen}");
        assert!(
            !screen.contains("Generate a command or ask a question"),
            "input panel should be hidden while asking:\n{screen}"
        );
    }

    #[test]
    fn permission_cursor_moves_and_wraps() {
        let mut h = harness_awaiting_shell_permission();
        assert_eq!(h.app().permission_select.cursor, 0);
        h.press(KeyCode::Down);
        assert_eq!(h.app().permission_select.cursor, 1);
        h.press(KeyCode::Up);
        h.press(KeyCode::Up);
        // 4 options: wraps to the last (Deny).
        assert_eq!(h.app().permission_select.cursor, 3);
    }

    #[test]
    fn allowing_starts_execution_with_an_interrupt_handle() {
        let mut h = harness_awaiting_shell_permission();
        h.press(KeyCode::Enter); // cursor 0 = Allow

        let app = h.app();
        assert!(app.fsm.ctx.tools.awaiting_permission().is_none());
        assert!(
            app.tool_interrupts.contains_key("tool-1"),
            "shell execution should register an interrupt sender"
        );
    }

    #[test]
    fn denying_resolves_the_tool_without_executing() {
        let mut h = harness_awaiting_shell_permission();
        h.press(KeyCode::Up); // wrap to Deny
        assert_eq!(h.app().permission_select.cursor, 3);
        h.press(KeyCode::Enter);

        let app = h.app();
        assert!(app.fsm.ctx.tools.awaiting_permission().is_none());
        assert!(
            app.tool_interrupts.is_empty(),
            "denied tool must not execute"
        );
        assert!(
            app.fsm
                .ctx
                .events
                .iter()
                .any(|e| matches!(e, ConversationEvent::ToolResult { is_error: true, .. })),
            "denial should record an error tool result"
        );
    }

    #[test]
    fn interrupt_drains_the_interrupt_sender() {
        let mut h = harness_awaiting_shell_permission();
        h.press(KeyCode::Enter); // Allow → executing
        assert!(h.app().tool_interrupts.contains_key("tool-1"));

        // Preview output arrives (as the detached shell stream would send);
        // only then does Ctrl+C mean "interrupt" rather than "quit".
        h.stream(Event::ToolPreviewUpdate {
            tool_id: "tool-1".into(),
            lines: vec!["hi".into()],
            exit_code: None,
        });
        h.press_mod(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(
            h.app().tool_interrupts.is_empty(),
            "Ctrl+C should fire and drop the interrupt sender"
        );
    }

    #[test]
    fn allowed_read_tool_executes_inline() {
        use std::io::Write as _;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "hello from the file").unwrap();

        let mut h = Harness::new(app_with(AgentFsm::new(
            vec!["client_v1_read_file".into()],
            "t".into(),
        )));
        h.type_str("read it");
        h.press(KeyCode::Enter);
        h.stream(Event::StreamStarted);
        h.stream(Event::StreamToolCall {
            id: "tool-r".into(),
            name: "read_file".into(),
            input: serde_json::json!({ "file_path": file.path().to_str().unwrap() }),
        });
        h.stream(Event::StreamDone {
            session_id: "s1".into(),
        });
        h.press(KeyCode::Enter); // Allow

        let app = h.app();
        assert!(
            app.fsm.ctx.events.iter().any(|e| matches!(
                e,
                ConversationEvent::ToolResult { content, is_error: false, .. }
                    if content.contains("hello from the file")
            )),
            "read result should carry the file content"
        );
    }

    #[test]
    fn skill_dispatches_even_when_registered_for_autocomplete() {
        let mut registry = SlashCommandRegistry::default();
        let mut skill_names = HashSet::new();
        registry.register(crate::tui::slash::SlashCommand::new(
            "release",
            "Cut a release",
        ));
        skill_names.insert("release".to_string());
        let app = AiApp::headless(
            AgentFsm::new(vec![], "t".into()),
            None,
            registry,
            skill_names,
        );

        assert!(matches!(
            app.dispatch_submit("/release 1.2".into()),
            Event::RequestSkillLoad { name, arguments }
                if name == "release" && arguments.as_deref() == Some("1.2")
        ));
    }

    #[test]
    fn builtin_takes_precedence_over_skill_with_same_name() {
        let mut registry = SlashCommandRegistry::default();
        let mut skill_names = HashSet::new();
        registry.register(crate::tui::slash::SlashCommand::new(
            "reload",
            "A skill shadowing /reload",
        ));
        skill_names.insert("reload".to_string());
        let app = AiApp::headless(
            AgentFsm::new(vec![], "t".into()),
            None,
            registry,
            skill_names,
        );

        assert!(matches!(
            app.dispatch_submit("/reload".into()),
            Event::SlashCommand { .. }
        ));
    }

    #[test]
    fn resume_notice_renders_above_turns() {
        let mut fsm = AgentFsm::new(vec![], "test-invocation".into());
        fsm.ctx.events.push(ConversationEvent::UserMessage {
            content: "hello".into(),
        });
        let app = AiApp::headless(
            fsm,
            Some("  Continuing previous session".into()),
            SlashCommandRegistry::default(),
            HashSet::new(),
        );

        let h = Harness::new(app);
        let lines = h.term.viewport_lines();
        let notice_row = lines
            .iter()
            .position(|l| l.contains("Continuing previous session"))
            .expect("notice missing");
        let user_row = lines
            .iter()
            .position(|l| l.contains(" You"))
            .expect("user label missing");
        assert!(notice_row < user_row);
    }
}
