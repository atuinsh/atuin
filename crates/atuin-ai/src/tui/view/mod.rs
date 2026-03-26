//! View function that builds the eye-declare element tree from app state.

use eye_declare::{
    Column, Component, Elements, HStack, Line, Span, Spinner, TextBlock, VStack, WidthConstraint,
    element, impl_slot_children,
};
use ratatui_core::style::{Color, Modifier, Style};

use super::components::atuin_ai::AtuinAi;
use super::components::input_box::InputBox;
use super::components::markdown::Markdown;
use super::state::{AppMode, AppState};

mod turn;

#[derive(Default)]
struct Padding {
    top: u16,
    left: u16,
    right: u16,
    bottom: u16,
}

impl Component for Padding {
    type State = ();

    fn content_inset(&self, _state: &Self::State) -> eye_declare::Insets {
        eye_declare::Insets::ZERO
            .left(self.left)
            .right(self.right)
            .top(self.top)
            .bottom(self.bottom)
    }

    fn desired_height(&self, _width: u16, _state: &Self::State) -> u16 {
        0
    }

    fn render(
        &self,
        _area: ratatui::layout::Rect,
        _buf: &mut ratatui::buffer::Buffer,
        _state: &(),
    ) {
    }
}

impl_slot_children!(Padding);

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
            mode: state.mode.clone(),
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
                TextBlock { Line { Span(text: "") } }
                InputBox(
                    key: "input",
                    title: "Generate a command or ask a question",
                    title_right: "Atuin AI",
                    footer: state.footer_text(),
                    active: state.mode == AppMode::Input && !state.confirmation_pending,
                )

                #(if state.is_input_blank && state.has_any_command() && state.mode == AppMode::Input {
                    #(if state.confirmation_pending {
                        TextBlock { Line { Span(text: "[Enter] Confirm dangerous command  [Esc] Cancel", style: Style::default().fg(Color::Gray)) } }
                    } else {
                        TextBlock { Line { Span(text: "[Enter] Execute suggested command  [Tab] Insert Command", style: Style::default().fg(Color::Gray)) } }
                    })
                })
            })
        }
    }
}

fn user_turn_view(events: &[turn::UiEvent], first_turn: bool) -> Elements {
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    element! {
        VStack {
            TextBlock {
                #(if !first_turn {
                    Line { Span() }
                })
                Line {
                    Span(text: "You", style: label_style)
                }
            }
            #(for event in events {
                #(match event {
                    turn::UiEvent::Text { content } => {
                        element! {
                            Padding(left: 2u16) {
                                TextBlock {
                                    Line {
                                        Span(text: content, style: Style::default())
                                    }
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
        VStack {
            Spinner(
                label: "Atuin AI",
                done: false,
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
                            Padding(left: 2u16) {
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
        VStack {
            TextBlock {
                Line { Span(text: "System", style: Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)) }
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
        Padding(left: 2u16) {
            #(if details.command.is_some() {
                TextBlock {
                    Line {
                        Span(text: details.command.as_ref().unwrap(), style: Style::default().fg(Color::Blue))
                    }
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
    // LeftPadded {
    //     TextBlock {
    //         Line {
    //             Span(text: icon, style: icon_style)
    //             Span(text: summary.summary(), style: style)
    //         }
    //     }
    // }
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
    };
    let danger_text = match details.danger_level {
        turn::DangerLevel::High(_) => "High",
        turn::DangerLevel::Medium(_) => "Medium",
        turn::DangerLevel::Low(_) => "Low",
    };

    let low_confidence = matches!(
        details.confidence_level,
        turn::ConfidenceLevel::Low(_) | turn::ConfidenceLevel::Medium(_)
    );

    let confidence_level = match details.confidence_level {
        turn::ConfidenceLevel::Low(_) => "Low",
        turn::ConfidenceLevel::Medium(_) => "Medium",
        turn::ConfidenceLevel::High(_) => "High",
    };

    let confidence_notes = details.confidence_level.notes();

    element! {
        VStack {
            TextBlock {
                #(if !details.first_event_in_turn {
                    Line { Span() }
                })
                Line {
                    Span(text: "  Suggested command:", style: Style::default().fg(Color::Cyan))
                }
            }
            HStack {
                Column(width: WidthConstraint::Fixed(2)) {
                    TextBlock {
                        Line {
                            #(if is_dangerous || low_confidence {
                                Span(text: "! ", style: Style::default().fg(Color::Yellow))
                            } else {
                                Span(text: "$ ", style: Style::default().fg(Color::Blue))
                            })
                        }
                    }
                }
                Column {
                    TextBlock {
                        Line {
                            Span(text: &details.command, style: Style::default().fg(Color::Green))
                        }
                    }
                }
            }
            #(if is_dangerous {
                Padding(left: 2u16) {
                    TextBlock {
                        Line {
                            Span(text: "Danger: ", style: danger_style)
                            Span(text: danger_text, style: danger_style.add_modifier(Modifier::BOLD))
                        }
                    }
                }
            })
            #(if is_dangerous && danger_notes.is_some() {
                Padding(left: 2u16) {
                    HStack {
                        Column(width: WidthConstraint::Fixed(2)) {
                            TextBlock {
                                Line {
                                    Span(text: "└")
                                }
                            }
                        }
                        Column(width: WidthConstraint::Fill) {
                            Markdown(source: danger_notes.unwrap())
                        }
                    }
                }
            })
            #(if low_confidence {
                Padding(left: 2u16) {
                    TextBlock {
                        Line {
                            Span(text: "Confidence: ", style: Style::default().fg(Color::Blue))
                            Span(text: confidence_level, style: Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD))
                        }
                    }
                }
            })
            #(if low_confidence && confidence_notes.is_some() {
                Padding(left: 2u16) {
                    HStack {
                        Column(width: WidthConstraint::Fixed(2)) {
                            TextBlock {
                                Line {
                                    Span(text: "└")
                                }
                            }
                        }
                        Column(width: WidthConstraint::Fill) {
                            Markdown(source: confidence_notes.unwrap())
                        }
                    }
                }
            })
        }
    }
}

// ai_view_old removed — superseded by ai_view above
