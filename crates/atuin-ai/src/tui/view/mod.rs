//! View function that builds the eye-declare element tree from app state.

use eye_declare::{
    BorderType, Cells, Column, Elements, HStack, Span, Spinner, Text, View, Viewport,
    WidthConstraint, element,
};
use ratatui_core::style::{Color, Modifier, Style};

use crate::tools::{ClientToolCall, TrackedTool};
use crate::tui::components::select::SelectOption;
use crate::tui::events::{AiTuiEvent, PermissionResult};

use super::components::atuin_ai::AtuinAi;
use super::components::input_box::InputBox;
use super::components::markdown::Markdown;
use super::components::select::Select;
use super::state::{AppMode, Session};

mod turn;

/// Build the element tree from current state.
///
/// Layout (top to bottom):
/// - Conversation messages (user messages, agent responses, tool status)
/// - Streaming content (if actively streaming)
/// - Error display (if in error state)
/// - Spacer
/// - Input box (bordered, with contextual keybindings)
pub(crate) fn ai_view(state: &Session) -> Elements {
    let mut turn_builder = turn::TurnBuilder::new(&state.tool_tracker);

    for event in &state.conversation.events {
        turn_builder.add_event(event);
    }
    let turns = turn_builder.build();

    let busy = state.interaction.mode == AppMode::Streaming
        || state.interaction.mode == AppMode::Generating;
    let last_index = turns.len().saturating_sub(1);

    element! {
        AtuinAi(
            mode: state.interaction.mode,
            has_command: state.conversation.has_any_command(),
            is_input_blank: state.interaction.is_input_blank,
            pending_confirmation: state.interaction.confirmation_pending,
            has_executing_preview: state.tool_tracker.has_executing_preview(),
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
                #(input_view(state))
            })
        }
    }
}

fn input_view(state: &Session) -> Elements {
    let asking_tool = state.tool_tracker.asking_for_permission();
    let in_git_project = state.in_git_project;

    element! {
        #(if let Some(tc) = asking_tool {
            #(tool_call_view(tc, in_git_project))
        })

        #(if asking_tool.is_none() {
            View(key: "input-box", padding_top: Cells::from(1)) {
                InputBox(
                    key: "input",
                    title: "Generate a command or ask a question",
                    title_right: "Atuin AI",
                    footer: state.footer_text(),
                    active: state.interaction.mode == AppMode::Input && !state.interaction.confirmation_pending,
                )

                #(if state.interaction.is_input_blank && state.conversation.has_any_command() && state.interaction.mode == AppMode::Input {
                    #(if state.interaction.confirmation_pending {
                        Text { Span(text: "[Enter] Confirm dangerous command  [Esc] Cancel", style: Style::default().fg(Color::Gray)) }
                    } else {
                        Text { Span(text: "[Enter] Execute suggested command  [Tab] Insert Command", style: Style::default().fg(Color::Gray)) }
                    })
                })
            }
        })
    }
}

