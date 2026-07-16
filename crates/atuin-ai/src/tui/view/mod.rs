//! View functions that build eye-declare elements from turn data.
//!
//! Every function here is pure and returns an element that owns its strings
//! (`AnyElement<'static>`), so turns can be rendered into the live tail or
//! pushed to scrollback with the same code. Message emission lives in the
//! app's keymap, not here — elements are display-only.

use eye_declare::{
    AnyElement, Element, ElementExt, Fluent, col, empty, markdown, row, spinner, text, viewport,
};
use atuin_common::path::DisplayRichExt;
use ratatui_core::style::{Color, Modifier, Style};

use crate::fsm::tools::InterruptReason;
use crate::tools::{HistorySearchFilterMode, ToolPreview};

pub(crate) mod input;
pub(crate) mod turn;

use turn::{
    ConfidenceLevel, DangerLevel, OutOfBandOutputDetails, SuggestedCommandDetails, ToolCallDetails,
    ToolGroup, ToolGroupKind, ToolRenderData, ToolResultStatus, ToolSummary, UiEvent, UiTurn,
    UiTurnKind,
};

/// Max output lines shown for a shell command preview.
const MAX_SHELL_PREVIEW_LINES: u16 = 5;

/// Max entries shown under a tool group header. When the group holds more,
/// only the most recent entries are displayed; the header count tells the
/// full story.
const MAX_GROUP_ENTRIES: usize = 5;

/// Render one turn. `first` suppresses the leading blank row; `busy` shows
/// the working spinner on the last agent turn; `showing_ui` suppresses it
/// while a prompt (e.g. permission select) is on screen.
pub(crate) fn turn_view(
    turn: &UiTurn,
    first: bool,
    busy: bool,
    showing_ui: bool,
) -> AnyElement<'static> {
    match &turn.kind {
        UiTurnKind::User { events } => user_turn_view(events, first),
        UiTurnKind::Agent { events } => agent_turn_view(events, busy, showing_ui),
        UiTurnKind::OutOfBand { events } => out_of_band_turn_view(events),
    }
}

pub(crate) fn user_turn_view(events: &[UiEvent], first_turn: bool) -> AnyElement<'static> {
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::REVERSED);

    col()
        .child(text(" You ").style(label_style))
        .children(events.iter().filter_map(|event| match event {
            UiEvent::Text { content } => Some(text(content.clone()).pad_left(2)),
            _ => None,
        }))
        .pad_top(if first_turn { 0 } else { 1 })
        .any()
}

pub(crate) fn agent_turn_view(
    events: &[UiEvent],
    busy: bool,
    showing_ui: bool,
) -> AnyElement<'static> {
    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::REVERSED);

    col()
        .child(text(" Atuin AI ").style(label_style))
        .children(events.iter().enumerate().map(|(i, event)| {
            col()
                .when(i > 0, |c| c.child(text("")))
                .child(event_view(event))
        }))
        .when(busy && !showing_ui, |c| {
            c.child(
                spinner("")
                    .spinner_style(
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )
                    .pad_left(2)
                    .pad_top(1),
            )
        })
        .any()
}

pub(crate) fn out_of_band_turn_view(events: &[UiEvent]) -> AnyElement<'static> {
    let label_style = Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::REVERSED);

    col()
        .child(text(" System ").style(label_style))
        .children(events.iter().filter_map(|event| match event {
            UiEvent::OutOfBandOutput(details) => Some(out_of_band_output_view(details)),
            _ => None,
        }))
        .any()
}

fn out_of_band_output_view(details: &OutOfBandOutputDetails) -> AnyElement<'static> {
    col()
        .when_some(details.command.clone(), |c, command| {
            c.child(text(command).style(Style::default().fg(Color::Blue)))
        })
        .child(markdown(details.content.clone()))
        .pad_left(2)
        .any()
}

fn event_view(event: &UiEvent) -> AnyElement<'static> {
    match event {
        UiEvent::Text { content } => markdown(content.clone()).pad_left(2).any(),
        UiEvent::ToolSummary(summary) => tool_summary_view(summary),
        UiEvent::SuggestedCommand(details) => suggested_command_view(details),
        UiEvent::ToolCall(details) => tool_call_view(details).pad_left(2).any(),
        UiEvent::ToolGroup(group) => group_view(group).pad_left(2).any(),
        UiEvent::OutOfBandOutput(_) => empty().any(),
    }
}

fn tool_summary_view(summary: &ToolSummary) -> AnyElement<'static> {
    spinner(summary.summary())
        .done(!summary.any_pending())
        .any()
}

