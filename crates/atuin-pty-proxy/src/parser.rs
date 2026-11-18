//! Streaming parser for the child's output.
//!
//! The output arrives in whatever pieces a read returns, cut anywhere: inside an escape code, or
//! between the bytes of a UTF-8 character. [`Parser`] follows it with `atuin-vte`, Atuin's fork of
//! `vte`, the parser the emulator is built on, so it reads the output as the emulator does. On the
//! way it:
//!
//! * splits the output after each operating system command the proxy follows, with the [`Event`]
//!   the command says, and
//! * knows whether the output so far ends partway through an escape code or a character, where
//!   bytes of the proxy's own would change the meaning of the child's.
//!
//! The operating system commands (OSC) the proxy follows:
//!
//! | Command                              | Meaning                              |
//! |--------------------------------------|--------------------------------------|
//! | `OSC 18188735 ; C ; <history id> ST` | Start of command output              |
//! | `OSC 18188735 ; D ; <history id> ST` | End of command output                |
//! | `OSC 0 ; <title> ST`, `1`, `2`       | The child set a window title or icon |
//!
//! `OSC` is `ESC ]`. `ST` is `BEL` (0x07) or `ESC \` (0x1B 0x5C).
//!
//! `OSC 18188735` is a private, Atuin-specific escape sequence. We use this to ensure that our
//! escape sequences don't conflict with any other program or terminal. 18188735 is `atuin`
//! interpreted as a base-36 number. It fits into a 32-bit signed integer, which many terminals use
//! for OSC numbers, and if truncated to a 16-bit or 8-bit integer, it produces results (35263, 191)
//! that also aren't in use. Additionally, no prefix of 18188735 of length 3 or greater is in use
//! either, in case a terminal only reads a fixed number of digits.

use atuin_client::history::HistoryId;

/// An event with the slice of data up to the end of its command.
#[derive(Debug, Clone, Copy)]
pub struct EventChunk<'a> {
    /// What the command said.
    pub event: Event,

    /// All the data between the last event and the end of this event's command.
    ///
    /// This includes the entire command itself.
    pub data: &'a [u8],

    /// The total length of the command corresponding to this event.
    pub osc_len: usize,
}

/// A read of the child's output, with what the parser found in it.
///
/// Concatenating [`EventChunk::data`] for every chunk, followed by [`Self::trailing_data`],
/// exactly reproduces [`Self::data`].
#[derive(Debug)]
pub struct Output {
    data: Vec<u8>,
    /// Each event, with where its command ends in `data`.
    events: Vec<Found>,
    ends_mid_escape_code_or_char: bool,
}

impl Output {
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The output, cut after each command the proxy follows.
    pub fn chunks(&self) -> impl Iterator<Item = EventChunk<'_>> {
        let starts = std::iter::once(0).chain(self.events.iter().map(|found| found.end));
        self.events.iter().zip(starts).map(|(found, start)| EventChunk {
            event: found.event,
            data: &self.data[start..found.end],
            osc_len: found.osc_len,
        })
    }

    /// The output after the last command the proxy follows.
    pub fn trailing_data(&self) -> &[u8] {
        &self.data[self.events.last().map_or(0, |found| found.end)..]
    }

    /// Whether the output ends partway through an escape code or a character, where bytes of the
    /// proxy's own would change the meaning of the child's.
    pub const fn ends_mid_escape_code_or_char(&self) -> bool {
        self.ends_mid_escape_code_or_char
    }
}

/// An event, and where its command ends in the output it was found in.
#[derive(Debug, Clone, Copy)]
struct Found {
    event: Event,
    end: usize,
    osc_len: usize,
}

/// A streaming parser for the child's output.
///
/// Feed each read of the output into [`Parser::read`], however the reads cut it. The parser returns
/// the read as an [`Output`], with the [`Event`]s of the commands it follows.
#[derive(Default)]
pub struct Parser {
    vt: atuin_vte::Parser,
    command: Command,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Follow `data`, the next read of the child's output.
    pub fn read(&mut self, data: Vec<u8>) -> Output {
        let mut events = Vec::new();
        let mut end = 0;
        loop {
            if let Some(command) = self.command.0.take() {
                let Some(backslash) = self.backslash(&data[end..]) else {
                    // The read stops between the `ESC` and the `\` of an `ESC \`.
                    self.command.0 = Some(command);
                    break;
                };
                end += backslash;
                events.push(Found {
                    event: command.event,
                    end,
                    osc_len: command.len + backslash,
                });
            }
            if end == data.len() {
                break;
            }
            end += self.vt.advance_until_terminated(&mut self.command, &data[end..]);
        }
        Output {
            data,
            events,
            ends_mid_escape_code_or_char: self.vt.is_reading_escape_code() || self.vt.is_mid_char(),
        }
    }

