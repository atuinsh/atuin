//! Width-aware one-line labels that middle-elide a command instead of
//! clipping it at the right edge.
//!
//! eye-declare's `Spinner` hard-clips its label and `Text` word-wraps;
//! neither suits a long shell command on a status line, where the tail
//! (flags, paths) matters as much as the head. These elements measure at
//! render time and splice an ellipsis into the middle so both ends stay
//! visible. Truncation is display-only — anywhere the user must review a
//! command in full (permission prompts for shell, suggested commands)
//! keeps the wrapping renderers.

use std::time::Duration;

use atuin_common::string::EllipsizeExt as _;
use atuin_common::string::Measure;
use atuin_common::string::ellipsis::{Indicator, Pos};
use eye_declare::{Element, Spinner};
use ratatui_core::buffer::Buffer;
use ratatui_core::layout::Rect;
use ratatui_core::style::Style;
use unicode_width::UnicodeWidthStr as _;

/// Fit `command` into `cols` display columns on a single line: whitespace
/// runs (including newlines in multiline commands) collapse to single
/// spaces, and the middle is elided when it still doesn't fit.
fn fit_middle(command: &str, cols: usize) -> String {
    let one_line = command.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line
        .ellipsize(Measure::Columns(cols), Pos::Middle, Indicator::UNICODE)
        .to_string()
}

/// A tool spinner labeled `prefix` + command, the command middle-elided to
/// the render width. Styling matches [`super::tool_spinner`]; the done
/// checkmark is always hidden, as in the shell tool view.
pub(crate) struct CommandSpinner {
    prefix: &'static str,
    command: String,
    done: bool,
}

pub(crate) fn command_spinner(
    prefix: &'static str,
    command: impl Into<String>,
    done: bool,
) -> CommandSpinner {
    CommandSpinner {
        prefix,
        command: command.into(),
        done,
    }
}

impl CommandSpinner {
    fn spinner_at(&self, width: u16) -> Spinner {
        // Marker glyph + trailing space while animating; a done spinner
        // with a hidden checkmark has no marker (see Spinner::render).
        let marker_cols = if self.done { 0 } else { 2 };
        let budget = (width as usize)
            .saturating_sub(marker_cols)
            .saturating_sub(self.prefix.width());
        let label = format!("{}{}", self.prefix, fit_middle(&self.command, budget));
        super::tool_spinner(label, self.done).hide_checkmark()
    }
}

impl Element for CommandSpinner {
    fn height(&self, width: u16) -> u16 {
        if width == 0 { 0 } else { 1 }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.spinner_at(area.width).render(area, buf);
    }

    fn animated(&self) -> Option<Duration> {
        // Delegate so the frame interval stays in lockstep with Spinner's.
        self.spinner_at(0).animated()
    }
}

/// A one-line `prefix` + value, the value middle-elided to the render
/// width rather than wrapped.
pub(crate) struct TruncatedLine {
    prefix: String,
    prefix_style: Style,
    value: String,
    value_style: Style,
}

pub(crate) fn truncated_line(
    prefix: impl Into<String>,
    prefix_style: Style,
    value: impl Into<String>,
    value_style: Style,
) -> TruncatedLine {
    TruncatedLine {
        prefix: prefix.into(),
        prefix_style,
        value: value.into(),
        value_style,
    }
}

impl Element for TruncatedLine {
    fn height(&self, width: u16) -> u16 {
        if width == 0 { 0 } else { 1 }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let width = area.width as usize;
        buf.set_stringn(area.x, area.y, &self.prefix, width, self.prefix_style);
        let used = self.prefix.width().min(width);
        let budget = width - used;
        if budget == 0 {
            return;
        }
        buf.set_stringn(
            area.x + used as u16,
            area.y,
            fit_middle(&self.value, budget),
            budget,
            self.value_style,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_line(el: &impl Element, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        el.render(area, &mut buf);
        (0..width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn fit_middle_keeps_short_commands() {
        assert_eq!(fit_middle("cargo test", 20), "cargo test");
    }

    #[test]
    fn fit_middle_elides_the_middle() {
        let out = fit_middle("cargo test --workspace -- --nocapture history", 20);
        assert!(out.starts_with("cargo"), "head survives: {out}");
        assert!(out.ends_with("history"), "tail survives: {out}");
        assert!(out.contains('…'), "elision marked: {out}");
        assert!(out.width() <= 20);
    }

    #[test]
    fn fit_middle_flattens_multiline_commands() {
        assert_eq!(
            fit_middle("for f in *.rs\ndo\n  wc -l \"$f\"\ndone", 80),
            "for f in *.rs do wc -l \"$f\" done"
        );
    }

    #[test]
    fn done_command_spinner_shows_both_ends() {
        let el = command_spinner("Ran: ", "git log --oneline --graph --all --decorate", true);
        let line = render_line(&el, 24);
        assert!(line.starts_with("Ran: git"), "got: {line}");
        assert!(line.ends_with("decorate"), "got: {line}");
        assert!(line.contains('…'), "got: {line}");
    }

    #[test]
    fn running_command_spinner_reserves_marker_columns() {
        let el = command_spinner("Running: ", "cargo build --release --locked", false);
        // The label budget accounts for the 2-column marker, so the command's
        // tail lands exactly at the right edge instead of getting clipped.
        let line = render_line(&el, 24);
        assert!(line.contains("Running: cargo"), "got: {line}");
        assert!(line.contains('…'), "got: {line}");
        assert!(line.ends_with("locked"), "got: {line}");
        assert_eq!(line.width(), 24, "got: {line}");
    }

    #[test]
    fn command_spinner_fits_untruncated_when_room() {
        let el = command_spinner("Ran: ", "ls -la", true);
        assert_eq!(render_line(&el, 40), "Ran: ls -la");
    }

    #[test]
    fn truncated_line_elides_value_not_prefix() {
        let el = truncated_line(
            "would like to view: ",
            Style::default(),
            "docker compose up --build --force-recreate",
            Style::default(),
        );
        let line = render_line(&el, 40);
        assert!(
            line.starts_with("would like to view: docker"),
            "got: {line}"
        );
        assert!(line.ends_with("recreate"), "got: {line}");
        assert!(line.contains('…'), "got: {line}");
    }

    #[test]
    fn truncated_line_fits_untruncated_when_room() {
        let el = truncated_line("view: ", Style::default(), "ls -la", Style::default());
        assert_eq!(render_line(&el, 40), "view: ls -la");
    }
}
