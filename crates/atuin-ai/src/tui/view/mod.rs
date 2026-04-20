//! View function that builds the eye-declare element tree from app state.

use eye_declare::{
    BorderType, Cells, Column, Direction, Elements, HStack, Span, Spinner, Text, View, Viewport,
    WidthConstraint, element,
};
use ratatui_core::style::{Color, Modifier, Style};

use crate::tools::{ClientToolCall, HistorySearchFilterMode, ToolPreview, TrackedTool};
use crate::tui::components::select::SelectOption;
use crate::tui::components::session_continue::SessionContinue;
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

    for event in &state.archived_view_events {
        turn_builder.add_event(event);
    }
    for event in &state.conversation.events[state.view_start_index..] {
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
            #(if state.is_resumed && (!state.is_exiting() || !turns.is_empty()) {
                SessionContinue(key: "continuation-notice", continued_at: state.last_event_time)
            })

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

            #({
                let needs_pending_banner = busy && !matches!(turns.last(), Some(turn::UiTurn::Agent { .. }));
                if needs_pending_banner {
                    let empty: &[turn::UiEvent] = &[];
                    agent_turn_view(empty, true)
                } else {
                    element! {}
                }
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
    let slash_results = state
        .interaction
        .slash_command_search_results
        .iter()
        .take(4)
        .collect::<Vec<_>>();
    let first_slash_result = slash_results.first().cloned();

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
                    slash_suggestion: first_slash_result.cloned()
                )

                #(if state.interaction.is_input_blank && state.conversation.has_any_command() && state.interaction.mode == AppMode::Input {
                    #(if state.interaction.confirmation_pending {
                        Text { Span(text: "[Enter] Confirm dangerous command  [Esc] Cancel", style: Style::default().fg(Color::Gray)) }
                    } else {
                        Text { Span(text: "[Enter] Execute suggested command  [Tab] Insert Command", style: Style::default().fg(Color::Gray)) }
                    })
                })

                #(if !slash_results.is_empty() {
                    #(for (i, result) in slash_results.iter().enumerate() {
                        Text {
                            Span(text: format!("/{}", &result.command.name[..result.span.0]), style: Style::default().fg(Color::Blue))
                            Span(text: &result.command.name[result.span.0..result.span.1], style: Style::default().fg(Color::Blue).add_modifier(Modifier::UNDERLINED))
                            Span(text: format!("{}", &result.command.name[result.span.1..]), style: Style::default().fg(Color::Blue))
                            Span(text: " - ")
                            Span(text: &result.command.description)

                            #(if i == 0 {
                                Span(text: " [Tab] Insert", style: Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC).dim())
                            })
                        }

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
        ClientToolCall::Edit(tool) => tool.path.display().to_string(),
        ClientToolCall::Write(tool) => tool.path.display().to_string(),
        ClientToolCall::Shell(tool) => tool.command.clone(),
        ClientToolCall::AtuinHistory(tool) => tool.query.clone(),
    };

    let select_options = permission_options_for_tool(&tool_call.tool, in_git_project);

    element! {
        View(key: format!("tool-call-{}", tool_call.id), padding_left: Cells::from(2), padding_top: Cells::from(1)) {
            Text {
                Span(text: format!("Atuin AI would like to {}: ", verb), style: Style::default())
                Span(text: &tool_desc, style: Style::default().fg(Color::Yellow))
            }
            View(padding_left: Cells::from(2)) {
                Select(options: select_options, on_select: Box::new(move |option: &SelectOption| {
                    PermissionResult::from_value_str(option.value.as_str())
                        .map(AiTuiEvent::SelectPermission)
                }) as Box<dyn Fn(&SelectOption) -> Option<AiTuiEvent> + Send + Sync>)
            }
        }
    }
}

