//! Turning a capture into the plain text we index.

use atuin_client::history::CommandCapture;
use vt100::capture::basic_formatted_to_plain;

/// The visible, plain text of a capture, as fed to the search index.
///
/// A capture's rendered output is "basic formatted" (SGR colour escapes plus `\n`, produced by
/// `vt100::capture`); [`basic_formatted_to_plain`] is the matching inverse that yields just the
/// visible words, so a search for `error` matches colourised output. The two halves of a truncated
/// capture are joined with a newline.
pub(super) fn indexable_text(capture: &CommandCapture) -> String {
    // Plain text is never longer than its formatted form, so this upper-bounds the capacity and
    // the string never has to grow.
    let mut out = String::with_capacity(
        capture.output_start.len() + capture.output_end.as_ref().map_or(0, |end| end.len() + 1),
    );
    out.extend(basic_formatted_to_plain(&capture.output_start));
    if let Some(end) = &capture.output_end {
        out.push('\n');
        out.extend(basic_formatted_to_plain(end));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(start: &str, end: Option<&str>) -> CommandCapture {
        CommandCapture {
            output_start: start.to_string(),
            output_end: end.map(str::to_string),
            output_observed_bytes: 0,
            terminal_width: 80,
            terminal_height: 24,
        }
    }

    #[test]
    fn drops_sgr_formatting_and_keeps_the_visible_text() {
        // Red "error", reset -- the SGR escapes must not reach the index.
        let text = indexable_text(&cap("\x1b[31merror\x1b[0m here", None));
        assert_eq!(text, "error here");
    }

    #[test]
    fn joins_both_halves_of_a_truncated_capture_with_a_newline() {
        let text = indexable_text(&cap("\x1b[1mstart\x1b[0m", Some("\x1b[1mend\x1b[0m")));
        assert_eq!(text, "start\nend");
    }
}