    /// How many bytes at the start of `rest` finish the command `vte` just dispatched, or nothing
    /// if `rest` is too short to tell.
    ///
    /// `vte` dispatches a command at the `ESC` of an `ESC \`. The `\` belongs with the command:
    /// handed on after the command's event, it would print on a screen the event just cleared.
    fn backslash(&mut self, rest: &[u8]) -> Option<usize> {
        // Of the bytes that end a command, only an `ESC` leaves `vte` reading an escape code.
        if !self.vt.is_reading_escape_code() || *rest.first()? != b'\\' {
            return Some(0);
        }
        self.vt.advance(&mut self.command, b"\\");
        Some(1)
    }
}

/// The command `vte` just dispatched, if the proxy follows it.
#[derive(Default)]
struct Command(Option<Dispatched>);

/// A command `vte` dispatched that the proxy follows.
struct Dispatched {
    event: Event,
    /// The command's length, up to the byte that ended it.
    len: usize,
}

impl atuin_vte::Perform for Command {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // `ESC ]`, the parameters and the `;`s between them, and the byte that ended the command.
        let len = 2 + params.iter().map(|param| param.len() + 1).sum::<usize>();
        self.0 = Event::parse(params).map(|event| Dispatched { event, len });
    }

    fn terminated(&self) -> bool {
        self.0.is_some()
    }
}

/// What an operating system command the proxy follows says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `OSC 18188735 ; C ; <history id> ST` -- start of command output.
    OutputStart(HistoryId),
    /// `OSC 18188735 ; D ; <history id> ST` -- end of command output.
    OutputEnd(HistoryId),
    /// `OSC 0`, `1` or `2` -- the child set the window title, its icon name, or both.
    Title,
}

impl Event {
    /// What the command with these `;`-separated `params` says, if it is one the proxy follows.
    fn parse(params: &[&[u8]]) -> Option<Self> {
        let history_id = |id: &[u8]| std::str::from_utf8(id).ok()?.parse::<HistoryId>().ok();
        match params {
            [b"18188735", b"C", id] => history_id(id).map(Self::OutputStart),
            [b"18188735", b"D", id] => history_id(id).map(Self::OutputEnd),
            [b"0" | b"1" | b"2", _, ..] => Some(Self::Title),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::arbitrary::any;
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    const HID: &str = "00000000-0000-0000-0000-0000000000a1";
    const HID_ONE: &str = "00000000-0000-0000-0000-000000000001";
    const HID_TWO: &str = "00000000-0000-0000-0000-000000000002";

    fn hid(s: &str) -> HistoryId {
        s.parse().expect("valid history id")
    }

    fn start(history_id: &str) -> Vec<u8> {
        format!("\x1b]18188735;C;{history_id}\x07").into_bytes()
    }

    fn end(history_id: &str) -> Vec<u8> {
        format!("\x1b]18188735;D;{history_id}\x07").into_bytes()
    }

    /// An owned copy of [`EventChunk`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct OwnedChunk {
        event: Event,
        data: Vec<u8>,
        osc_len: usize,
    }

    /// The full result of one [`Parser::read`] call.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Read {
        chunks: Vec<OwnedChunk>,
        trailing_data: Vec<u8>,
        ends_mid_escape_code_or_char: bool,
    }

    impl Read {
        fn events(&self) -> Vec<Event> {
            self.chunks.iter().map(|chunk| chunk.event).collect()
        }
    }

    fn read(parser: &mut Parser, data: &[u8]) -> Read {
        let output = parser.read(data.to_vec());
        Read {
            chunks: output
                .chunks()
                .map(|chunk| OwnedChunk {
                    event: chunk.event,
                    data: chunk.data.to_vec(),
                    osc_len: chunk.osc_len,
                })
                .collect(),
            trailing_data: output.trailing_data().to_vec(),
            ends_mid_escape_code_or_char: output.ends_mid_escape_code_or_char(),
        }
    }

    /// Read `data` with a fresh parser.
    fn parse(data: &[u8]) -> Read {
        read(&mut Parser::new(), data)
    }

    /// Collect all events from a single read.
    fn parse_events(data: &[u8]) -> Vec<Event> {
        parse(data).events()
    }

    // -- What a command says --------------------------------------------------