fn tool_call_view(details: &ToolCallDetails) -> AnyElement<'static> {
    match &details.render_data {
        ToolRenderData::Shell { command, preview } => shell_tool_view(command, preview.as_ref()),
        ToolRenderData::FileEdit { path, preview } => {
            file_edit_tool_view(&details.status, path, preview.as_ref())
        }
        ToolRenderData::FileWrite { path, preview } => {
            file_write_tool_view(&details.status, path, preview.as_ref())
        }
        ToolRenderData::Remote => tool_status_view(&details.name, &details.status),
        ToolRenderData::FileRead { .. }
        | ToolRenderData::HistorySearch { .. }
        | ToolRenderData::SkillLoad { .. } => empty().any(),
    }
}

/// Status indicator for a non-preview tool call (e.g. a server-side tool).
fn tool_status_view(name: &str, status: &ToolResultStatus) -> AnyElement<'static> {
    match status {
        ToolResultStatus::Pending => spinner(format!("Running: {name}"))
            .label_style(Style::default().fg(Color::Yellow))
            .done(false)
            .any(),
        ToolResultStatus::Success => spinner(format!("Ran: {name}")).done(true).any(),
        ToolResultStatus::Error => text("✗ ")
            .style(Style::default().fg(Color::Red))
            .span(format!("{name}: denied"), Style::default().fg(Color::Red))
            .any(),
    }
}

// ───────────────────────────────────────────────────────────────────
// Shell tool
// ───────────────────────────────────────────────────────────────────

/// Shell command execution with live output viewport.
fn shell_tool_view(command: &str, preview: Option<&ToolPreview>) -> AnyElement<'static> {
    let done = preview.is_some_and(|p| p.exit_code.is_some() || p.interrupted.is_some());

    match preview {
        Some(preview) => col()
            .child(
                spinner(if done {
                    format!("Ran: {command}")
                } else {
                    format!("Running: {command}")
                })
                .done(done)
                .hide_checkmark(),
            )
            .child(
                row().fixed(2, text("└ ")).fill(
                    viewport(preview.lines.iter().cloned())
                        .height((preview.lines.len() as u16).clamp(1, MAX_SHELL_PREVIEW_LINES))
                        .style(Style::default().fg(Color::Gray))
                        .wrap(false),
                ),
            )
            .child(shell_tool_footer(preview, done))
            .any(),
        None => spinner(format!("Running: {command}"))
            .label_style(Style::default().fg(Color::Yellow))
            .done(false)
            .any(),
    }
}

fn shell_tool_footer(preview: &ToolPreview, done: bool) -> AnyElement<'static> {
    if let Some(reason) = &preview.interrupted {
        let label = match reason {
            InterruptReason::User => "Interrupted".to_string(),
            InterruptReason::Timeout(secs) => format!("Timed out ({secs}s)"),
        };
        return text(label)
            .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
            .any();
    }
    if !done {
        return text("[Ctrl+C] Interrupt")
            .style(Style::default().fg(Color::DarkGray))
            .any();
    }
    if let Some(code) = preview.exit_code {
        let style = if code == 0 {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        };
        return text(format!("Exit code: {code}")).style(style).any();
    }
    empty().any()
}

// ───────────────────────────────────────────────────────────────────
// File edit / write tools
// ───────────────────────────────────────────────────────────────────

/// File edit tool call with diff preview.
fn file_edit_tool_view(
    status: &ToolResultStatus,
    path: &std::path::Path,
    preview: Option<&crate::diff::EditPreview>,
) -> AnyElement<'static> {
    let display_path = format_path_for_display(path);
    let status_line = tool_status_line(status, "Editing", "Edited", "Edit", &display_path, "");

    let Some(preview) = preview else {
        return status_line;
    };
    if preview.hunks.is_empty() {
        return status_line;
    }

    let gutter_w = gutter_width(preview.max_line_number() as usize);

    col()
        .child(status_line)
        .child(
            col()
                .children(preview.hunks.iter().map(|hunk| hunk_view(hunk, gutter_w)))
                .pad_left(2),
        )
        .any()
}

/// One diff hunk: gutter-numbered context/removed/added lines. The
/// before/after line counters mutate directly inside the `map` closure.
fn hunk_view(hunk: &crate::diff::DiffHunk, gutter_w: u16) -> impl Element + use<> {
    use crate::diff::DiffLine;

    let mut before_pos = hunk.before_start;
    let mut after_pos = hunk.after_start;
    let num_w = (gutter_w - 1) as usize;

    col().children(hunk.lines.iter().map(move |line| {
        let (prefix, content, style, gutter) = match line {
            DiffLine::Context(t) => {
                let num = format!("{after_pos:>num_w$}");
                before_pos += 1;
                after_pos += 1;
                (" ", t, Style::default().fg(Color::DarkGray), num)
            }
            DiffLine::Removed(t) => {
                let num = format!("{before_pos:>num_w$}");
                before_pos += 1;
                ("-", t, Style::default().fg(Color::Red), num)
            }
            DiffLine::Added(t) => {
                let num = format!("{after_pos:>num_w$}");
                after_pos += 1;
                ("+", t, Style::default().fg(Color::Green), num)
            }
        };

        row()
            .fixed(gutter_w, text(gutter).style(style))
            .fill(text(prefix).style(style).span(content.clone(), style))
    }))
}

