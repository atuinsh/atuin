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
}

impl CommandCaptureTracker {
    pub(crate) fn new() -> Self {
        Self {
            parser: Parser::new(),
            zone: Zone::Unknown,
            buffers: CaptureBuffers::default(),
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
        let prompt = clean_text(&buffers.prompt);
        let command = clean_text(&buffers.command)
            .trim_matches(|c| c == '\r' || c == '\n')
            .to_string();
        let output = clean_text(&buffers.output);
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

fn clean_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&strip_ansi_sequences(bytes)).to_string()
}

fn strip_ansi_sequences(bytes: &[u8]) -> Vec<u8> {
    #[derive(Clone, Copy)]
    enum State {
        Ground,
        Esc,
        Csi,
        Osc,
        OscEsc,
    }

    let mut out = Vec::with_capacity(bytes.len());
    let mut state = State::Ground;

    for &byte in bytes {
        match state {
            State::Ground => {
                if byte == 0x1b {
                    state = State::Esc;
                } else {
                    out.push(byte);
                }
            }
            State::Esc => match byte {
                b'[' => state = State::Csi,
                b']' => state = State::Osc,
                _ => state = State::Ground,
            },
            State::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    state = State::Ground;
                }
            }
            State::Osc => match byte {
                0x07 => state = State::Ground,
                0x1b => state = State::OscEsc,
                _ => {}
            },
            State::OscEsc => {
                if byte == b'\\' {
                    state = State::Ground;
                } else {
                    state = State::Osc;
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_complete_command() {
        let mut tracker = CommandCaptureTracker::new();
        let mut captures = Vec::new();

        tracker.push(
            b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07hi\r\n\x1b]133;D;0\x07",
            |capture| captures.push(capture),
        );

        assert_eq!(
            captures,
            vec![CommandCapture {
                prompt: "$ ".to_string(),
                command: "echo hi".to_string(),
                output: "hi\r\n".to_string(),
                exit_code: Some(0),
                history_id: None,
                session_id: None,
            }]
        );
    }

    #[test]
    fn strips_ansi_and_split_markers() {
        let mut tracker = CommandCaptureTracker::new();
        let mut captures = Vec::new();

        tracker.push(b"\x1b]133;A\x07\x1b[32m%\x1b[0m ", |_| {});
        tracker.push(b"\x1b]133;B\x07ls\x1b]133;C", |_| {});
        tracker.push(b"\x07\x1b[31mfile\x1b[0m\r\n\x1b]133;D;1\x07", |capture| {
            captures.push(capture);
        });

        assert_eq!(
            captures,
            vec![CommandCapture {
                prompt: "% ".to_string(),
                command: "ls".to_string(),
                output: "file\r\n".to_string(),
                exit_code: Some(1),
                history_id: None,
                session_id: None,
            }]
        );
    }

    #[test]
    fn captures_output_with_history_metadata_from_d_marker() {
        let mut tracker = CommandCaptureTracker::new();
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
                output: "line one\r\n".to_string(),
                exit_code: Some(0),
                history_id: Some("018f".to_string()),
                session_id: Some("abcd".to_string()),
            }]
        );
    }

    #[test]
    fn resets_buffers_between_c_d_only_captures() {
        let mut tracker = CommandCaptureTracker::new();
        let mut captures = Vec::new();

        tracker.push(
            b"\x1b]133;C\x07first\r\n\x1b]133;D;0;history_id=one\x07\x1b]133;C\x07second\r\n\x1b]133;D;1;history_id=two\x07",
            |capture| captures.push(capture),
        );

        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0].output, "first\r\n");
        assert_eq!(captures[0].history_id.as_deref(), Some("one"));
        assert_eq!(captures[1].output, "second\r\n");
        assert_eq!(captures[1].history_id.as_deref(), Some("two"));
    }
}