    #[rstest]
    #[case::start(format!("18188735;C;{HID}"), Some(Event::OutputStart(hid(HID))))]
    #[case::end(format!("18188735;D;{HID}"), Some(Event::OutputEnd(hid(HID))))]
    // The shell integration sends whatever `atuin history start` printed, which is the simple
    // (hyphen-less) form rather than the hyphenated one the other cases use.
    #[case::simple_history_id("18188735;D;000000000000000000000000000000a1".into(), Some(Event::OutputEnd(hid(HID))))]
    #[case::window_and_icon("0;title".into(), Some(Event::Title))]
    #[case::icon("1;title".into(), Some(Event::Title))]
    #[case::window("2;title".into(), Some(Event::Title))]
    #[case::empty_title("2;".into(), Some(Event::Title))]
    #[case::semicolon_in_the_title("2;a;b".into(), Some(Event::Title))]
    #[case::no_title("2".into(), None)]
    #[case::directory("7;file:///tmp".into(), None)]
    #[case::pointer_shape("22;xterm".into(), None)]
    #[case::foreground_color("10;?".into(), None)]
    #[case::osc_133_start("133;C".into(), None)]
    #[case::osc_133_end(format!("133;D;0;history_id={HID}"), None)]
    #[case::missing_history_id("18188735;C".into(), None)]
    #[case::empty_history_id("18188735;D;".into(), None)]
    #[case::invalid_history_id("18188735;D;not-a-uuid".into(), None)]
    #[case::trailing_param(format!("18188735;D;{HID};extra"), None)]
    #[case::unknown_kind(format!("18188735;A;{HID}"), None)]
    #[case::kind_too_long(format!("18188735;CD;{HID}"), None)]
    #[case::missing_kind(format!("18188735;;{HID}"), None)]
    #[case::lowercase_kind(format!("18188735;c;{HID}"), None)]
    #[case::wrong_osc_number(format!("1818873;C;{HID}"), None)]
    #[case::longer_osc_number(format!("181887350;C;{HID}"), None)]
    #[case::bare_osc_number("18188735".into(), None)]
    #[case::empty("".into(), None)]
    fn reads_what_a_command_says(#[case] text: String, #[case] expected: Option<Event>) {
        let params: Vec<&[u8]> = text.split(';').map(str::as_bytes).collect();
        assert_eq!(Event::parse(&params), expected);
    }

    // -- Multiple events / interleaved text in one read -----------------------

    #[rstest]
    fn emits_events_in_order_around_other_output() {
        let data = [
            b"$ ls\r\n".as_slice(),
            &start(HID_ONE),
            b"\x1b]0;title\x07file\r\n",
            &end(HID_ONE),
            b"\x1b[32m$\x1b[0m ",
            &start(HID_TWO),
            &end(HID_TWO),
        ]
        .concat();

        assert_eq!(parse_events(&data), vec![
            Event::OutputStart(hid(HID_ONE)),
            Event::Title,
            Event::OutputEnd(hid(HID_ONE)),
            Event::OutputStart(hid(HID_TWO)),
            Event::OutputEnd(hid(HID_TWO)),
        ]);
    }

    #[rstest]
    fn a_command_ended_by_the_next_one_reports_both() {
        let data = [b"\x1b]0;title".as_slice(), &start(HID)].concat();
        assert_eq!(parse_events(&data), vec![Event::Title, Event::OutputStart(hid(HID))]);
    }

    // -- Chunk data -----------------------------------------------------------

    #[rstest]
    fn splits_data_after_each_marker(
        #[values(b"\x07".as_slice(), b"\x1b\\".as_slice())] terminator: &[u8],
    ) {
        // A chunk that stopped one byte short would leave the marker looking unterminated to
        // whoever replays the data, and drop its final byte into the next chunk.
        let opening = [format!("\x1b]18188735;C;{HID}").as_bytes(), terminator].concat();
        let closing = [format!("\x1b]18188735;D;{HID}").as_bytes(), terminator].concat();
        let data = [b"before".as_slice(), &opening, b"between", &closing, b"after"].concat();

        let result = parse(&data);
        assert_eq!(result.chunks, vec![
            OwnedChunk {
                event: Event::OutputStart(hid(HID)),
                data: [b"before".as_slice(), &opening].concat(),
                osc_len: opening.len(),
            },
            OwnedChunk {
                event: Event::OutputEnd(hid(HID)),
                data: [b"between".as_slice(), &closing].concat(),
                osc_len: closing.len(),
            },
        ]);
        assert_eq!(result.trailing_data, b"after");
    }