/// Build the permission SelectOptions appropriate for a tool call.
///
/// Edit tools get a per-file session-scoped option instead of the
/// workspace-level "Always allow in this directory". Other tools
/// keep the standard set.
fn permission_options_for_tool(tool: &ClientToolCall, in_git_project: bool) -> Vec<SelectOption> {
    match tool {
        ClientToolCall::Edit(_) => vec![
            SelectOption::builder()
                .label("Allow")
                .value(PermissionResult::Allow.as_value_str())
                .build(),
            SelectOption::builder()
                .label("Allow this file for this session")
                .value(PermissionResult::AllowFileForSession.as_value_str())
                .build(),
            SelectOption::builder()
                .label("Always allow")
                .value(PermissionResult::AlwaysAllow.as_value_str())
                .build(),
            SelectOption::builder()
                .label("Deny")
                .value(PermissionResult::Deny.as_value_str())
                .build(),
        ],
        _ => {
            let dir_label = if in_git_project {
                "Always allow in this workspace"
            } else {
                "Always allow in this directory"
            };
            vec![
                SelectOption::builder()
                    .label("Allow")
                    .value(PermissionResult::Allow.as_value_str())
                    .build(),
                SelectOption::builder()
                    .label(dir_label)
                    .value(PermissionResult::AlwaysAllowInDir.as_value_str())
                    .build(),
                SelectOption::builder()
                    .label("Always allow")
                    .value(PermissionResult::AlwaysAllow.as_value_str())
                    .build(),
                SelectOption::builder()
                    .label("Deny")
                    .value(PermissionResult::Deny.as_value_str())
                    .build(),
            ]
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
            #(for (i, event) in events.iter().enumerate() {
                #(if i > 0 {
                    Text { Span(text: "") }
                })
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
                        let tool_key = details.tool_use_id.clone();

                        element! {
                            View(key: format!("tool-output-{tool_key}"), padding_left: Cells::from(2)) {
                                #(match &details.render_data {
                                    turn::ToolRenderData::Shell { command, preview } => {
                                        shell_tool_view(&tool_key, command, preview.as_ref())
                                    },
                                    turn::ToolRenderData::FileEdit { path, preview } => {
                                        file_edit_tool_view(&tool_key, &details.status, path, preview.as_ref())
                                    },
                                    turn::ToolRenderData::FileWrite { path } => {
                                        file_write_tool_view(&details.status, path)
                                    },
                                    turn::ToolRenderData::Remote => {
                                        tool_status_view(&details.name, &details.status)
                                    },
                                    turn::ToolRenderData::FileRead { .. }
                                    | turn::ToolRenderData::HistorySearch { .. } => {
                                        element!{}
                                    },
                                })
                            }
                        }
                    }
                    turn::UiEvent::ToolGroup(group) => {
                        let group_key = group.calls
                            .first()
                            .map(|c| c.tool_use_id.as_str())
                            .unwrap_or("empty");

                        element! {
                            View(key: format!("group-{group_key}"), padding_left: Cells::from(2)) {
                                #(match group.kind {
                                    turn::ToolGroupKind::FileRead => file_read_group_view(group),
                                    turn::ToolGroupKind::HistorySearch => history_search_group_view(group),
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
                Span(text: " System ", style: Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD).add_modifier(Modifier::REVERSED))
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

// ───────────────────────────────────────────────────────────────────
// Per-tool view functions
// ───────────────────────────────────────────────────────────────────

/// Max output lines shown for a shell command preview.
const MAX_SHELL_PREVIEW_LINES: u16 = 5;

/// Render a shell command execution with live VT100 output viewport.
fn shell_tool_view(tool_key: &str, command: &str, preview: Option<&ToolPreview>) -> Elements {
    let preview_done = preview.is_some_and(|p| p.exit_code.is_some() || p.interrupted);

    element! {
        #(if let Some(preview) = preview {
            View(key: format!("preview-{tool_key}")) {
                Spinner(
                    label: if preview_done { format!("Ran: {command}") } else { format!("Running: {command}") },
                    done: preview_done,
                    hide_checkmark: true,
                )
                HStack {
                    View(width: WidthConstraint::Fixed(2)) {
                        Text { Span(text: "└ ") }
                    }
                    Column {
                        Viewport(
                            key: format!("viewport-{tool_key}"),
                            lines: preview.lines.clone(),
                            height: (preview.lines.len() as u16).clamp(1, MAX_SHELL_PREVIEW_LINES),
                            style: Style::default().fg(Color::Gray),
                            wrap: false,
                        )
                    }
                }
                #(shell_tool_footer(preview, preview_done))
            }
        } else {
            Spinner(
                label: format!("Running: {command}"),
                label_style: Style::default().fg(Color::Yellow),
                done: false,
            )
        })
    }
}