fn tool_call_view(tool_call: &TrackedTool, in_git_project: bool) -> Elements {
    let verb = tool_call.tool.descriptor().display_verb;
    let tool_desc = match &tool_call.tool {
        ClientToolCall::Read(tool) => tool.path.display().to_string(),
        ClientToolCall::Write(tool) => tool.path.display().to_string(),
        ClientToolCall::Shell(tool) => tool.command.clone(),
        ClientToolCall::AtuinHistory(tool) => tool.query.clone(),
    };

    let dir_label = if in_git_project {
        "Always allow in this workspace"
    } else {
        "Always allow in this directory"
    };

    element! {
        View(key: format!("tool-call-{}", tool_call.id), padding_left: Cells::from(2), padding_top: Cells::from(1)) {
            Text {
                Span(text: format!("Atuin AI would like to {}: ", verb), style: Style::default())
                Span(text: &tool_desc, style: Style::default().fg(Color::Yellow))
            }
            View(padding_left: Cells::from(2)) {
                Select(options: [
                    SelectOption::builder()
                        .label("Allow")
                        .value("allow")
                        .build(),
                    SelectOption::builder()
                        .label(dir_label)
                        .value("always-allow-in-dir")
                        .build(),
                    SelectOption::builder()
                        .label("Always allow")
                        .value("always-allow")
                        .build(),
                    SelectOption::builder()
                        .label("Deny")
                        .value("deny")
                        .build(),
                ], on_select: Box::new(move |option: &SelectOption| {
                    let value = match option.value.as_str() {
                        "allow" => PermissionResult::Allow,
                        "always-allow-in-dir" => PermissionResult::AlwaysAllowInDir,
                        "always-allow" => PermissionResult::AlwaysAllow,
                        "deny" => PermissionResult::Deny,
                        _ => unreachable!(),
                    };

                    Some(AiTuiEvent::SelectPermission(value))
                }) as Box<dyn Fn(&SelectOption) -> Option<AiTuiEvent> + Send + Sync>)
            }
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
                Span(text: " You ", style: label_style.reversed())
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
                label: " Atuin AI ",
                label_style: label_style.reversed(),
                done_label_style: label_style.reversed(),
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
                    turn::UiEvent::ToolCall(details) => {
                        let preview_done = details.preview.as_ref().is_some_and(|p| p.exit_code.is_some() || p.interrupted);
                        let tool_key = details.tool_use_id.clone();

                        element! {
                            View(key: format!("tool-output-{tool_key}"), padding_left: Cells::from(2)) {
                                #(if let Some(ref preview) = details.preview {
                                    View(key: format!("preview-{tool_key}")) {
                                        #(preview_spinner_view(&details.name, preview_done))
                                        Viewport(
                                            key: format!("viewport-{tool_key}"),
                                            lines: preview.lines.clone(),
                                            height: 10,
                                            border: BorderType::Plain,
                                            border_style: Style::default().fg(Color::DarkGray),
                                            style: Style::default().fg(Color::White),
                                            wrap: false,
                                        )
                                        #(if let Some(code) = preview.exit_code {
                                            #(if code == 0 {
                                                Text {
                                                    Span(text: format!("Exit code: {code}"), style: Style::default().fg(Color::Green))
                                                }
                                            } else {
                                                Text {
                                                    Span(text: format!("Exit code: {code}"), style: Style::default().fg(Color::Red))
                                                }
                                            })
                                        })
                                        #(if preview.interrupted {
                                            Text {
                                                Span(text: "Interrupted", style: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                                            }
                                        })
                                        #(if !preview_done {
                                            Text {
                                                Span(text: "[Ctrl+C] Interrupt", style: Style::default().fg(Color::DarkGray))
                                            }
                                        })
                                    }
                                } else {
                                    #(tool_status_view(&details.name, &details.status))
                                })
                            }
                        }
                    }
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

/// Render a status indicator for a non-preview tool call (e.g. atuin_history, read_file).
fn tool_status_view(name: &str, status: &turn::ToolResultStatus) -> Elements {
    match status {
        turn::ToolResultStatus::Pending => {
            element! {
                Spinner(
                    label: format!("Running: {name}"),
                    label_style: Style::default().fg(Color::Yellow),
                    done: false,
                )
            }
        }
        turn::ToolResultStatus::Success => {
            element! {
                Spinner(
                    label: format!("Ran: {name}"),
                    done: true,
                )
            }
        }
        turn::ToolResultStatus::Error => {
            element! {
                Text {
                    Span(text: "✗ ", style: Style::default().fg(Color::Red))
                    Span(text: format!("{name}: denied"), style: Style::default().fg(Color::Red))
                }
            }
        }
    }
}

/// Render a spinner/status line for a command preview (shell tools).
fn preview_spinner_view(name: &str, done: bool) -> Elements {
    element! {
        Spinner(
            label: if done { format!("Ran: {name}") } else { format!("Running: {name}") },
            label_style: Style::default().fg(Color::Yellow),
            done: done,
        )
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
