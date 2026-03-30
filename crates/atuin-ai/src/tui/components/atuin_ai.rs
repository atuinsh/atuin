//! Top-level AtuinAi component that translates key events into AiTuiEvents.
//!
//! This component wraps the entire view and handles key events that bubble up
//! from child components (or aren't consumed by them). It maps raw key events
//! to semantic `AiTuiEvent` variants based on the current `AppMode`.

use std::sync::mpsc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use eye_declare::{Elements, EventResult, Hooks, component, props};

use crate::tui::events::AiTuiEvent;
use crate::tui::state::AppMode;

/// Top-level wrapper component for the AI TUI.
///
/// Props carry the current mode so `handle_event` can translate keys
/// into the right `AiTuiEvent`. Children are rendered via slot children.
#[props]
pub(crate) struct AtuinAi {
    pub mode: AppMode,
    pub has_command: bool,
    pub is_input_blank: bool,
    pub pending_confirmation: bool,
}

#[derive(Default)]
pub struct AtuinAiState {
    tx: Option<mpsc::Sender<AiTuiEvent>>,
}

#[component(props = AtuinAi, state = AtuinAiState, children = Elements)]
fn atuin_ai(
    _props: &AtuinAi,
    _state: &AtuinAiState,
    hooks: &mut Hooks<AtuinAi, AtuinAiState>,
    children: Elements,
) -> Elements {
    hooks.use_context::<mpsc::Sender<AiTuiEvent>>(|tx, _, state| {
        state.tx = tx.cloned();
    });

    hooks.use_event_capture(move |event, props, state| {
        let Event::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            modifiers,
            ..
        }) = event
        else {
            return EventResult::Ignored;
        };

        let Some(ref tx) = state.read().tx else {
            return EventResult::Ignored;
        };

        // Ctrl+C always exits
        if modifiers.contains(KeyModifiers::CONTROL) && *code == KeyCode::Char('c') {
            let _ = tx.send(AiTuiEvent::Exit);
            return EventResult::Consumed;
        }

        match props.mode {
            AppMode::Input => match code {
                KeyCode::Esc => {
                    if props.pending_confirmation {
                        let _ = tx.send(AiTuiEvent::CancelConfirmation);
                        return EventResult::Consumed;
                    }

                    let _ = tx.send(AiTuiEvent::Exit);
                    EventResult::Consumed
                }
                KeyCode::Tab => {
                    if props.has_command && props.is_input_blank {
                        let _ = tx.send(AiTuiEvent::InsertCommand);
                        return EventResult::Consumed;
                    }

                    EventResult::Ignored
                }
                KeyCode::Enter => {
                    if props.has_command && props.is_input_blank {
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
    });

    children
}
