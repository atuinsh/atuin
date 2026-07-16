//! The eye-declare v2 app: the FSM is the model, `update` is the driver
//! loop, and the live tail is a view over the not-yet-committed turns.

use crossterm::event::KeyCode;
use eye_declare::{App, Ctx, Element, Fluent, Keymap, col, key, keymap, text};
use ratatui_core::style::{Color, Modifier, Style};

use crate::fsm::{AgentFsm, AgentState};
use crate::tui::view;
use crate::tui::view::turn::{TurnBuilder, UiTurn};

/// What the TUI resolves to, for the shell hook.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ExitOutcome {
    Execute(String),
    Insert(String),
    #[default]
    Cancel,
}

/// IO resources the app's effects need (databases, session persistence,
/// caches). Not part of the FSM's state.
// TODO(v2 port, streaming slice): effect execution reconnects these.
#[allow(dead_code)]
pub(crate) struct IoContext {
    pub app_ctx: crate::context::AppContext,
    pub client_ctx: crate::context::ClientContext,
    pub session_mgr: crate::session::SessionManager,
    pub file_tracker: crate::file_tracker::FileReadTracker,
    pub edit_permissions: crate::edit_permissions::EditPermissionCache,
    pub snapshot_store: Option<crate::snapshots::SnapshotStore>,
    pub skill_registry: crate::skills::SkillRegistry,
    pub user_context_cache: crate::user_context::UserContextCache,
}

#[derive(Debug, Clone)]
pub(crate) enum Msg {
    /// Leave the TUI without a command (Esc / Ctrl+C).
    Quit,
}

pub(crate) struct AiApp {
    pub fsm: AgentFsm,
    /// "Continuing previous session…" banner, frozen at startup.
    resume_notice: Option<String>,
}

impl AiApp {
    pub fn new(fsm: AgentFsm, resume_notice: Option<String>) -> Self {
        Self { fsm, resume_notice }
    }

    fn is_busy(&self) -> bool {
        matches!(self.fsm.state, AgentState::Turn { .. })
    }

    /// Turns not yet committed to scrollback, built fresh from the FSM's
    /// events. O(visible conversation) per frame for now; the committed
    /// frontier arrives with `ctx.push` wiring in the streaming slice.
    fn live_turns(&self) -> Vec<UiTurn> {
        let events = &self.fsm.ctx.events;
        let safe_start = self.fsm.ctx.view_start_index.min(events.len());
        let mut builder = TurnBuilder::new(&self.fsm.ctx.tools);
        for event in &events[safe_start..] {
            builder.add_event(event);
        }
        builder.build()
    }
}

impl App for AiApp {
    type Msg = Msg;
    type Output = ExitOutcome;

    fn update(&mut self, msg: Msg, ctx: &mut Ctx<'_, Self>) {
        match msg {
            Msg::Quit => ctx.exit(ExitOutcome::Cancel),
        }
    }

    fn tail(&self) -> impl Element + '_ {
        let turns = self.live_turns();
        let busy = self.is_busy();
        let last = turns.len().saturating_sub(1);

        col()
            .when_some(self.resume_notice.clone(), |c, notice| {
                c.child(
                    text(notice).style(
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                )
            })
            .children(
                turns
                    .iter()
                    .enumerate()
                    .map(|(i, turn)| view::turn_view(turn, i == 0, busy && i == last, false)),
            )
    }

    fn keymap(&self) -> Keymap<Msg> {
        keymap()
            .on_override(key(KeyCode::Char('c')).ctrl(), Msg::Quit)
            .on(key(KeyCode::Esc), Msg::Quit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::ConversationEvent;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use eye_declare::{InputEvent, Runtime};
    use eye_declare_engine::test_terminal::TestTerminal;

    fn fixture_app() -> AiApp {
        let mut fsm = AgentFsm::new(vec![], "test-invocation".into());
        fsm.ctx.events.push(ConversationEvent::UserMessage {
            content: "list my files".into(),
        });
        fsm.ctx.events.push(ConversationEvent::Text {
            content: "Use `ls` to list files.".into(),
        });
        AiApp::new(fsm, None)
    }

    #[test]
    fn tail_renders_conversation_turns() {
        let mut rt = Runtime::new(fixture_app(), 60, 24);
        let mut term = TestTerminal::new(60, 24);
        term.feed(&rt.present());

        let screen = term.viewport_lines().join("\n");
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
        let mut rt = Runtime::new(fixture_app(), 60, 24);
        let (_, exit) = rt.handle(InputEvent::Key(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert_eq!(exit, Some(ExitOutcome::Cancel));
    }

    #[test]
    fn resume_notice_renders_above_turns() {
        let mut fsm = AgentFsm::new(vec![], "test-invocation".into());
        fsm.ctx.events.push(ConversationEvent::UserMessage {
            content: "hello".into(),
        });
        let app = AiApp::new(fsm, Some("  Continuing previous session".into()));

        let mut rt = Runtime::new(app, 60, 24);
        let mut term = TestTerminal::new(60, 24);
        term.feed(&rt.present());

        let lines = term.viewport_lines();
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
