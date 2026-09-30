use crate::markers::{Event, Parser};

pub const RESET: &[u8] = b"\x1b[0m";

pub struct MarkerDebugHighlighter {
    parser: Parser,
}

impl MarkerDebugHighlighter {
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
        Event::OutputStart(_) => b"\x1b[1;30;46m[atuin: output start]\x1b[0m",
        Event::OutputEnd(_) => b"\x1b[1;37;44m[atuin: output end]\x1b[0m",
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::history::HistoryId;
    use rstest::{fixture, rstest};

    use super::*;

    const HID: &str = "00000000-0000-0000-0000-0000000000a1";

    fn hid() -> HistoryId {
        HID.parse().expect("valid history id")
    }

    /// Strip every debug label (and the reset that follows it) back out of rendered output.
    fn without_labels(rendered: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = rendered;
        'outer: while !rest.is_empty() {
            for event in [Event::OutputStart(hid()), Event::OutputEnd(hid())] {
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

    #[fixture]
    fn highlighter() -> MarkerDebugHighlighter {
        MarkerDebugHighlighter::new()
    }

    #[rstest]
    #[case::no_markers(b"plain output\r\n")]
    #[case::csi_only(b"\x1b[32mgreen\x1b[0m")]
    #[case::other_osc(b"\x1b]0;window title\x07")]
    #[case::osc_133(b"\x1b]133;C\x07")]
    fn passes_unmarked_data_through_unchanged(
        mut highlighter: MarkerDebugHighlighter,
        #[case] data: &[u8],
    ) {
        assert_eq!(highlighter.render(data), data);
    }

    #[rstest]
    #[case::start_bel(format!("\x1b]18188735;C;{HID}\x07"), Event::OutputStart(hid()))]
    #[case::start_st(format!("\x1b]18188735;C;{HID}\x1b\\"), Event::OutputStart(hid()))]
    #[case::end(format!("\x1b]18188735;D;{HID}\x07"), Event::OutputEnd(hid()))]
    fn markers_are_passed_through_before_the_label(
        mut highlighter: MarkerDebugHighlighter,
        #[case] marker: String,
        #[case] event: Event,
    ) {
        let mut expected = marker.clone().into_bytes();
        expected.extend_from_slice(event_label(event));
        expected.extend_from_slice(RESET);

        assert_eq!(highlighter.render(marker.as_bytes()), expected);
    }

    #[rstest]
    fn labels_every_marker_in_a_full_cycle(mut highlighter: MarkerDebugHighlighter) {
        let data = format!("$ ls\r\n\x1b]18188735;C;{HID}\x07file\r\n\x1b]18188735;D;{HID}\x07");
        let rendered = highlighter.render(data.as_bytes());

        // Every marker survives verbatim, in place, so downstream consumers (the capture
        // tracker sees the highlighted stream, not the raw one) still work.
        assert_eq!(without_labels(&rendered), data.as_bytes());
        for event in [Event::OutputStart(hid()), Event::OutputEnd(hid())] {
            let label = event_label(event);
            assert!(
                rendered.windows(label.len()).any(|window| window == label),
                "missing label for {event:?}"
            );
        }
    }

    #[rstest]
    fn a_marker_split_across_renders_is_still_passed_through(
        mut highlighter: MarkerDebugHighlighter,
    ) {
        let (head, tail) = HID.split_at(20);
        let first = format!("out\x1b]18188735;D;{head}");
        assert_eq!(highlighter.render(first.as_bytes()), first.as_bytes());

        let second = highlighter.render(format!("{tail}\x07more").as_bytes());
        let mut expected = format!("{tail}\x07").into_bytes();
        expected.extend_from_slice(event_label(Event::OutputEnd(hid())));
        expected.extend_from_slice(RESET);
        expected.extend_from_slice(b"more");
        assert_eq!(second, expected);
    }

    #[rstest]
    fn start_and_end_get_distinct_labels() {
        assert_ne!(event_label(Event::OutputStart(hid())), event_label(Event::OutputEnd(hid())));
    }
}