/// File write tool call with content preview.
fn file_write_tool_view(
    status: &ToolResultStatus,
    path: &std::path::Path,
    preview: Option<&crate::diff::WritePreview>,
) -> AnyElement<'static> {
    let display_path = format_path_for_display(path);
    let line_info = match (status, preview) {
        (ToolResultStatus::Success, Some(p)) => format!(" ({} lines)", p.total_lines),
        _ => String::new(),
    };
    let status_line = tool_status_line(
        status,
        "Writing",
        "Wrote",
        "Write",
        &display_path,
        &line_info,
    );

    let Some(preview) = preview else {
        return status_line;
    };
    if preview.lines.is_empty() {
        return status_line;
    }

    let gutter_w = gutter_width(preview.total_lines);
    let num_w = (gutter_w - 1) as usize;
    let remaining = preview.remaining_lines();
    let dim = Style::default().fg(Color::DarkGray);

    col()
        .child(status_line)
        .child(
            col()
                .children(preview.lines.iter().enumerate().map(|(idx, line)| {
                    row()
                        .fixed(gutter_w, text(format!("{:>num_w$}", idx + 1)).style(dim))
                        .fill(text(line.clone()).style(dim))
                }))
                .when(remaining > 0, |c| {
                    c.child(text(format!("     ... +{remaining} more lines")).style(dim))
                })
                .pad_left(2),
        )
        .any()
}

/// Line-number gutter width for the highest displayed number, plus spacing.
fn gutter_width(max_line_num: usize) -> u16 {
    max_line_num.to_string().len().max(2) as u16 + 1
}

/// Shared pending/success/error status line for edit and write.
fn tool_status_line(
    status: &ToolResultStatus,
    doing: &str,
    did: &str,
    noun: &str,
    display_path: &str,
    suffix: &str,
) -> AnyElement<'static> {
    match status {
        ToolResultStatus::Pending => spinner(format!("{doing}: {display_path}"))
            .label_style(Style::default().fg(Color::Yellow))
            .done(false)
            .any(),
        ToolResultStatus::Success => spinner(format!("{did}: {display_path}{suffix}"))
            .done(true)
            .any(),
        ToolResultStatus::Error => text("✗ ")
            .style(Style::default().fg(Color::Red))
            .span(
                format!("{noun} {display_path}: failed"),
                Style::default().fg(Color::Red),
            )
            .any(),
    }
}

// ───────────────────────────────────────────────────────────────────
// Tool groups
// ───────────────────────────────────────────────────────────────────

fn group_view(group: &ToolGroup) -> AnyElement<'static> {
    match group.kind {
        ToolGroupKind::FileRead => file_read_group_view(group),
        ToolGroupKind::HistorySearch => history_search_group_view(group),
    }
}

/// Tree-connector marker: `└ ` for the first visible row, spaces after.
fn tree_marker(is_first: bool) -> &'static str {
    if is_first { "└ " } else { "  " }
}

/// 2-char status marker column: ✓ / ✗ / blank.
fn status_marker_view(status: &ToolResultStatus) -> AnyElement<'static> {
    match status {
        ToolResultStatus::Pending => text("  ").any(),
        ToolResultStatus::Success => text("✓ ").style(Style::default().fg(Color::Green)).any(),
        ToolResultStatus::Error => text("✗ ").style(Style::default().fg(Color::Red)).any(),
    }
}

/// The most recent `MAX_GROUP_ENTRIES` calls.
fn visible_group_calls(group: &ToolGroup) -> &[ToolCallDetails] {
    let start = group.calls.len().saturating_sub(MAX_GROUP_ENTRIES);
    &group.calls[start..]
}

/// One row in a grouped list: `[tree marker][status][content]`.
fn group_row_view(
    is_first: bool,
    status: &ToolResultStatus,
    content: AnyElement<'static>,
) -> AnyElement<'static> {
    row()
        .fixed(2, text(tree_marker(is_first)))
        .fixed(2, status_marker_view(status))
        .fill(content)
        .any()
}

fn file_read_group_view(group: &ToolGroup) -> AnyElement<'static> {
    let count = group.calls.len();
    let label = if count == 1 {
        "Read 1 file".to_string()
    } else {
        format!("Read {count} files")
    };

    col()
        .child(spinner(label).done(!group.any_pending()).hide_checkmark())
        .children(
            visible_group_calls(group)
                .iter()
                .enumerate()
                .map(|(i, details)| file_read_row(i == 0, details)),
        )
        .any()
}

