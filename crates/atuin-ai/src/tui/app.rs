//! The eye-declare v2 app: the FSM is the model, `update` is the driver
//! loop, and the live tail is a view over the not-yet-committed turns.

use std::cell::RefCell;
use std::collections::HashSet;

use crossterm::event::KeyCode;
use eye_declare::{
    App, Ctx, Element, ElementExt, Fluent, Focus, FocusHandle, InputEvent, Keymap, Task, col, key,
    keymap, text,
};
use ratatui_core::style::{Color, Modifier, Style};
use tokio::sync::mpsc::UnboundedSender;
use tui_textarea::TextArea;

use crate::fsm::effects::{Effect, ExitAction, TimeoutKind};
use crate::fsm::events::Event;
use crate::fsm::{AgentFsm, AgentState};
use crate::tui::persist::PersistJob;
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
// TODO(v2 port, tools slice): trackers + snapshot store reconnect there.
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
                // TODO(v2 port): tools + permissions + model fetch reconnect
                // in the next slices.
                other => tracing::debug!(effect = ?other, "effect deferred (v2 port)"),
            }
        }
        self.push_ready_turns(ctx);
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
                    false,
                )
            }))
            .when(needs_pending_banner, |c| {
                c.child(view::agent_turn_view(&[], true, false))
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
            .child(view::input::input_area(
                &self.input,
                self.input_active(),
                self.footer_text(),
                self.is_input_blank() && self.has_command() && self.input_active(),
                &self.slash_results,
            ))
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
