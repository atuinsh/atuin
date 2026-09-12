use crate::osc133::{Event, Parser};

pub const RESET: &[u8] = b"\x1b[0m";

pub struct Osc133DebugHighlighter {
    parser: Parser,
}

impl Osc133DebugHighlighter {
    pub(crate) fn new() -> Self {
        Self {
            parser: Parser::new(),
        }
    }

    #[must_use]
    pub(crate) fn render(&mut self, data: &[u8]) -> Vec<u8> {
        let mut chunk_iter = self.parser.push(data);
        let chunks: Vec<_> = chunk_iter.by_ref().collect();

        if chunks.is_empty() {
            return data.to_vec();
        }

        let mut rendered = Vec::with_capacity(data.len() + (chunks.len() * 64));

        for chunk in chunks {
            rendered.extend_from_slice(chunk.data);
            rendered.extend_from_slice(event_label(chunk.event));
            rendered.extend_from_slice(RESET);
        }

        rendered.extend_from_slice(chunk_iter.trailing_data());
        rendered
    }
}

fn event_label(event: Event) -> &'static [u8] {
    match event {
        Event::PromptStart => b"\x1b[1;37;45m[OSC133:A prompt]\x1b[0m",
        Event::CommandStart => b"\x1b[1;30;43m[OSC133:B input]\x1b[0m",
        Event::CommandExecuted => b"\x1b[1;30;46m[OSC133:C output]\x1b[0m",
        Event::CommandFinished { exit_code: Some(0) } => b"\x1b[1;37;42m[OSC133:D exit=0]\x1b[0m",
        Event::CommandFinished { exit_code: Some(_) } => b"\x1b[1;37;41m[OSC133:D exit!=0]\x1b[0m",
        Event::CommandFinished { exit_code: None } => b"\x1b[1;37;44m[OSC133:D exit=?]\x1b[0m",
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;

    /// Strip every debug label (and the reset that follows it) back out of rendered output.
    fn without_labels(rendered: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = rendered;
        'outer: while !rest.is_empty() {
            for event in EVENTS {
                let label = [event_label(event), RESET].concat();
                if let Some(remainder) = rest.strip_prefix(label.as_slice()) {
                    rest = remainder;
                    continue 'outer;
                }
            }
            out.push(rest[0]);
            rest = &rest[1..];
        }
        out
    }

    const EVENTS: [Event; 6] = [
        Event::PromptStart,
        Event::CommandStart,
        Event::CommandExecuted,
        Event::CommandFinished { exit_code: Some(0) },
        Event::CommandFinished { exit_code: Some(1) },
        Event::CommandFinished { exit_code: None },
    ];

    #[fixture]
    fn highlighter() -> Osc133DebugHighlighter {
        Osc133DebugHighlighter::new()
    }

    #[rstest]
    #[case::no_markers(b"plain output\r\n")]
    #[case::csi_only(b"\x1b[32mgreen\x1b[0m")]
    #[case::non_133_osc(b"\x1b]0;window title\x07")]
    fn passes_unmarked_data_through_unchanged(
        mut highlighter: Osc133DebugHighlighter,
        #[case] data: &[u8],
    ) {
        assert_eq!(highlighter.render(data), data);
    }

    #[rstest]
    #[case::prompt_start_bel(b"\x1b]133;A\x07", Event::PromptStart)]
    #[case::prompt_start_st(b"\x1b]133;A\x1b\\", Event::PromptStart)]
    #[case::prompt_start_c1_st(b"\x1b]133;A\x9c", Event::PromptStart)]
    #[case::command_start(b"\x1b]133;B\x07", Event::CommandStart)]
    #[case::command_executed(b"\x1b]133;C\x07", Event::CommandExecuted)]
    #[case::finished_zero(b"\x1b]133;D;0\x07", Event::CommandFinished { exit_code: Some(0) })]
    #[case::finished_nonzero(b"\x1b]133;D;1\x07", Event::CommandFinished { exit_code: Some(1) })]
    #[case::finished_bare(b"\x1b]133;D\x07", Event::CommandFinished { exit_code: None })]
    fn markers_are_passed_through_before_the_label(
        mut highlighter: Osc133DebugHighlighter,
        #[case] marker: &[u8],
        #[case] event: Event,
    ) {
        let mut expected = marker.to_vec();
        expected.extend_from_slice(event_label(event));
        expected.extend_from_slice(RESET);

        assert_eq!(highlighter.render(marker), expected);
    }

    #[rstest]
    fn labels_every_marker_in_a_full_cycle(mut highlighter: Osc133DebugHighlighter) {
        let data = b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07file\r\n\x1b]133;D;0\x07";
        let rendered = highlighter.render(data);

        // Every marker survives verbatim, in place, so downstream consumers (the capture
        // tracker sees the highlighted stream, not the raw one) still work.
        assert_eq!(without_labels(&rendered), data);
        for event in [
            Event::PromptStart,
            Event::CommandStart,
            Event::CommandExecuted,
            Event::CommandFinished { exit_code: Some(0) },
        ] {
            let label = event_label(event);
            assert!(
                rendered.windows(label.len()).any(|window| window == label),
                "missing label for {event:?}"
            );
        }
    }

    #[rstest]
    fn a_marker_split_across_renders_is_still_passed_through(
        mut highlighter: Osc133DebugHighlighter,
    ) {
        let first = highlighter.render(b"out\x1b]133;D");
        assert_eq!(first, b"out\x1b]133;D");

        let second = highlighter.render(b";0\x07more");
        let mut expected = b";0\x07".to_vec();
        expected.extend_from_slice(event_label(Event::CommandFinished { exit_code: Some(0) }));
        expected.extend_from_slice(RESET);
        expected.extend_from_slice(b"more");
        assert_eq!(second, expected);
    }

    #[rstest]
    #[case::zero(Some(0))]
    #[case::nonzero(Some(1))]
    #[case::unknown(None)]
    fn exit_codes_get_distinct_labels(#[case] exit_code: Option<i32>) {
        let labels: Vec<_> = EVENTS.iter().map(|event| event_label(*event)).collect();
        let label = event_label(Event::CommandFinished { exit_code });
        assert_eq!(labels.iter().filter(|other| **other == label).count(), 1);
    }
}