fn file_read_row(is_first: bool, details: &ToolCallDetails) -> AnyElement<'static> {
    let path_str = match &details.render_data {
        ToolRenderData::FileRead { path } => format_path_for_display(path),
        _ => String::new(),
    };

    group_row_view(is_first, &details.status, text(path_str).any())
}

fn history_search_group_view(group: &ToolGroup) -> AnyElement<'static> {
    col()
        .child(
            spinner("Searched Atuin history:")
                .done(!group.any_pending())
                .hide_checkmark(),
        )
        .children(
            visible_group_calls(group)
                .iter()
                .enumerate()
                .map(|(i, details)| history_search_row(i == 0, details)),
        )
        .any()
}

fn history_search_row(is_first: bool, details: &ToolCallDetails) -> AnyElement<'static> {
    let (query, filter_modes) = match &details.render_data {
        turn::ToolRenderData::HistorySearch {
            query,
            filter_modes,
        } => (query.as_str(), filter_modes.as_slice()),
        _ => ("", [].as_slice()),
    };

    let filter_label = format_filter_modes(filter_modes);
    let filter_style = Style::default().fg(Color::DarkGray);

    let content = if query.trim().is_empty() {
        text("recent commands")
            .style(
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
            )
            .when(!filter_label.is_empty(), |t| {
                t.span(" ", Style::default())
                    .span(&filter_label, filter_style)
            })
            .any()
    } else {
        text(query)
            .when(!filter_label.is_empty(), |t| {
                t.span(" ", Style::default())
                    .span(&filter_label, filter_style)
            })
            .any()
    };

    group_row_view(is_first, &details.status, content)
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

// ───────────────────────────────────────────────────────────────────
// Suggested command
// ───────────────────────────────────────────────────────────────────

fn suggested_command_view(details: &SuggestedCommandDetails) -> AnyElement<'static> {
    let is_dangerous = matches!(
        details.danger_level,
        DangerLevel::High(_) | DangerLevel::Medium(_)
    );
    let danger_notes = details.danger_level.notes().cloned();
    let danger_style = match details.danger_level {
        DangerLevel::High(_) => Style::default().fg(Color::Red),
        DangerLevel::Medium(_) => Style::default().fg(Color::Yellow),
        DangerLevel::Low(_) | DangerLevel::Unknown(_) => Style::default().fg(Color::Green),
    };
    let danger_text = match details.danger_level {
        DangerLevel::High(_) => "High",
        DangerLevel::Medium(_) => "Medium",
        DangerLevel::Low(_) => "Low",
        DangerLevel::Unknown(_) => "Unknown",
    };

    let low_confidence = matches!(
        details.confidence_level,
        ConfidenceLevel::Low(_) | ConfidenceLevel::Medium(_)
    );
    let confidence_level = match details.confidence_level {
        ConfidenceLevel::Low(_) => "Low",
        ConfidenceLevel::Medium(_) => "Medium",
        ConfidenceLevel::High(_) => "High",
        ConfidenceLevel::Unknown(_) => "Unknown",
    };
    let confidence_notes = details.confidence_level.notes().cloned();

    col()
        .child(text("  Suggested command:").style(Style::default().fg(Color::Cyan)))
        .child(
            row()
                .fixed(
                    2,
                    if is_dangerous || low_confidence {
                        text("! ").style(Style::default().fg(Color::Yellow))
                    } else {
                        text("$ ").style(Style::default().fg(Color::Blue))
                    },
                )
                .fill(text(details.command.clone()).style(Style::default().fg(Color::Green))),
        )
        .when(is_dangerous, |c| {
            c.child(
                text("Danger: ")
                    .style(danger_style)
                    .span(danger_text, danger_style.add_modifier(Modifier::BOLD))
                    .pad_left(2),
            )
        })
        .when_some(
            is_dangerous.then_some(danger_notes).flatten(),
            |c, notes| c.child(row().fixed(2, text("└")).fill(markdown(notes)).pad_left(2)),
        )
        .when(low_confidence, |c| {
            c.child(
                text("Confidence: ")
                    .style(Style::default().fg(Color::Blue))
                    .span(
                        confidence_level,
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::BOLD),
                    )
                    .pad_left(2),
            )
        })
        .when_some(
            low_confidence.then_some(confidence_notes).flatten(),
            |c, notes| c.child(row().fixed(2, text("└")).fill(markdown(notes)).pad_left(2)),
        )
        .any()
}

// ───────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────

/// Format a filesystem path for display in tool rows (cwd-relative,
/// `~`-abbreviated).
fn format_path_for_display(path: &std::path::Path) -> String {
    path.display_rich().relative_to_cwd().tilde_me().to_string()
}
