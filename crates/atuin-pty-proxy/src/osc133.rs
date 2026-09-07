//! Streaming parser for OSC 133 (FinalTerm semantic prompt) escape sequences.
//!
//! OSC 133 marks four regions of a shell interaction:
//!
//! | Marker  | Meaning                               |
//! |---------|---------------------------------------|
//! | `A`     | Prompt is about to be printed         |
//! | `B`     | Prompt ended — command input begins   |
//! | `C`     | Command submitted — output begins     |
//! | `D[;n]` | Command finished with exit code *n*   |
//!
//! The wire format is `ESC ] 133 ; <cmd> [; <params>] ST` where `ST` is `BEL`
//! (0x07), `ESC \` (0x1B 0x5C), or `C1 ST` (0x9C).

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const C1_ST: u8 = 0x9C;
const BACKSLASH: u8 = b'\\';
const RIGHT_BRACKET: u8 = b']';

/// Maximum bytes we'll buffer for the OSC parameter string. This is large enough
/// for Atuin metadata such as history/session IDs while still bounding malformed
/// OSC sequences.
const MAX_PARAMS_SIZE: usize = 512;

/// Events emitted when an OSC 133 marker is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `ESC ] 133 ; A ST` — the shell is about to display its prompt.
    PromptStart,
    /// `ESC ] 133 ; B ST` — the prompt has ended; the user may type a command.
    CommandStart,
    /// `ESC ] 133 ; C ST` — the command has been submitted for execution.
    CommandExecuted,
    /// `ESC ] 133 ; D [; <exit_code>] ST` — command output is complete.
    CommandFinished {
        /// The exit code reported after the `;`, if present and valid.
        exit_code: Option<i32>,
    },
}

/// The current semantic zone as determined by the most recent OSC 133 marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// No marker seen yet, or after a `D` marker (between commands).
    #[default]
    Unknown,
    /// Between `A` and `B` — the shell is rendering its prompt.
    Prompt,
    /// Between `B` and `C` — the user is editing a command line.
    Input,
    /// Between `C` and `D` — command output is being produced.
    Output,
}

impl Event {
    /// Get the zone corresponding to the data that comes after this event.
    pub fn zone(self) -> Zone {
        match self {
            Self::PromptStart => Zone::Prompt,
            Self::CommandStart => Zone::Input,
            Self::CommandExecuted => Zone::Output,
            Self::CommandFinished { .. } => Zone::Unknown,
        }
    }
}

/// An OSC 133 event with the slice of data up to the end of the OSC sequence.
///
/// Concatenating [`Self::data`] for every chunk, followed by [`EventChunks::trailing_data`],
/// exactly reproduces the bytes passed to [`Parser::push`].
#[derive(Debug, Clone, Copy)]
pub struct EventChunk<'a> {
    /// The OSC 133 event.
    pub event: Event,

    /// All the data between the last event and the end of this event's OSC 133 sequence.
    ///
    /// This includes the entire OSC 133 sequence itself.
    pub data: &'a [u8],

    /// The total length of the OSC 133 sequence corresponding to this event.
    pub osc_len: usize,
}

/// A single OSC 133 marker parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Param<'a> {
    Value(&'a [u8]),
    KeyValue {
        key: &'a [u8],
        value: &'a [u8],
    },
}

/// An iterator of OSC 133 events, created by [`Parser::push`].
///
/// After exhausting the iterator, you will likely want to call [`Self::trailing_data`] to get the
/// last chunk of data that was not yielded by the iterator. You may need to use
/// [`Iterator::by_ref`] when iterating to ensure you still have access to the iterator afterward.
pub struct EventChunks<'parser, 'data> {
    parser: &'parser mut Parser,
    data: &'data [u8],
    /// Index within `parser.param_buf` where unhandled parameters start.
    params_start: usize,
    exhausted: bool,
}

impl<'data> Iterator for EventChunks<'_, 'data> {
    type Item = EventChunk<'data>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        for (i, b) in self.data.iter().copied().enumerate() {
            if let Some(item) = self.handle_byte(b, i) {
                return Some(item);
            }
        }
        self.exhausted = true;
        None
    }
}

