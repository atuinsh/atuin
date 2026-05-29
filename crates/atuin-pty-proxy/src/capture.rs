use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use crate::osc133::{Event, Params, Parser, Zone};

const HISTORY_ID_PARAM: &str = "history_id";
const SESSION_PARAM: &str = "session";
const SESSION_ID_PARAM: &str = "session_id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCapture {
    pub prompt: String,
    pub command: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub history_id: Option<String>,
    pub session_id: Option<String>,
}

pub type CommandCaptureSink = Box<dyn Fn(CommandCapture) + Send + 'static>;

#[derive(Default)]
struct CaptureBuffers {
    prompt: Vec<u8>,
    command: Vec<u8>,
    output: Vec<u8>,
    exit_code: Option<i32>,
    history_id: Option<String>,
    session_id: Option<String>,
}

pub(crate) struct CommandCaptureTracker {
    parser: Parser,
    zone: Zone,
    buffers: CaptureBuffers,
    cols: Arc<AtomicU16>,
}

impl CommandCaptureTracker {
    pub(crate) fn new(cols: Arc<AtomicU16>) -> Self {
        Self {
            parser: Parser::new(),
            zone: Zone::Unknown,
            buffers: CaptureBuffers::default(),
            cols,
        }
    }

    pub(crate) fn push(&mut self, data: &[u8], mut on_capture: impl FnMut(CommandCapture)) {
        let mut events = Vec::new();
        self.parser
            .push_located(data, |located| events.push(located));

        let mut start = 0;
        for located in events {
            let offset = located.offset.min(data.len());
            self.append(&data[start..offset]);
            self.handle_event(located.event, &located.params, &mut on_capture);
            self.zone = located.zone;
            start = offset;
        }

        self.append(&data[start..]);
    }

    fn append(&mut self, data: &[u8]) {
        match self.zone {
            Zone::Prompt => self.buffers.prompt.extend_from_slice(data),
            Zone::Input => self.buffers.command.extend_from_slice(data),
            Zone::Output => self.buffers.output.extend_from_slice(data),
            Zone::Unknown => {}
        }
    }

    fn handle_event(
        &mut self,
        event: Event,
        params: &Params,
        on_capture: &mut impl FnMut(CommandCapture),
    ) {
        match event {
            Event::PromptStart => self.buffers = CaptureBuffers::default(),
            Event::CommandStart | Event::CommandExecuted => {}
            Event::CommandFinished { exit_code } => {
                self.buffers.exit_code = exit_code;
                self.buffers.history_id = params.get(HISTORY_ID_PARAM).map(str::to_owned);
                self.buffers.session_id = params
                    .get(SESSION_PARAM)
                    .or_else(|| params.get(SESSION_ID_PARAM))
                    .map(str::to_owned);
                if let Some(capture) = self.finish_capture() {
                    on_capture(capture);
                }
            }
        }
    }

    fn finish_capture(&mut self) -> Option<CommandCapture> {
        let buffers = std::mem::take(&mut self.buffers);
        let cols = self.cols.load(Ordering::Relaxed).max(1);
        let prompt = render_plain_text(&buffers.prompt, cols);
        let command = render_plain_text(&buffers.command, cols)
            .trim_matches(|c| c == '\r' || c == '\n')
            .to_string();
        let output = render_plain_text(&buffers.output, cols);
        let exit_code = buffers.exit_code;
        let history_id = buffers.history_id;
        let session_id = buffers.session_id;

        if command.is_empty() && output.is_empty() {
            return None;
        }

        Some(CommandCapture {
            prompt,
            command,
            output,
            exit_code,
            history_id,
            session_id,
        })
    }
}

const CLEAN_TEXT_MAX_ROWS: usize = 10_000;

fn render_plain_text(bytes: &[u8], cols: u16) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    let cols = cols.max(1);
    let mut parser = vt100::Parser::new(estimated_rows(bytes, cols), cols, 0);
    parser.process(bytes);
    normalize_screen_contents(&parser.screen().contents())
}