    #[rstest]
    fn a_string_terminator_split_across_reads_stays_with_its_marker() {
        let mut parser = Parser::new();

        let first = read(&mut parser, format!("out\x1b]18188735;D;{HID}\x1b").as_bytes());
        assert!(first.chunks.is_empty());
        assert!(first.ends_mid_escape_code_or_char);

        let second = read(&mut parser, b"\\rest");
        assert_eq!(second.events(), vec![Event::OutputEnd(hid(HID))]);
        assert_eq!(second.chunks[0].data, b"\\");
        assert_eq!(second.trailing_data, b"rest");
    }

    #[rstest]
    fn a_backslash_after_a_cut_off_marker_is_output(#[values(b'\x18', b'\x1a')] cut_off_by: u8) {
        let marker = [format!("\x1b]18188735;D;{HID}").as_bytes(), &[cut_off_by]].concat();
        let mut parser = Parser::new();

        let first = read(&mut parser, &marker);
        assert_eq!(first.chunks[0].data, marker);

        let second = read(&mut parser, b"\\rest");
        assert!(second.chunks.is_empty());
        assert_eq!(second.trailing_data, b"\\rest");
    }

    // -- Properties -----------------------------------------------------------

    /// Bytes drawn mostly from the characters escape codes are built of, so that generated input is
    /// dense with escape codes that are complete, partial, or nearly markers.
    fn stream() -> impl Strategy<Value = Vec<u8>> {
        let piece = prop_oneof![
            Just(start(HID)),
            Just(end(HID_ONE)),
            Just(b"\x1b]18188735;".to_vec()),
            Just(b"\x1b]0;title\x07".to_vec()),
            Just(b"\x1b]2;".to_vec()),
            Just(b"\x1b]7;".to_vec()),
            Just(b"\x1b[1;31m".to_vec()),
            Just(b"\x1b[1;3".to_vec()),
            Just(b"\x1bP1$r".to_vec()),
            Just(b"\x1b_".to_vec()),
            Just(b"\x1b(".to_vec()),
            Just(b"\x1b".to_vec()),
            Just(b"\x07".to_vec()),
            Just(b"\x1b\\".to_vec()),
            Just(b"\\".to_vec()),
            Just(b"\x18".to_vec()),
            Just(b"\x1a".to_vec()),
            Just(vec![0x9C]),
            Just("é€🦀".as_bytes().to_vec()),
            Just(vec![0xE2]),
            Just(vec![0x82]),
            prop::collection::vec(any::<u8>(), 0..8),
        ];
        prop::collection::vec(piece, 0..16).prop_map(|pieces| pieces.concat())
    }

    /// What reading `data` split at the given points made of it: every event, every byte handed
    /// back, and whether it ended partway through an escape code or a character.
    fn read_split(data: &[u8], splits: &[prop::sample::Index]) -> (Vec<Event>, Vec<u8>, bool) {
        let mut cuts: Vec<usize> = splits.iter().map(|index| index.index(data.len() + 1)).collect();
        cuts.push(0);
        cuts.push(data.len());
        cuts.sort_unstable();

        let mut parser = Parser::new();
        let mut events = Vec::new();
        let mut rebuilt = Vec::new();
        let mut ends_mid_escape_code_or_char = false;
        for window in cuts.windows(2) {
            let result = read(&mut parser, &data[window[0]..window[1]]);
            events.extend(result.events());
            for chunk in result.chunks {
                rebuilt.extend(chunk.data);
            }
            rebuilt.extend(result.trailing_data);
            ends_mid_escape_code_or_char = result.ends_mid_escape_code_or_char;
        }
        (events, rebuilt, ends_mid_escape_code_or_char)
    }

    proptest! {
        /// Whatever the parser makes of the stream, every byte handed to it has to come back out:
        /// the caller passes this data straight on to a terminal.
        #[rstest]
        fn chunks_and_trailing_data_reconstruct_the_input(
            data in stream(),
            splits in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
        ) {
            let (_, rebuilt, _) = read_split(&data, &splits);
            prop_assert_eq!(rebuilt, data);
        }

        /// Output reaches the proxy in whatever pieces the pty hands over, so where it is cut must
        /// make no difference to what the parser makes of it.
        #[rstest]
        fn reads_output_the_same_however_it_is_cut(
            data in stream(),
            splits in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
        ) {
            let (events, _, ends_mid_escape_code_or_char) = read_split(&data, &splits);
            let whole = parse(&data);
            prop_assert_eq!(
                (events, ends_mid_escape_code_or_char),
                (whole.events(), whole.ends_mid_escape_code_or_char)
            );
        }
    }
}
