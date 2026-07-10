use crate::osc133::{Event, Parser};

pub(crate) const RESET: &[u8] = b"\x1b[0m";

pub(crate) struct Osc133DebugHighlighter {
    parser: Parser,
}

impl Osc133DebugHighlighter {
    pub(crate) fn new() -> Self {
        Self {
            parser: Parser::new(),
        }
    }

    pub(crate) fn render(&mut self, data: &[u8]) -> Vec<u8> {
        let mut events = Vec::new();
        self.parser
            .push_located(data, |located| events.push(located));

        if events.is_empty() {
            return data.to_vec();
        }

        let mut rendered = Vec::with_capacity(data.len() + (events.len() * 64));
        let mut start = 0;

        for located in events {
            let offset = located.offset.min(data.len());
            if offset > start {
                rendered.extend_from_slice(&data[start..offset]);
            }

            rendered.extend_from_slice(event_label(&located.event));
            rendered.extend_from_slice(RESET);
            start = offset;
        }

        rendered.extend_from_slice(&data[start..]);
        rendered
    }
}

fn event_label(event: &Event) -> &'static [u8] {
    match event {
        Event::PromptStart => b"\x1b[1;37;45m[OSC133:A prompt]\x1b[0m",
        Event::CommandStart => b"\x1b[1;30;43m[OSC133:B input]\x1b[0m",
        Event::CommandExecuted => b"\x1b[1;30;46m[OSC133:C output]\x1b[0m",
        Event::CommandFinished { exit_code: Some(0) } => b"\x1b[1;37;42m[OSC133:D exit=0]\x1b[0m",
        Event::CommandFinished { exit_code: Some(_) } => b"\x1b[1;37;41m[OSC133:D exit!=0]\x1b[0m",
        Event::CommandFinished { exit_code: None } => b"\x1b[1;37;44m[OSC133:D exit=?]\x1b[0m",
    }
}