fn normalize_screen_contents(contents: &str) -> String {
    let mut lines = contents.lines().map(str::trim_end).collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

fn estimated_rows(bytes: &[u8], cols: u16) -> u16 {
    let newline_rows = bytes.iter().filter(|byte| **byte == b'\n').count() + 1;
    let wrapped_rows = bytes.len() / cols as usize;
    newline_rows
        .saturating_add(wrapped_rows)
        .saturating_add(1)
        .clamp(1, CLEAN_TEXT_MAX_ROWS) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(cols: u16) -> CommandCaptureTracker {
        CommandCaptureTracker::new(Arc::new(AtomicU16::new(cols)))
    }

    fn assert_no_terminal_controls(text: &str) {
        assert!(
            !text
                .chars()
                .any(|ch| ch.is_control() && ch != '\n' && ch != '\t'),
            "text still contains terminal controls: {text:?}"
        );
    }

    #[test]
    fn command_text_collapses_terminal_echo_edits() {
        assert_eq!(render_plain_text(b"e\x08echo hi", 80), "echo hi");
        assert_eq!(
            render_plain_text(
                b"e\x08echo\x08 \x08\x08 \x08\x08\x08e \x08\x08 \x08e\x08echo hi",
                80
            ),
            "echo hi"
        );
        assert_eq!(render_plain_text(b"echo hi", 80), "echo hi");
    }

    #[test]
    fn text_cleaning_strips_ansi_and_terminal_controls() {
        let text = render_plain_text(
            b"\x1b[32mhi\x1b[0m\r\n%                                    \r \r",
            80,
        );

        assert_eq!(text, "hi");
        assert_no_terminal_controls(&text);
    }

    #[test]
    fn text_cleaning_preserves_valid_utf8_after_backspace() {
        let text = render_plain_text("🦀x\x08 \x08 crab".as_bytes(), 80);

        assert_eq!(text, "🦀 crab");
        assert_no_terminal_controls(&text);
    }

    #[test]
    fn command_text_replays_backspaces() {
        let mut tracker = tracker(80);
        let mut captures = Vec::new();

        let input =
            b"\x1b]133;A\x07$ \x1b]133;B\x07e\x08echo hi\r\n\x1b]133;C\x07hi\r\n\x1b]133;D;0\x07";
        tracker.push(input, |capture| captures.push(capture));

        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].command, "echo hi");
        assert_eq!(captures[0].output, "hi");
        assert_no_terminal_controls(&captures[0].command);
        assert_no_terminal_controls(&captures[0].output);
    }

    #[test]
    fn captures_complete_command() {
        let mut tracker = tracker(80);
        let mut captures = Vec::new();

        tracker.push(
            b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07hi\r\n\x1b]133;D;0\x07",
            |capture| captures.push(capture),
        );

        assert_eq!(
            captures,
            vec![CommandCapture {
                prompt: "$".to_string(),
                command: "echo hi".to_string(),
                output: "hi".to_string(),
                exit_code: Some(0),
                history_id: None,
                session_id: None,
            }]
        );
    }

    #[test]
    fn strips_ansi_and_split_markers() {
        let mut tracker = tracker(80);
        let mut captures = Vec::new();

        tracker.push(b"\x1b]133;A\x07\x1b[32m%\x1b[0m ", |_| {});
        tracker.push(b"\x1b]133;B\x07ls\x1b]133;C", |_| {});
        tracker.push(b"\x07\x1b[31mfile\x1b[0m\r\n\x1b]133;D;1\x07", |capture| {
            captures.push(capture);
        });

        assert_eq!(
            captures,
            vec![CommandCapture {
                prompt: "%".to_string(),
                command: "ls".to_string(),
                output: "file".to_string(),
                exit_code: Some(1),
                history_id: None,
                session_id: None,
            }]
        );
    }

    #[test]
    fn captures_output_with_history_metadata_from_d_marker() {
        let mut tracker = tracker(80);
        let mut captures = Vec::new();

        tracker.push(
            b"\x1b]133;C\x07line one\r\n\x1b]133;D;0;history_id=018f;session=abcd\x07",
            |capture| captures.push(capture),
        );

        assert_eq!(
            captures,
            vec![CommandCapture {
                prompt: String::new(),
                command: String::new(),
                output: "line one".to_string(),
                exit_code: Some(0),
                history_id: Some("018f".to_string()),
                session_id: Some("abcd".to_string()),
            }]
        );
    }

    #[test]
    fn resets_buffers_between_c_d_only_captures() {
        let mut tracker = tracker(80);
        let mut captures = Vec::new();

        tracker.push(
            b"\x1b]133;C\x07first\r\n\x1b]133;D;0;history_id=one\x07\x1b]133;C\x07second\r\n\x1b]133;D;1;history_id=two\x07",
            |capture| captures.push(capture),
        );

        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0].output, "first");
        assert_eq!(captures[0].history_id.as_deref(), Some("one"));
        assert_eq!(captures[1].output, "second");
        assert_eq!(captures[1].history_id.as_deref(), Some("two"));
    }
}