fn shell_tool_footer(preview: &ToolPreview, preview_done: bool) -> Elements {
    if preview.interrupted {
        return element! {
            Text {
                Span(text: "Interrupted", style: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
            }
        };
    }
    if !preview_done {
        return element! {
            Text {
                Span(text: "[Ctrl+C] Interrupt", style: Style::default().fg(Color::DarkGray))
            }
        };
    }
    if let Some(code) = preview.exit_code {
        let style = if code == 0 {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        };
        return element! {
            Text { Span(text: format!("Exit code: {code}"), style: style) }
        };
    }
    element! {}
}

/// Render a file edit tool call with diff preview.
fn file_edit_tool_view(
    key: &str,
    status: &turn::ToolResultStatus,
    path: &std::path::Path,
    preview: Option<&crate::diff::EditPreview>,
) -> Elements {
    use crate::diff::DiffLine;

    let display_path = format_path_for_display(path);

    let status_line = match status {
        turn::ToolResultStatus::Pending => {
            element! {
                Spinner(
                    label: format!("Editing: {display_path}"),
                    label_style: Style::default().fg(Color::Yellow),
                    done: false,
                )
            }
        }
        turn::ToolResultStatus::Success => {
            element! {
                Spinner(label: format!("Edited: {display_path}"), done: true)
            }
        }
        turn::ToolResultStatus::Error => {
            element! {
                Text {
                    Span(text: "✗ ", style: Style::default().fg(Color::Red))
                    Span(text: format!("Edit {display_path}: failed"), style: Style::default().fg(Color::Red))
                }
            }
        }
    };

    // If no preview, just show the status line
    let Some(preview) = preview else {
        return status_line;
    };
    if preview.hunks.is_empty() {
        return status_line;
    }

    // Calculate the line number gutter width from the highest line number
    let max_line_num = preview.max_line_number();
    let gutter_width = max_line_num.to_string().len().max(2) as u16 + 1; // +1 for spacing

    element! {
        View(key: key.to_string()) {
            #(status_line)

            View(key: format!("{key}-diff"), padding_left: Cells::from(2)) {
                #(for (hunk_idx, hunk) in preview.hunks.iter().enumerate() {
                    #({
                        let gutter_w = gutter_width;
                        let mut before_pos = hunk.before_start;
                        let mut after_pos = hunk.after_start;
                        let lines_rendered: Vec<_> = hunk.lines.iter().enumerate().map(|(line_idx, line)| {
                            let (prefix, text, style, gutter_text, gutter_style) = match line {
                                DiffLine::Context(t) => {
                                    let num = format!("{:>width$}", after_pos, width = (gutter_w - 1) as usize);
                                    before_pos += 1;
                                    after_pos += 1;
                                    (" ", t.as_str(), Style::default().fg(Color::DarkGray), num, Style::default().fg(Color::DarkGray))
                                }
                                DiffLine::Removed(t) => {
                                    let num = format!("{:>width$}", before_pos, width = (gutter_w - 1) as usize);
                                    before_pos += 1;
                                    ("-", t.as_str(), Style::default().fg(Color::Red), num, Style::default().fg(Color::Red))
                                }
                                DiffLine::Added(t) => {
                                    let num = format!("{:>width$}", after_pos, width = (gutter_w - 1) as usize);
                                    after_pos += 1;
                                    ("+", t.as_str(), Style::default().fg(Color::Green), num, Style::default().fg(Color::Green))
                                }
                            };
                            (line_idx, prefix, text.to_string(), style, gutter_text, gutter_style)
                        }).collect();

                        element! {
                            View(key: format!("{key}-hunk-{hunk_idx}")) {
                                #(for (line_idx, prefix, text, style, gutter_text, gutter_style) in &lines_rendered {
                                    HStack(key: format!("{key}-hunk-{hunk_idx}-line-{line_idx}")) {
                                        View(width: WidthConstraint::Fixed(gutter_w)) {
                                            Text { Span(text: gutter_text, style: *gutter_style) }
                                        }
                                        View {
                                            Text {
                                                Span(text: *prefix, style: *style)
                                                Span(text: text, style: *style)
                                            }
                                        }
                                    }
                                })
                            }
                        }
                    })
                })
            }
        }
    }
}

