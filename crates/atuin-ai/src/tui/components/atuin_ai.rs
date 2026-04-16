//! Top-level AtuinAi component that translates key events into AiTuiEvents.
//!
//! Global shortcuts (Ctrl+C, Esc) are handled in the capture phase so they
//! fire regardless of which child is focused. Contextual shortcuts (Enter,
//! Tab) are handled in the bubble phase so child components like the
//! permission Select can consume them first.

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
    pub has_executing_preview: bool,
}

#[derive(Default)]
pub(crate) struct AtuinAiState {
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

    // Capture phase: global shortcuts that must fire regardless of child focus.
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

        // Ctrl+C — interrupt executing command or exit
        if modifiers.contains(KeyModifiers::CONTROL) && *code == KeyCode::Char('c') {
            if props.has_executing_preview {
                let _ = tx.send(AiTuiEvent::InterruptToolExecution);
            } else {
                let _ = tx.send(AiTuiEvent::Exit);
            }
            return EventResult::Consumed;
        }

        // Esc — always handled at the top level
        if *code == KeyCode::Esc {
            match props.mode {
                AppMode::Input => {
                    if props.has_executing_preview {
                        let _ = tx.send(AiTuiEvent::InterruptToolExecution);
                    } else if props.pending_confirmation {
                        let _ = tx.send(AiTuiEvent::CancelConfirmation);
                    } else {
                        let _ = tx.send(AiTuiEvent::Exit);
                    }
                }
                AppMode::Generating | AppMode::Streaming => {
                    let _ = tx.send(AiTuiEvent::CancelGeneration);
                }
                AppMode::Error => {
                    let _ = tx.send(AiTuiEvent::Exit);
                }
            }
            return EventResult::Consumed;
        }

        if *code == KeyCode::Tab
            && matches!(props.mode, AppMode::Input)
            && modifiers.contains(KeyModifiers::NONE)
            && props.has_command
            && props.is_input_blank
        {
            let _ = tx.send(AiTuiEvent::InsertCommand);
            return EventResult::Consumed;
        }

        EventResult::Ignored
    });

    // Bubble phase: contextual shortcuts that children (e.g. Select) may handle first.
    hooks.use_event(move |event, props, state| {
        let Event::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            ..
        }) = event
        else {
            return EventResult::Ignored;
        };

        let Some(ref tx) = state.read().tx else {
            return EventResult::Ignored;
        };

        match props.mode {
            AppMode::Input => match code {
                KeyCode::Enter => {
                    if props.has_command && props.is_input_blank {
                        let _ = tx.send(AiTuiEvent::ExecuteCommand);
                        return EventResult::Consumed;
                    }
                    EventResult::Ignored
                }
                _ => EventResult::Ignored,
            },
            AppMode::Error => match code {
                KeyCode::Enter | KeyCode::Char('r') => {
                    let _ = tx.send(AiTuiEvent::Retry);
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            },
            _ => EventResult::Ignored,
        }
    });

    children
}
