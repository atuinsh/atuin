use crate::parser::{Event, Output};

pub const RESET: &[u8] = b"\x1b[0m";

impl Output {
    /// The output with a label after each marker, for debugging the shell integration.
    #[must_use]
    pub(crate) fn with_marker_labels(&self) -> Vec<u8> {
        let mut labelled = Vec::with_capacity(self.data().len());
        for chunk in self.chunks() {
            labelled.extend_from_slice(chunk.data);
            if let Some(label) = event_label(chunk.event) {
                labelled.extend_from_slice(label);
                labelled.extend_from_slice(RESET);
            }
        }
        labelled.extend_from_slice(self.trailing_data());
        labelled
    }
}

fn event_label(event: Event) -> Option<&'static [u8]> {
    match event {
        Event::OutputStart(_) => Some(b"\x1b[1;30;46m[atuin: output start]\x1b[0m"),
        Event::OutputEnd(_) => Some(b"\x1b[1;37;44m[atuin: output end]\x1b[0m"),
        Event::Title => None,
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::history::HistoryId;
    use rstest::{fixture, rstest};

    use super::*;
    use crate::parser::Parser;

    const HID: &str = "00000000-0000-0000-0000-0000000000a1";

    fn hid() -> HistoryId {
        HID.parse().expect("valid history id")
    }

    fn label(event: Event) -> &'static [u8] {
        event_label(event).expect("markers are labelled")
    }

    /// Strip every debug label (and the reset that follows it) back out of rendered output.
    fn without_labels(rendered: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = rendered;
        'outer: while !rest.is_empty() {
            for event in [Event::OutputStart(hid()), Event::OutputEnd(hid())] {
                let label = [label(event), RESET].concat();
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
    fn parser() -> Parser {
        Parser::new()
    }

    /// `data` as the terminal is sent it in debug mode.
    fn labelled(parser: &mut Parser, data: &[u8]) -> Vec<u8> {
        parser.read(data.to_vec()).with_marker_labels()
    }

    #[rstest]
    #[case::no_markers(b"plain output\r\n")]
    #[case::csi_only(b"\x1b[32mgreen\x1b[0m")]
    #[case::other_osc(b"\x1b]0;window title\x07")]
    #[case::osc_133(b"\x1b]133;C\x07")]
    fn passes_unmarked_data_through_unchanged(mut parser: Parser, #[case] data: &[u8]) {
        assert_eq!(labelled(&mut parser, data), data);
    }

    #[rstest]
    #[case::start_bel(format!("\x1b]18188735;C;{HID}\x07"), Event::OutputStart(hid()))]
    #[case::start_st(format!("\x1b]18188735;C;{HID}\x1b\\"), Event::OutputStart(hid()))]
    #[case::end(format!("\x1b]18188735;D;{HID}\x07"), Event::OutputEnd(hid()))]
    fn markers_are_passed_through_before_the_label(
        mut parser: Parser,
        #[case] marker: String,
        #[case] event: Event,
    ) {
        let mut expected = marker.clone().into_bytes();
        expected.extend_from_slice(label(event));
        expected.extend_from_slice(RESET);

        assert_eq!(labelled(&mut parser, marker.as_bytes()), expected);
    }

    #[rstest]
    fn labels_every_marker_in_a_full_cycle(mut parser: Parser) {
        let data = format!("$ ls\r\n\x1b]18188735;C;{HID}\x07file\r\n\x1b]18188735;D;{HID}\x07");
        let rendered = labelled(&mut parser, data.as_bytes());

        // Every marker survives verbatim, in place, so downstream consumers (the capture
        // tracker sees the highlighted stream, not the raw one) still work.
        assert_eq!(without_labels(&rendered), data.as_bytes());
        for event in [Event::OutputStart(hid()), Event::OutputEnd(hid())] {
            let label = label(event);
            assert!(
                rendered.windows(label.len()).any(|window| window == label),
                "missing label for {event:?}"
            );
        }
    }

    #[rstest]
    fn a_marker_split_across_reads_is_still_passed_through(mut parser: Parser) {
        let (head, tail) = HID.split_at(20);
        let first = format!("out\x1b]18188735;D;{head}");
        assert_eq!(labelled(&mut parser, first.as_bytes()), first.as_bytes());

        let second = labelled(&mut parser, format!("{tail}\x07more").as_bytes());
        let mut expected = format!("{tail}\x07").into_bytes();
        expected.extend_from_slice(label(Event::OutputEnd(hid())));
        expected.extend_from_slice(RESET);
        expected.extend_from_slice(b"more");
        assert_eq!(second, expected);
    }

    #[rstest]
    fn start_and_end_get_distinct_labels() {
        assert_ne!(label(Event::OutputStart(hid())), label(Event::OutputEnd(hid())));
    }
}
