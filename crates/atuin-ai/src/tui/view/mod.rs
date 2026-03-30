//! View function that builds the eye-declare element tree from app state.

use eye_declare::{
    Cells, Column, Elements, HStack, Span, Spinner, Text, View, WidthConstraint, element,
};
use ratatui_core::style::{Color, Modifier, Style};

use super::components::atuin_ai::AtuinAi;
use super::components::input_box::InputBox;
use super::components::markdown::Markdown;
use super::state::{AppMode, AppState};

mod turn;

/// Build the element tree from current state.
///
/// Layout (top to bottom):
/// - Conversation messages (user messages, agent responses, tool status)
/// - Streaming content (if actively streaming)
/// - Error display (if in error state)
/// - Spacer
/// - Input box (bordered, with contextual keybindings)
pub fn ai_view(state: &AppState) -> Elements {
    let mut turn_builder = turn::TurnBuilder::new();

    for event in &state.events {
        turn_builder.add_event(event);
    }
    let turns = turn_builder.build();

    let busy = state.mode == AppMode::Streaming || state.mode == AppMode::Generating;
    let last_index = turns.len().saturating_sub(1);

    element! {
        AtuinAi(
            mode: state.mode,
            has_command: state.has_any_command(),
            is_input_blank: state.is_input_blank,
            pending_confirmation: state.confirmation_pending,
        ) {
            #(for (index, turn) in turns.iter().enumerate() {
                #(match turn {
                    turn::UiTurn::User { events } => {
                        user_turn_view(events, index == 0)
                    }
                    turn::UiTurn::Agent { events } => {
                        agent_turn_view(events, busy && index == last_index)
                    }
                    turn::UiTurn::OutOfBand { events } => {
                        out_of_band_turn_view(events)
                    }
                })
            })

            #(if !state.is_exiting() {
                View(key: "input-box", padding_top: Cells::from(1)) {
                    InputBox(
                        key: "input",
                        title: "Generate a command or ask a question",
                        title_right: "Atuin AI",
                        footer: state.footer_text(),
                        active: state.mode == AppMode::Input && !state.confirmation_pending,
                    )

                    #(if state.is_input_blank && state.has_any_command() && state.mode == AppMode::Input {
                        #(if state.confirmation_pending {
                            Text { Span(text: "[Enter] Confirm dangerous command  [Esc] Cancel", style: Style::default().fg(Color::Gray)) }
                        } else {
                            Text { Span(text: "[Enter] Execute suggested command  [Tab] Insert Command", style: Style::default().fg(Color::Gray)) }
                        })
                    })

                }
            })
        }
    }
}

fn user_turn_view(events: &[turn::UiEvent], first_turn: bool) -> Elements {
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let padding = if first_turn { 0 } else { 1 };

    element! {
        View(padding_top: Cells::from(padding)) {
            Text {
                Span(text: "You", style: label_style)
            }
            #(for event in events {
                #(match event {
                    turn::UiEvent::Text { content } => {
                        element! {
                            View(padding_left: Cells::from(2)) {
                                Text {
                                    Span(text: content, style: Style::default())
                                }
                            }
                        }
                    },
                    _ => element!{}
                })
            })
        }
    }
}

fn agent_turn_view(events: &[turn::UiEvent], busy: bool) -> Elements {
    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    element! {
        View {
            Spinner(
                label: "Atuin AI",
                label_style: label_style,
                done_label_style: label_style,
                hide_checkmark: true,
                label_first: true,
                done: !busy,
            )
            #(for event in events {
                #(match event {
                    turn::UiEvent::Text { content } => {
                        element! {
                            View(padding_left: Cells::from(2)) {
                                Markdown(source: content)
                            }
                        }
                    },
                    turn::UiEvent::ToolSummary(summary) => {
                        tool_summary_view(summary)
                    },
                    turn::UiEvent::SuggestedCommand(details) => {
                        suggested_command_view(details)
                    },
                    _ => element!{}
                })
            })
        }
    }
}

fn out_of_band_turn_view(events: &[turn::UiEvent]) -> Elements {
    element! {
        View {
            Text {
                Span(text: "System", style: Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD))
            }
            #(for event in events {
                #(match event {
                    turn::UiEvent::OutOfBandOutput(details) => {
                        out_of_band_output_view(details)
                    }
                    _ => element!{}
                })
            })
        }
    }
}

