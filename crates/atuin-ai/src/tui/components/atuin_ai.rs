//! Top-level AtuinAi component that translates key events into AiTuiEvents.
//!
//! This component wraps the entire view and handles key events that bubble up
//! from child components (or aren't consumed by them). It maps raw key events
//! to semantic `AiTuiEvent` variants based on the current `AppMode`.

use std::sync::mpsc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use eye_declare::{Component, EventResult, Hooks, Tracked, impl_slot_children};

use crate::tui::events::AiTuiEvent;
use crate::tui::state::AppMode;

/// Top-level wrapper component for the AI TUI.
///
/// Props carry the current mode so `handle_event` can translate keys
/// into the right `AiTuiEvent`. Children are rendered via slot children.
pub struct AtuinAi {
    pub mode: AppMode,
    pub has_command: bool,
    pub is_input_blank: bool,
    pub pending_confirmation: bool,
}

impl Default for AtuinAi {
    fn default() -> Self {
        Self {
            mode: AppMode::Input,
            has_command: false,
            is_input_blank: false,
            pending_confirmation: false,
        }
    }
}

impl_slot_children!(AtuinAi);

#[derive(Default)]
pub struct AtuinAiState {
    tx: Option<mpsc::Sender<AiTuiEvent>>,
}

impl Component for AtuinAi {
    type State = AtuinAiState;

    fn initial_state(&self) -> Option<Self::State> {
        Some(AtuinAiState::default())
    }

    fn lifecycle(&self, hooks: &mut Hooks<Self::State>, _state: &Self::State) {
        hooks.use_context::<mpsc::Sender<AiTuiEvent>>(|tx, state| {
            state.tx = tx.cloned();
        });
    }

    fn render(
        &self,
        _area: ratatui::layout::Rect,
        _buf: &mut ratatui::buffer::Buffer,
        _state: &Self::State,
    ) {
        // Rendering is handled by slot children
    }

    fn desired_height(&self, _width: u16, _state: &Self::State) -> u16 {
        0
    }

    fn handle_event_capture(&self, event: &Event, state: &mut Tracked<Self::State>) -> EventResult {
        let state = state.read();

        let Event::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            modifiers,
            ..
        }) = event
        else {
            return EventResult::Ignored;
        };

        let Some(ref tx) = state.tx else {
            return EventResult::Ignored;
        };

        // Ctrl+C always exits
        if modifiers.contains(KeyModifiers::CONTROL) && *code == KeyCode::Char('c') {
            let _ = tx.send(AiTuiEvent::Exit);
            return EventResult::Consumed;
        }

        match self.mode {
            AppMode::Input => match code {
                KeyCode::Esc => {
                    if self.pending_confirmation {
                        let _ = tx.send(AiTuiEvent::CancelConfirmation);
                        return EventResult::Consumed;
                    }

                    let _ = tx.send(AiTuiEvent::Exit);
                    EventResult::Consumed
                }
                KeyCode::Tab => {
                    if self.has_command && self.is_input_blank {
                        let _ = tx.send(AiTuiEvent::InsertCommand);
                        return EventResult::Consumed;
                    }

                    EventResult::Ignored
                }
                KeyCode::Enter => {
                    if self.has_command && self.is_input_blank {
                        let _ = tx.send(AiTuiEvent::ExecuteCommand);
                        return EventResult::Consumed;
                    }

                    EventResult::Ignored
                }
                _ => EventResult::Ignored,
            },
            AppMode::Generating | AppMode::Streaming => match code {
                KeyCode::Esc => {
                    let _ = tx.send(AiTuiEvent::CancelGeneration);
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            },
            AppMode::Error => match code {
                KeyCode::Esc => {
                    let _ = tx.send(AiTuiEvent::Exit);
                    EventResult::Consumed
                }
                KeyCode::Enter | KeyCode::Char('r') => {
                    let _ = tx.send(AiTuiEvent::Retry);
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            },
        }
    }
}