impl std::iter::FusedIterator for EventChunks<'_, '_> {}

impl<'data> EventChunks<'_, 'data> {
    /// Get the OSC 133 params for the most recently yielded event.
    pub fn params(&self) -> impl Iterator<Item = Param<'_>> {
        let bytes = if self.exhausted {
            b""
        } else {
            &self.parser.param_bytes()[self.params_start..]
        };
        // Make the split iterator conditional on `bytes` being non-empty; otherwise it will yield
        // an empty param when `bytes` is empty.
        let split = (!bytes.is_empty()).then(|| bytes.split(|b| *b == b';'));
        split.into_iter().flatten().map(|bytes| {
            let Some(sep) = bytes.iter().position(|b| *b == b'=') else {
                return Param::Value(bytes);
            };
            Param::KeyValue {
                key: &bytes[..sep],
                value: &bytes[sep + 1..],
            }
        })
    }

    /// Get the last bit of data not yielded by this iterator.
    ///
    /// This method is intended to be called once the iterator has been exhausted.
    pub fn trailing_data(&self) -> &'data [u8] {
        self.data
    }

    fn handle_byte(&mut self, byte: u8, offset: usize) -> Option<EventChunk<'data>> {
        match self.parser.state {
            State::Ground => {
                if byte == ESC {
                    self.parser.state = State::Esc;
                }
            }
            State::Esc => {
                self.handle_esc(byte);
            }
            State::OscParam => {
                if byte == BEL || byte == C1_ST {
                    let terminator_len = 1;
                    return self.end_osc(offset, terminator_len);
                } else if byte == ESC {
                    self.parser.state = State::OscEsc;
                } else if self.parser.append_param_byte(byte).is_err() {
                    self.parser.state = State::Ground;
                }
            }
            State::OscEsc => {
                if byte == BACKSLASH {
                    let terminator_len = 2; // ESC + BACKSLASH
                    return self.end_osc(offset, terminator_len);
                }
                // Fall back to handling this byte as if we had been in the regular `Esc` state.
                // If something spits out a malformed unterminated OSC sequence, we want the next
                // legitimate OSC sequence to reset us into the proper state. This accomplishes
                // that.
                self.handle_esc(byte);
            }
        }
        None
    }

    fn handle_esc(&mut self, byte: u8) {
        match byte {
            RIGHT_BRACKET => {
                self.parser.state = State::OscParam;
                self.parser.clear_param_bytes();
                self.params_start = 0;
            }
            ESC => {
                // Restart the escape sequence if we get another ESC.
                self.parser.state = State::Esc;
            }
            _ => {
                self.parser.state = State::Ground;
            }
        }
    }

    /// Finish the OSC sequence whose terminator ends at `offset`.
    ///
    /// Returns an [`EventChunk`] if this was an OSC 133 sequence; otherwise, returns [`None`],
    /// and the bytes will get included as normal data in the next [`EventChunk`] (or in
    /// [`EventChunks::trailing_data`]).
    fn end_osc(&mut self, offset: usize, terminator_len: usize) -> Option<EventChunk<'data>> {
        self.parser.state = State::Ground;

        let param_bytes = self.parser.param_bytes();
        let mut payload = param_bytes.strip_prefix(b"133")?;
        if *payload.split_off_first()? != b';' {
            return None;
        }

        let cmd = *payload.split_off_first()?;
        if payload.split_off_first().is_some_and(|b| *b != b';') {
            return None;
        }

        let event = match cmd {
            b'A' => Event::PromptStart,
            b'B' => Event::CommandStart,
            b'C' => Event::CommandExecuted,
            b'D' => {
                let mut exit_code = None;
                if let Some(bytes) = payload.split(|b| *b == b';').next()
                    && let Some(code) = parse_bytes(bytes)
                {
                    exit_code = Some(code);
                    payload = payload.get(bytes.len() + 1..).unwrap_or_default();
                }
                Event::CommandFinished { exit_code }
            }
            _ => return None,
        };

        self.params_start = param_bytes.len() - payload.len();
        let (data, rest) = self.data.split_at(offset + 1);
        self.data = rest;

        let non_params_len = 2 + terminator_len; // ESC + RIGHT_BRACKET + terminator_len
        let osc_len = non_params_len + self.parser.param_buf_len;
        Some(EventChunk {
            event,
            data,
            osc_len,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum State {
    /// Normal pass-through.
    Ground,
    /// Saw ESC (0x1B).
    Esc,
    /// Inside an OSC sequence (`ESC ]`), accumulating parameter bytes.
    OscParam,
    /// Inside an OSC sequence, saw ESC — next byte decides if this is `ESC \`
    /// (string terminator) or something else.
    OscEsc,
}

/// A streaming, zero-allocation parser for OSC 133 escape sequences.
///
/// Feed arbitrary byte slices into [`Parser::push`]. The parser detects
/// OSC 133 markers and returns an iterator of [`EventChunk`]s, each containing
/// an [`Event`] and the data up to that point.
pub struct Parser {
    state: State,
    param_buf: [u8; MAX_PARAMS_SIZE],
    param_buf_len: usize,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    /// Create a new parser in the initial (ground / unknown-zone) state.
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            param_buf: [0; MAX_PARAMS_SIZE],
            param_buf_len: 0,
        }
    }

    /// Process a chunk of bytes, yielding an [`EventChunk`] for every OSC 133 marker
    /// found, containing the event type and all the data up to that point.
    pub fn push<'data>(&mut self, data: &'data [u8]) -> EventChunks<'_, 'data> {
        EventChunks {
            parser: self,
            data,
            params_start: 0,
            exhausted: false,
        }
    }

    fn param_bytes(&self) -> &[u8] {
        &self.param_buf[..self.param_buf_len]
    }

    fn append_param_byte(&mut self, byte: u8) -> Result<(), ()> {
        if self.param_buf_len >= self.param_buf.len() {
            return Err(());
        }
        self.param_buf[self.param_buf_len] = byte;
        self.param_buf_len += 1;
        Ok(())
    }

    fn clear_param_bytes(&mut self) {
        self.param_buf_len = 0;
    }
}