/// Render a file write tool call status with the target path.
fn file_write_tool_view(status: &turn::ToolResultStatus, path: &std::path::Path) -> Elements {
    let display_path = path.display();
    match status {
        turn::ToolResultStatus::Pending => {
            element! {
                Spinner(
                    label: format!("Writing: {display_path}"),
                    label_style: Style::default().fg(Color::Yellow),
                    done: false,
                )
            }
        }
        turn::ToolResultStatus::Success => {
            element! {
                Spinner(label: format!("Wrote: {display_path}"), done: true)
            }
        }
        turn::ToolResultStatus::Error => {
            element! {
                Text {
                    Span(text: "✗ ", style: Style::default().fg(Color::Red))
                    Span(text: format!("Write {display_path}: denied"), style: Style::default().fg(Color::Red))
                }
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Tool group view functions
// ───────────────────────────────────────────────────────────────────

/// Max entries shown under a tool group header. When the group holds more
/// than this, only the most recent `MAX_GROUP_ENTRIES` are displayed; the
/// count in the header line tells the full story.
const MAX_GROUP_ENTRIES: usize = 5;

/// Format a filesystem path for display in tool rows.
///
/// - Relative to the current working directory if the path is under it
/// - `~/...` prefix if the path is under the user's home directory
/// - Absolute otherwise (and relative paths pass through unchanged)
fn format_path_for_display(path: &std::path::Path) -> String {
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(relative) = path.strip_prefix(&cwd)
    {
        return relative.display().to_string();
    }

    if let Ok(home) = std::env::var("HOME")
        && let Ok(relative) = path.strip_prefix(&home)
    {
        return format!("~/{}", relative.display());
    }

    path.display().to_string()
}

fn filter_mode_label(mode: &HistorySearchFilterMode) -> &'static str {
    match mode {
        HistorySearchFilterMode::Global => "global",
        HistorySearchFilterMode::Host => "host",
        HistorySearchFilterMode::Session => "session",
        HistorySearchFilterMode::Directory => "directory",
        HistorySearchFilterMode::Workspace => "workspace",
    }
}

/// Format a list of filter modes as `"(global, workspace)"`, or an empty
/// string if the list is empty.
fn format_filter_modes(modes: &[HistorySearchFilterMode]) -> String {
    if modes.is_empty() {
        return String::new();
    }
    let parts: Vec<&'static str> = modes.iter().map(filter_mode_label).collect();
    format!("({})", parts.join(", "))
}

/// Tree-connector marker for a row in a grouped list: `└ ` for the first
/// visible row, two spaces for subsequent rows.
fn tree_marker(is_first: bool) -> &'static str {
    if is_first { "└ " } else { "  " }
}

/// 2-char status marker column: ✓ / ✗ / blank.
fn status_marker_view(status: &turn::ToolResultStatus) -> Elements {
    match status {
        turn::ToolResultStatus::Pending => element! {
            Text { Span(text: "  ") }
        },
        turn::ToolResultStatus::Success => element! {
            Text { Span(text: "✓ ", style: Style::default().fg(Color::Green)) }
        },
        turn::ToolResultStatus::Error => element! {
            Text { Span(text: "✗ ", style: Style::default().fg(Color::Red)) }
        },
    }
}

/// Compute the slice of calls to show — the most recent `MAX_GROUP_ENTRIES`.
fn visible_group_calls(group: &turn::ToolGroup) -> &[turn::ToolCallDetails] {
    let start = group.calls.len().saturating_sub(MAX_GROUP_ENTRIES);
    &group.calls[start..]
}

/// Render a single row in a grouped list: [tree marker][status][content].
fn group_row_view(is_first: bool, status: &turn::ToolResultStatus, content: Elements) -> Elements {
    element! {
        HStack {
            View(width: WidthConstraint::Fixed(2)) {
                Text { Span(text: tree_marker(is_first)) }
            }
            View(width: WidthConstraint::Fixed(2)) {
                #(status_marker_view(status))
            }
            Column {
                #(content)
            }
        }
    }
}

/// Render a group of consecutive `read_file` tool calls.
fn file_read_group_view(group: &turn::ToolGroup) -> Elements {
    let count = group.calls.len();
    let label = if count == 1 {
        "Read 1 file".to_string()
    } else {
        format!("Read {count} files")
    };
    let done = !group.any_pending();
    let visible = visible_group_calls(group);

    element! {
        Spinner(label: label, done: done, hide_checkmark: true)
        #(for (i, details) in visible.iter().enumerate() {
            #(file_read_row(i == 0, details))
        })
    }
}

fn file_read_row(is_first: bool, details: &turn::ToolCallDetails) -> Elements {
    let path_str = match &details.render_data {
        turn::ToolRenderData::FileRead { path } => format_path_for_display(path),
        _ => String::new(),
    };

    let content = element! {
        Text { Span(text: path_str) }
    };

    group_row_view(is_first, &details.status, content)
}

/// Render a group of consecutive `atuin_history` tool calls.
fn history_search_group_view(group: &turn::ToolGroup) -> Elements {
    let done = !group.any_pending();
    let visible = visible_group_calls(group);

    element! {
        Spinner(label: "Searched Atuin history:", done: done, hide_checkmark: true)
        #(for (i, details) in visible.iter().enumerate() {
            #(history_search_row(i == 0, details))
        })
    }
}

fn history_search_row(is_first: bool, details: &turn::ToolCallDetails) -> Elements {
    let (query, filter_modes) = match &details.render_data {
        turn::ToolRenderData::HistorySearch {
            query,
            filter_modes,
        } => (query.as_str(), filter_modes.as_slice()),
        _ => ("", [].as_slice()),
    };

    let is_empty_query = query.trim().is_empty();
    let filter_label = format_filter_modes(filter_modes);

    let content = if is_empty_query {
        element! {
            Text {
                Span(
                    text: "recent commands",
                    style: Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
                )
                #(if !filter_label.is_empty() {
                    Span(text: " ")
                    Span(text: filter_label, style: Style::default().fg(Color::DarkGray))
                })
            }
        }
    } else {
        element! {
            Text {
                Span(text: query.to_string())
                #(if !filter_label.is_empty() {
                    Span(text: " ")
                    Span(text: filter_label, style: Style::default().fg(Color::DarkGray))
                })
            }
        }
    };

    group_row_view(is_first, &details.status, content)
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