fn out_of_band_output_view(details: &turn::OutOfBandOutputDetails) -> Elements {
    element! {
        View(padding_left: Cells::from(2)) {
            #(if details.command.is_some() {
                Text {
                    Span(text: details.command.as_ref().unwrap(), style: Style::default().fg(Color::Blue))
                }
            })
            Markdown(source: details.content.clone())
        }
    }
}

fn tool_summary_view(summary: &turn::ToolSummary) -> Elements {
    element! {
        Spinner(label: summary.summary(), done: !summary.any_pending())
    }
}

fn suggested_command_view(details: &turn::SuggestedCommandDetails) -> Elements {
    let is_dangerous = matches!(
        details.danger_level,
        turn::DangerLevel::High(_) | turn::DangerLevel::Medium(_)
    );
    let danger_notes = details.danger_level.notes();
    let danger_style = match details.danger_level {
        turn::DangerLevel::High(_) => Style::default().fg(Color::Red),
        turn::DangerLevel::Medium(_) => Style::default().fg(Color::Yellow),
        turn::DangerLevel::Low(_) => Style::default().fg(Color::Green),
        turn::DangerLevel::Unknown(_) => Style::default().fg(Color::Green),
    };
    let danger_text = match details.danger_level {
        turn::DangerLevel::High(_) => "High",
        turn::DangerLevel::Medium(_) => "Medium",
        turn::DangerLevel::Low(_) => "Low",
        turn::DangerLevel::Unknown(_) => "Unknown",
    };

    let low_confidence = matches!(
        details.confidence_level,
        turn::ConfidenceLevel::Low(_) | turn::ConfidenceLevel::Medium(_)
    );

    let confidence_level = match details.confidence_level {
        turn::ConfidenceLevel::Low(_) => "Low",
        turn::ConfidenceLevel::Medium(_) => "Medium",
        turn::ConfidenceLevel::High(_) => "High",
        turn::ConfidenceLevel::Unknown(_) => "Unknown",
    };

    let confidence_notes = details.confidence_level.notes();

    element! {
        View {
            #(if !details.first_event_in_turn {
                Text { Span(text: "") }
            })
            Text {
                Span(text: "  Suggested command:", style: Style::default().fg(Color::Cyan))
            }
            HStack {
                View(width: WidthConstraint::Fixed(2)) {
                    Text {
                        #(if is_dangerous || low_confidence {
                            Span(text: "! ", style: Style::default().fg(Color::Yellow))
                        } else {
                            Span(text: "$ ", style: Style::default().fg(Color::Blue))
                        })
                    }
                }
                Column {
                    Text {
                        Span(text: &details.command, style: Style::default().fg(Color::Green))
                    }
                }
            }
            #(if is_dangerous {
                View(padding_left: Cells::from(2)) {
                    Text {
                        Span(text: "Danger: ", style: danger_style)
                        Span(text: danger_text, style: danger_style.add_modifier(Modifier::BOLD))
                    }
                }
            })
            #(if is_dangerous && danger_notes.is_some() {
                View(padding_left: Cells::from(2)) {
                    HStack {
                        View(width: WidthConstraint::Fixed(2)) {
                            Text {
                                Span(text: "└")
                            }
                        }
                        View(width: WidthConstraint::Fill) {
                            Markdown(source: danger_notes.unwrap())
                        }
                    }
                }
            })
            #(if low_confidence {
                View(padding_left: Cells::from(2)) {
                    Text {
                        Span(text: "Confidence: ", style: Style::default().fg(Color::Blue))
                        Span(text: confidence_level, style: Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD))
                    }
                }
            })
            #(if low_confidence && confidence_notes.is_some() {
                View(padding_left: Cells::from(2)) {
                    HStack {
                        View(width: WidthConstraint::Fixed(2)) {
                            Text {
                                Span(text: "└")
                            }
                        }
                        View(width: WidthConstraint::Fill) {
                            Markdown(source: confidence_notes.unwrap())
                        }
                    }
                }
            })
        }
    }
}

// ai_view_old removed — superseded by ai_view above