fn parse_bytes<T>(bytes: &[u8]) -> Option<T>
where
    T: std::str::FromStr,
{
    if bytes.is_empty() {
        return None;
    }
    std::str::from_utf8(bytes).ok().and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// An owned copy of [`Param`], so tests can hold onto parameters after the
    /// borrow of the parser ends.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum OwnedParam {
        Value(Vec<u8>),
        KeyValue {
            key: Vec<u8>,
            value: Vec<u8>,
        },
    }

    impl From<Param<'_>> for OwnedParam {
        fn from(param: Param<'_>) -> Self {
            match param {
                Param::Value(value) => Self::Value(value.to_vec()),
                Param::KeyValue { key, value } => Self::KeyValue {
                    key: key.to_vec(),
                    value: value.to_vec(),
                },
            }
        }
    }

    impl OwnedParam {
        fn value(value: &str) -> Self {
            Self::Value(value.as_bytes().to_vec())
        }

        fn key_value(key: &str, value: &str) -> Self {
            Self::KeyValue {
                key: key.as_bytes().to_vec(),
                value: value.as_bytes().to_vec(),
            }
        }
    }

    /// An owned copy of [`EventChunk`] plus the parameters that went with it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct OwnedChunk {
        event: Event,
        data: Vec<u8>,
        params: Vec<OwnedParam>,
    }

    /// The full result of one [`Parser::push`] call.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Push {
        chunks: Vec<OwnedChunk>,
        trailing_data: Vec<u8>,
    }

    impl Push {
        fn events(&self) -> Vec<Event> {
            self.chunks.iter().map(|chunk| chunk.event).collect()
        }
    }

    fn push(parser: &mut Parser, data: &[u8]) -> Push {
        let mut chunks = Vec::new();
        let mut iter = parser.push(data);
        while let Some(chunk) = iter.next() {
            chunks.push(OwnedChunk {
                event: chunk.event,
                data: chunk.data.to_vec(),
                params: iter.params().map(OwnedParam::from).collect(),
            });
        }
        Push {
            chunks,
            trailing_data: iter.trailing_data().to_vec(),
        }
    }

    /// Push `data` through a fresh parser in a single call.
    fn parse(data: &[u8]) -> Push {
        push(&mut Parser::new(), data)
    }

    /// Collect all events from a single `push` call.
    fn parse_events(data: &[u8]) -> Vec<Event> {
        parse(data).events()
    }

    // -- Basic event detection ------------------------------------------------

    #[rstest]
    #[case::prompt_start_bel(b"\x1b]133;A\x07", Event::PromptStart)]
    #[case::prompt_start_st(b"\x1b]133;A\x1b\\", Event::PromptStart)]
    #[case::command_start_bel(b"\x1b]133;B\x07", Event::CommandStart)]
    #[case::command_start_st(b"\x1b]133;B\x1b\\", Event::CommandStart)]
    #[case::command_executed_bel(b"\x1b]133;C\x07", Event::CommandExecuted)]
    #[case::command_executed_st(b"\x1b]133;C\x1b\\", Event::CommandExecuted)]
    #[case::finished_no_exit_code(b"\x1b]133;D\x07", Event::CommandFinished { exit_code: None })]
    #[case::finished_exit_zero(b"\x1b]133;D;0\x07", Event::CommandFinished { exit_code: Some(0) })]
    #[case::finished_exit_nonzero(b"\x1b]133;D;127\x07", Event::CommandFinished { exit_code: Some(127) })]
    #[case::finished_negative_exit_code(b"\x1b]133;D;-1\x07", Event::CommandFinished { exit_code: Some(-1) })]
    #[case::finished_exit_code_st(b"\x1b]133;D;42\x1b\\", Event::CommandFinished { exit_code: Some(42) })]
    #[case::invalid_exit_code_yields_none(b"\x1b]133;D;abc\x07", Event::CommandFinished { exit_code: None })]
    #[case::d_semicolon_empty_exit(b"\x1b]133;D;\x07", Event::CommandFinished { exit_code: None })]
    #[case::large_exit_code(b"\x1b]133;D;2147483647\x07", Event::CommandFinished { exit_code: Some(i32::MAX) })]
    #[case::overflow_exit_code(b"\x1b]133;D;9999999999999\x07", Event::CommandFinished { exit_code: None })]
    fn detects_event(#[case] data: &[u8], #[case] expected: Event) {
        assert_eq!(parse_events(data), vec![expected]);
    }

    // -- Zone mapping ---------------------------------------------------------

    #[rstest]
    #[case(Event::PromptStart, Zone::Prompt)]
    #[case(Event::CommandStart, Zone::Input)]
    #[case(Event::CommandExecuted, Zone::Output)]
    #[case(Event::CommandFinished { exit_code: Some(0) }, Zone::Unknown)]
    #[case(Event::CommandFinished { exit_code: None }, Zone::Unknown)]
    fn event_maps_to_zone(#[case] event: Event, #[case] expected: Zone) {
        assert_eq!(event.zone(), expected);
    }

    #[rstest]
    fn zone_default_is_unknown() {
        assert_eq!(Zone::default(), Zone::Unknown);
    }

    #[rstest]
    fn full_zone_cycle() {
        let mut parser = Parser::new();
        let mut zones = Vec::new();

        for data in [
            b"\x1b]133;A\x07".as_slice(),
            b"\x1b]133;B\x07",
            b"\x1b]133;C\x07",
            b"\x1b]133;D;0\x07",
        ] {
            zones.extend(push(&mut parser, data).events().into_iter().map(Event::zone));
        }

        assert_eq!(zones, vec![Zone::Prompt, Zone::Input, Zone::Output, Zone::Unknown]);
    }

    // -- Multiple events / interleaved text in one push -----------------------

    #[rstest]
    #[case::multiple_events(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\n\x1b]133;C\x07file.txt\n\x1b]133;D;0\x07", vec![Event::PromptStart, Event::CommandStart, Event::CommandExecuted, Event::CommandFinished { exit_code: Some(0) }])]
    #[case::mixed_terminators(b"\x1b]133;A\x07\x1b]133;B\x1b\\\x1b]133;C\x07\x1b]133;D;1\x1b\\", vec![Event::PromptStart, Event::CommandStart, Event::CommandExecuted, Event::CommandFinished { exit_code: Some(1) }])]
    #[case::normal_text_before_and_after(b"hello world\x1b]133;A\x07prompt text\x1b]133;B\x07command", vec![Event::PromptStart, Event::CommandStart])]
    #[case::non_133_osc_ignored(b"\x1b]0;window title\x07\x1b]133;A\x07", vec![Event::PromptStart])]
    #[case::esc_followed_by_non_bracket(b"\x1b[31m\x1b]133;A\x07", vec![Event::PromptStart])]
    #[case::detects_c1_st_terminator(b"\x1b]133;A\x9c", vec![Event::PromptStart])]
    #[case::back_to_back_osc_no_gap(b"\x1b]133;A\x07\x1b]133;B\x07", vec![Event::PromptStart, Event::CommandStart])]
    #[case::csi_sequences_ignored(b"\x1b[32m\x1b]133;A\x07\x1b[0m$ \x1b]133;B\x07", vec![Event::PromptStart, Event::CommandStart])]
    #[case::unterminated_osc_then_marker(b"\x1b]0;title\x1b]133;A\x07", vec![Event::PromptStart])]
    // Enter pressed on an empty prompt, twice.
    #[case::repeated_prompt_cycle(b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;D\x07\x1b]133;A\x07$ \x1b]133;B\x07", vec![Event::PromptStart, Event::CommandStart, Event::CommandFinished { exit_code: None }, Event::PromptStart, Event::CommandStart])]
    fn emits_events(#[case] data: &[u8], #[case] expected: Vec<Event>) {
        assert_eq!(parse_events(data), expected);
    }

    // -- Chunk data -----------------------------------------------------------

    #[rstest]
    #[case::marker_at_the_end(
        b"before\x1b]133;A\x07",
        vec![(Event::PromptStart, b"before\x1b]133;A\x07".as_slice())],
        b"",
    )]
    #[case::two_markers(
        b"before\x1b]133;A\x07between\x1b]133;B\x07after",
        vec![
            (Event::PromptStart, b"before\x1b]133;A\x07".as_slice()),
            (Event::CommandStart, b"between\x1b]133;B\x07".as_slice()),
        ],
        b"after",
    )]
    // A non-OSC-133 sequence is still terminal output, so it stays in the data we hand back.
    #[case::non_133_osc_stays_in_the_data(
        b"\x1b]0;title\x07x\x1b]133;A\x07",
        vec![(Event::PromptStart, b"\x1b]0;title\x07x\x1b]133;A\x07".as_slice())],
        b"",
    )]
    #[case::partial_marker_is_left_trailing(
        b"out\x1b]133;C\x07more\x1b]133;D;0",
        vec![(Event::CommandExecuted, b"out\x1b]133;C\x07".as_slice())],
        b"more\x1b]133;D;0",
    )]
    // The parser has to return to the ground state once a sequence is terminated. Otherwise
    // plain output keeps accumulating in the parameter buffer, and a bare BEL in that output
    // fabricates an event out of it.
    #[case::output_after_a_marker_is_not_parsed_as_parameters(
        b"\x1b]133;A\x07133;D;99\x07",
        vec![(Event::PromptStart, b"\x1b]133;A\x07".as_slice())],
        b"133;D;99\x07",
    )]
    // The same, but with output that would graft cleanly onto the parameters of the marker
    // before it: `133;A` + `;fake` parses as a prompt-start marker carrying a `fake` param.
    #[case::a_bel_in_output_cannot_extend_the_previous_marker(
        b"\x1b]133;A\x07;fake\x07more",
        vec![(Event::PromptStart, b"\x1b]133;A\x07".as_slice())],
        b";fake\x07more",
    )]
    // `ESC ] 133 ; A ESC` is a malformed, unterminated OSC. The second ESC starts a new
    // sequence rather than aborting into the ground state.
    #[case::unterminated_sequence_does_not_swallow_the_next_esc(
        b"\x1b]133;A\x1b\x1b]133;B\x07",
        vec![(Event::CommandStart, b"\x1b]133;A\x1b\x1b]133;B\x07".as_slice())],
        b"",
    )]
    fn splits_data_at_the_markers(
        #[case] data: &[u8],
        #[case] expected_chunks: Vec<(Event, &[u8])>,
        #[case] expected_trailing: &[u8],
    ) {
        let result = parse(data);
        let chunks: Vec<_> =
            result.chunks.iter().map(|chunk| (chunk.event, chunk.data.as_slice())).collect();

        assert_eq!(chunks, expected_chunks);
        assert_eq!(result.trailing_data, expected_trailing);
    }

    #[rstest]
    #[case::bel(b"\x1b]133;D;0\x07")]
    #[case::st(b"\x1b]133;D;0\x1b\\")]
    #[case::c1_st(b"\x1b]133;D;0\x9c")]
    fn chunk_data_includes_the_terminator(#[case] marker: &[u8]) {
        // A chunk that stopped one byte short would leave the marker looking unterminated
        // to whoever replays the data, and drop its final byte into the next chunk.
        let data = [b"out".as_slice(), marker, b"rest"].concat();

        let result = parse(&data);
        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.chunks[0].data, [b"out".as_slice(), marker].concat());
        assert_eq!(result.trailing_data, b"rest");
    }

    #[rstest]
    #[case::empty(b"")]
    #[case::no_markers(b"just some regular terminal output\r\n")]
    #[case::every_terminator_and_a_partial_marker(
        b"a\x1b]133;A\x07b\x1b]133;B\x1b\\c\x1b]0;title\x07d\x1b]133;D;0\x9ce\x1b]133;"
    )]
    #[case::back_to_back_markers(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D;0\x07")]
    #[case::trailing_lone_esc(b"out\x1b")]
    #[case::overlong_osc(b"\x1b]133;A\x07\x1b]0;\x07\x1b\x1b]133;B\x07tail")]
    fn chunks_and_trailing_data_reconstruct_the_input(#[case] data: &[u8]) {
        // Whatever the parser makes of the stream, every byte handed to it has to come back
        // out: the caller passes this data straight on to a terminal.
        let result = parse(data);

        let mut rebuilt = Vec::new();
        for chunk in &result.chunks {
            rebuilt.extend_from_slice(&chunk.data);
        }
        rebuilt.extend_from_slice(&result.trailing_data);

        assert_eq!(rebuilt, data);
    }

    #[rstest]
    fn a_marker_split_across_pushes_is_reported_by_the_completing_push() {
        let mut parser = Parser::new();

        let first = push(&mut parser, b"out\x1b]133;D");
        assert!(first.chunks.is_empty());
        assert_eq!(first.trailing_data, b"out\x1b]133;D");

        let second = push(&mut parser, b";0;history_id=x\x07rest");
        assert_eq!(second.chunks.len(), 1);
        assert_eq!(second.chunks[0].data, b";0;history_id=x\x07");
        assert_eq!(second.trailing_data, b"rest");
    }

    // -- Params ---------------------------------------------------------------

    #[rstest]
    #[case::key_values_and_a_bare_value(
        b"\x1b]133;D;127;history_id=018f;session_id=abcd;flag\x07",
        Event::CommandFinished { exit_code: Some(127) },
        vec![
            OwnedParam::key_value("history_id", "018f"),
            OwnedParam::key_value("session_id", "abcd"),
            OwnedParam::value("flag"),
        ],
    )]
    #[case::params_survive_a_missing_exit_code(
        b"\x1b]133;D;history_id=018f;session_id=abcd\x07",
        Event::CommandFinished { exit_code: None },
        vec![
            OwnedParam::key_value("history_id", "018f"),
            OwnedParam::key_value("session_id", "abcd"),
        ],
    )]
    #[case::unparsable_exit_code_is_kept_as_a_param(
        b"\x1b]133;D;abc;history_id=018f\x07",
        Event::CommandFinished { exit_code: None },
        vec![OwnedParam::value("abc"), OwnedParam::key_value("history_id", "018f")],
    )]
    // This is what `atuin init bash` emits.
    #[case::prompt_start_params(
        b"\x1b]133;A;cl=line\x07",
        Event::PromptStart,
        vec![OwnedParam::key_value("cl", "line")],
    )]
    #[case::no_params_prompt_start(b"\x1b]133;A\x07", Event::PromptStart, vec![])]
    #[case::no_params_command_start(b"\x1b]133;B\x07", Event::CommandStart, vec![])]
    #[case::no_params_command_executed(b"\x1b]133;C\x07", Event::CommandExecuted, vec![])]
    #[case::no_params_finished_bare(
        b"\x1b]133;D\x07",
        Event::CommandFinished { exit_code: None },
        vec![],
    )]
    #[case::no_params_finished_exit_code_only(
        b"\x1b]133;D;0\x07",
        Event::CommandFinished { exit_code: Some(0) },
        vec![],
    )]
    #[case::no_params_finished_trailing_semicolon(
        b"\x1b]133;D;\x07",
        Event::CommandFinished { exit_code: None },
        vec![],
    )]
    fn parses_params(
        #[case] data: &[u8],
        #[case] expected_event: Event,
        #[case] expected_params: Vec<OwnedParam>,
    ) {
        let result = parse(data);

        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.chunks[0].event, expected_event);
        assert_eq!(result.chunks[0].params, expected_params);
    }

    #[rstest]
    fn params_belong_to_the_most_recent_event() {
        let mut parser = Parser::new();
        let mut iter =
            parser.push(b"\x1b]133;D;0;history_id=one\x07\x1b]133;D;1;history_id=two\x07");

        assert!(iter.next().is_some());
        assert_eq!(iter.params().map(OwnedParam::from).collect::<Vec<_>>(), vec![
            OwnedParam::key_value("history_id", "one")
        ]);

        assert!(iter.next().is_some());
        assert_eq!(iter.params().map(OwnedParam::from).collect::<Vec<_>>(), vec![
            OwnedParam::key_value("history_id", "two")
        ]);
    }

    // -- Split across push boundaries -----------------------------------------

    #[rstest]
    #[case::esc_and_bracket(b"\x1b", b"]133;A\x07", vec![Event::PromptStart])]
    #[case::mid_param(b"\x1b]13", b"3;D;42\x07", vec![Event::CommandFinished { exit_code: Some(42) }])]
    #[case::before_terminator(b"\x1b]133;B", b"\x07", vec![Event::CommandStart])]
    #[case::esc_backslash_terminator(b"\x1b]133;C\x1b", b"\\", vec![Event::CommandExecuted])]
    #[case::lone_esc_aborted(b"\x1b", b"x\x1b]133;A\x07", vec![Event::PromptStart])]
    fn split_across_push_boundary(
        #[case] first: &[u8],
        #[case] second: &[u8],
        #[case] expected: Vec<Event>,
    ) {
        let mut parser = Parser::new();
        assert_eq!(push(&mut parser, first).events(), vec![]);
        assert_eq!(push(&mut parser, second).events(), expected);
    }

    #[rstest]
    fn params_split_across_push_boundary() {
        let mut parser = Parser::new();
        assert_eq!(push(&mut parser, b"\x1b]133;D;0;history_id=018f").events(), vec![]);

        let result = push(&mut parser, b";session_id=abcd\x07rest");
        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.chunks[0].params, vec![
            OwnedParam::key_value("history_id", "018f"),
            OwnedParam::key_value("session_id", "abcd"),
        ]);
        assert_eq!(result.trailing_data, b"rest");
    }

    #[rstest]
    fn events_do_not_depend_on_how_the_stream_is_chunked(
        #[values(1, 2, 3, 7, usize::MAX)] chunk_size: usize,
    ) {
        let data = b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07f\r\n\x1b]133;D;99\x07";
        let mut parser = Parser::new();
        let mut events = Vec::new();

        for chunk in data.chunks(chunk_size.min(data.len())) {
            events.extend(push(&mut parser, chunk).events());
        }

        assert_eq!(events, vec![
            Event::PromptStart,
            Event::CommandStart,
            Event::CommandExecuted,
            Event::CommandFinished {
                exit_code: Some(99)
            },
        ]);
    }

    // -- Input that must not produce events -----------------------------------

    #[rstest]
    #[case::osc_7(b"\x1b]7;file:///home/user\x07")]
    #[case::unknown_command_letter(b"\x1b]133;Z\x07")]
    #[case::marker_with_unexpected_trailing_bytes(b"\x1b]133;ABC\x07")]
    // "13" followed by terminator — not "133;" so no event.
    #[case::truncated_133_prefix(b"\x1b]13\x07")]
    #[case::wrong_osc_number(b"\x1b]1330;A\x07")]
    #[case::empty_osc(b"\x1b]\x07")]
    #[case::empty_input(b"")]
    #[case::only_normal_text(b"just some regular terminal output\r\n")]
    #[case::csi_only(b"\x1b[2J\x1b[H")]
    fn ignores_input(#[case] data: &[u8]) {
        assert!(parse_events(data).is_empty());
    }

    // -- Buffer overflow (very long non-133 OSC) ------------------------------

    #[rstest]
    // An OSC whose parameters overflow the buffer is dropped, without panicking...
    #[case::overlong_osc_is_dropped(b"\x1b]", b"", vec![])]
    #[case::overlong_133_osc_is_dropped(b"\x1b]133;D;0;", b"", vec![])]
    // ...and the parser is back in a state where it recognises the next marker.
    #[case::parser_recovers(b"\x1b]133;D;0;", b"\x1b]133;A\x07", vec![Event::PromptStart])]
    fn an_overlong_osc_does_not_panic(
        #[case] prefix: &[u8],
        #[case] suffix: &[u8],
        #[case] expected: Vec<Event>,
    ) {
        let mut data = prefix.to_vec();
        data.extend(std::iter::repeat_n(b'x', MAX_PARAMS_SIZE * 2));
        data.push(BEL);
        data.extend_from_slice(suffix);

        assert_eq!(parse_events(&data), expected);
    }

    // -- Repeated prompts (empty command) ------------------------------------

    // -- Fused ----------------------------------------------------------------

    #[rstest]
    fn params_are_empty_once_the_iterator_is_exhausted() {
        // Exhausting the iterator scans the trailing data, which can start a fresh OSC and
        // leave stale bytes in the parameter buffer. They belong to no yielded event.
        let mut parser = Parser::new();
        let mut iter = parser.push(b"\x1b]133;D;0;history_id=x\x07tail\x1b]0;title");

        assert!(iter.next().is_some());
        assert_eq!(iter.params().count(), 1, "the yielded event still has its params");

        assert!(iter.next().is_none());
        assert_eq!(iter.params().count(), 0);
    }

    #[rstest]
    fn iterator_is_fused() {
        let mut parser = Parser::new();
        let mut iter = parser.push(b"\x1b]133;A\x07tail");

        assert!(iter.next().is_some());
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
        assert_eq!(iter.trailing_data(), b"tail");
    }

    // -- Default trait --------------------------------------------------------

    #[rstest]
    fn parser_default_matches_new() {
        assert_eq!(push(&mut Parser::default(), b"\x1b]133;A\x07").events(), vec![
            Event::PromptStart
        ]);
    }
}
