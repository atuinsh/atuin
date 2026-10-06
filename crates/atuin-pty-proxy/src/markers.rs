//! Streaming parser for Atuin's command output markers.
//!
//! Command output is delineated by the following markers:
//!
//! | Marker                               | Meaning                 |
//! |--------------------------------------|-------------------------|
//! | `OSC 18188735 ; C ; <history id> ST` | Start of command output |
//! | `OSC 18188735 ; D ; <history id> ST` | End of command output   |
//!
//! `OSC` is `ESC ]`. `ST` is `BEL` (0x07), `ESC \` (0x1B 0x5C), or `C1 ST` (0x9C).
//!
//! `OSC 18188735` is a private, Atuin-specific escape sequence. We use this to ensure that our
//! escape sequences don't conflict with any other program or terminal. 18188735 is `atuin`
//! interpreted as a base-36 number. It fits into a 32-bit signed integer, which many terminals use
//! for OSC numbers, and if truncated to a 16-bit or 8-bit integer, it produces results (35263, 191)
//! that also aren't in use. Additionally, no prefix of 18188735 of length 3 or greater is in use
//! either, in case a terminal only reads a fixed number of digits.

use atuin_client::history::HistoryId;

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const C1_ST: u8 = 0x9C;
const BACKSLASH: u8 = b'\\';
const RIGHT_BRACKET: u8 = b']';

/// The prefix that all Atuin OSC sequences start with, excluding the OSC itself.
const MARKER_PREFIX: &[u8] = b"18188735;";

/// Maximum number of bytes we'll buffer for the OSC parameter string.
///
/// This is large enough to hold `18188735;C;` (11 bytes) plus the history ID in the longest format
/// supported by the `uuid` crate (45 bytes for `urn:uuid:<hyphenated-uuid>`) -- 56 bytes total.
const MAX_PARAMS_SIZE: usize = 64;

/// Events emitted when a marker is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `OSC 18188735 ; C ; <history id> ST` -- start of command output.
    OutputStart(HistoryId),
    /// `OSC 18188735 ; D ; <history id> ST` -- end of command output.
    OutputEnd(HistoryId),
}

/// A marker event with the slice of data up to the end of the marker.
///
/// Concatenating [`Self::data`] for every chunk, followed by [`EventChunks::trailing_data`],
/// exactly reproduces the bytes passed to [`Parser::push`].
#[derive(Debug, Clone, Copy)]
pub struct EventChunk<'a> {
    /// The marker event.
    pub event: Event,

    /// All the data between the last event and the end of this event's marker.
    ///
    /// This includes the entire marker itself.
    pub data: &'a [u8],

    /// The total length of the marker corresponding to this event.
    pub osc_len: usize,
}

/// An iterator of marker events, created by [`Parser::push`].
///
/// After exhausting the iterator, you will likely want to call [`Self::trailing_data`] to get the
/// last chunk of data that was not yielded by the iterator. You may need to use
/// [`Iterator::by_ref`] when iterating to ensure you still have access to the iterator afterward.
pub struct EventChunks<'parser, 'data> {
    parser: &'parser mut Parser,
    data: &'data [u8],
    exhausted: bool,
}

impl<'data> Iterator for EventChunks<'_, 'data> {
    type Item = EventChunk<'data>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        let mut i = 0;
        while i < self.data.len() {
            // This is a tiny optimization. The only way we can get out of the [`State::Ground`] is
            // by seeing an `ESC` character. For proof, see [`Self::handle_byte`].
            //
            // In order to speed up this parsing, we use a memchr call. Surprisingly, it's ~10X
            // improvement, so it is really worth doing.
            if self.parser.state == State::Ground {
                match memchr::memchr(ESC, &self.data[i..]) {
                    Some(rel) => i += rel,
                    None => break,
                }
            }
            let b = self.data[i];
            if let Some(item) = self.handle_byte(b, i) {
                return Some(item);
            }
            i += 1;
        }
        self.exhausted = true;
        None
    }
}

impl std::iter::FusedIterator for EventChunks<'_, '_> {}

impl<'data> EventChunks<'_, 'data> {
    /// Get the last bit of data not yielded by this iterator.
    ///
    /// This method is intended to be called once the iterator has been exhausted.
    pub fn trailing_data(&self) -> &'data [u8] {
        self.data
    }

    fn handle_byte(&mut self, byte: u8, offset: usize) -> Option<EventChunk<'data>> {
        match self.parser.state {
            State::Ground => {
                // Warning: If you change the lgoic here, make sure you visit
                // `<EventChunks as Iterator>::next` and edit the logic there too.
                //
                // There's a nice comment there to guide you! Make sure you look at it!
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
    /// Returns an [`EventChunk`] if this was a marker; otherwise, returns [`None`], and the bytes
    /// will get included as normal data in the next [`EventChunk`] (or in
    /// [`EventChunks::trailing_data`]).
    fn end_osc(&mut self, offset: usize, terminator_len: usize) -> Option<EventChunk<'data>> {
        self.parser.state = State::Ground;

        let event = parse_marker(self.parser.param_bytes())?;

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

/// Try to parse an OSC sequence's parameters as an Atuin marker.
fn parse_marker(params: &[u8]) -> Option<Event> {
    let [kind, b';', rest @ ..] = params.strip_prefix(MARKER_PREFIX)? else {
        return None;
    };

    let rest_as_history_id = || std::str::from_utf8(rest).ok()?.parse::<HistoryId>().ok();

    match kind {
        b'C' => Some(Event::OutputStart(rest_as_history_id()?)),
        b'D' => Some(Event::OutputEnd(rest_as_history_id()?)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum State {
    /// Normal pass-through.
    Ground,
    /// Saw ESC (0x1B).
    Esc,
    /// Inside an OSC sequence (`ESC ]`), accumulating parameter bytes.
    OscParam,
    /// Inside an OSC sequence, saw ESC. Next byte decides if this is `ESC \` (string terminator) or
    /// something else.
    OscEsc,
}

/// A streaming, zero-allocation parser for Atuin's command output markers.
///
/// Feed arbitrary byte slices into [`Parser::push`]. The parser detects markers and returns an
/// iterator of [`EventChunk`]s, each containing an [`Event`] and the data up to that point.
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
    /// Create a new parser in the initial (ground) state.
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            param_buf: [0; MAX_PARAMS_SIZE],
            param_buf_len: 0,
        }
    }

    /// Process a chunk of bytes, yielding an [`EventChunk`] for every Atuin OSC marker found,
    /// containing the event and all the data up to that point.
    pub fn push<'data>(&mut self, data: &'data [u8]) -> EventChunks<'_, 'data> {
        EventChunks {
            parser: self,
            data,
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

#[cfg(test)]
mod tests {
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
        let mut iter = parser.push(data);
        let chunks = iter
            .by_ref()
            .map(|chunk| OwnedChunk {
                event: chunk.event,
                data: chunk.data.to_vec(),
                osc_len: chunk.osc_len,
            })
            .collect();
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
    #[case::start("C", Event::OutputStart(hid(HID)))]
    #[case::end("D", Event::OutputEnd(hid(HID)))]
    fn detects_event(
        #[case] kind: &str,
        #[case] expected: Event,
        #[values(b"\x07".as_slice(), b"\x1b\\".as_slice(), [C1_ST].as_slice())] terminator: &[u8],
    ) {
        let data = [format!("\x1b]18188735;{kind};{HID}").as_bytes(), terminator].concat();
        assert_eq!(parse_events(&data), vec![expected]);
    }

    #[rstest]
    fn accepts_the_simple_history_id_encoding() {
        // The shell integration sends whatever `atuin history start` printed, which is the simple
        // (hyphen-less) form rather than the hyphenated one the other tests use.
        let data = b"\x1b]18188735;D;000000000000000000000000000000a1\x07";
        assert_eq!(parse_events(data), vec![Event::OutputEnd(hid(HID))]);
    }

    #[rstest]
    fn reports_the_length_of_the_marker() {
        let marker = end(HID);
        let result = parse(&[b"output".as_slice(), &marker].concat());

        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.chunks[0].osc_len, marker.len());
    }

    // -- Multiple events / interleaved text in one push -----------------------

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
            Event::OutputEnd(hid(HID_ONE)),
            Event::OutputStart(hid(HID_TWO)),
            Event::OutputEnd(hid(HID_TWO)),
        ]);
    }

    #[rstest]
    fn an_unterminated_osc_does_not_hide_the_next_marker() {
        let data = [b"\x1b]0;title".as_slice(), &start(HID)].concat();
        assert_eq!(parse_events(&data), vec![Event::OutputStart(hid(HID))]);
    }

    // -- Chunk data -----------------------------------------------------------

    #[rstest]
    fn splits_data_at_the_markers() {
        let data = [b"before".as_slice(), &start(HID), b"between", &end(HID), b"after"].concat();

        let result = parse(&data);
        let chunks: Vec<_> = result.chunks.iter().map(|chunk| chunk.data.clone()).collect();
        assert_eq!(chunks, vec![
            [b"before".as_slice(), &start(HID)].concat(),
            [b"between".as_slice(), &end(HID)].concat(),
        ]);
        assert_eq!(result.trailing_data, b"after");
    }

    #[rstest]
    // The parser has to return to the ground state once a sequence is terminated. Otherwise
    // plain output keeps accumulating in the parameter buffer, and a bare BEL in that output
    // fabricates an event out of it.
    #[case::output_after_a_foreign_osc(
        [b"\x1b]0;\x07".as_slice(), format!("18188735;D;{HID}\x07").as_bytes()].concat()
    )]
    #[case::output_after_a_marker(
        [start(HID).as_slice(), format!("18188735;D;{HID}\x07").as_bytes()].concat()
    )]
    fn output_after_a_sequence_is_not_parsed_as_parameters(#[case] data: Vec<u8>) {
        let events = parse_events(&data);
        assert!(!events.contains(&Event::OutputEnd(hid(HID))), "{events:?}");
    }

    #[rstest]
    #[case::bel(b"\x07".as_slice())]
    #[case::st(b"\x1b\\")]
    #[case::c1_st(&[C1_ST])]
    fn chunk_data_includes_the_terminator(#[case] terminator: &[u8]) {
        // A chunk that stopped one byte short would leave the marker looking unterminated
        // to whoever replays the data, and drop its final byte into the next chunk.
        let marker = [format!("\x1b]18188735;D;{HID}").as_bytes(), terminator].concat();
        let data = [b"out".as_slice(), &marker, b"rest"].concat();

        let result = parse(&data);
        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.chunks[0].data, [b"out".as_slice(), &marker].concat());
        assert_eq!(result.trailing_data, b"rest");
    }

    #[rstest]
    fn a_marker_split_across_pushes_is_reported_by_the_completing_push() {
        let (head, tail) = HID.split_at(20);
        let mut parser = Parser::new();

        let first = push(&mut parser, format!("out\x1b]18188735;D;{head}").as_bytes());
        assert!(first.chunks.is_empty());
        assert_eq!(first.trailing_data, format!("out\x1b]18188735;D;{head}").as_bytes());

        let second = push(&mut parser, format!("{tail}\x07rest").as_bytes());
        assert_eq!(second.events(), vec![Event::OutputEnd(hid(HID))]);
        assert_eq!(second.chunks[0].data, format!("{tail}\x07").as_bytes());
        assert_eq!(second.trailing_data, b"rest");
    }

    // -- Input that must not produce events -----------------------------------

    #[rstest]
    #[case::osc_7(b"\x1b]7;file:///home/user\x07".to_vec())]
    #[case::osc_133_start(b"\x1b]133;C\x07".to_vec())]
    #[case::osc_133_end(format!("\x1b]133;D;0;history_id={HID}\x07").into_bytes())]
    #[case::missing_history_id(b"\x1b]18188735;C\x07".to_vec())]
    #[case::empty_history_id(b"\x1b]18188735;D;\x07".to_vec())]
    #[case::invalid_history_id(b"\x1b]18188735;D;not-a-uuid\x07".to_vec())]
    #[case::trailing_param(format!("\x1b]18188735;D;{HID};extra\x07").into_bytes())]
    #[case::unknown_kind(format!("\x1b]18188735;A;{HID}\x07").into_bytes())]
    #[case::kind_too_long(format!("\x1b]18188735;CD;{HID}\x07").into_bytes())]
    #[case::missing_kind(format!("\x1b]18188735;;{HID}\x07").into_bytes())]
    #[case::lowercase_kind(format!("\x1b]18188735;c;{HID}\x07").into_bytes())]
    #[case::wrong_osc_number(format!("\x1b]1818873;C;{HID}\x07").into_bytes())]
    #[case::longer_osc_number(format!("\x1b]181887350;C;{HID}\x07").into_bytes())]
    #[case::bare_osc_number(b"\x1b]18188735\x07".to_vec())]
    #[case::empty_osc(b"\x1b]\x07".to_vec())]
    #[case::unterminated(format!("\x1b]18188735;C;{HID}").into_bytes())]
    #[case::only_normal_text(b"just some regular terminal output\r\n".to_vec())]
    #[case::csi_only(b"\x1b[2J\x1b[H".to_vec())]
    fn ignores_input(#[case] data: Vec<u8>) {
        assert_eq!(parse_events(&data), vec![]);
    }

    // -- Buffer overflow (very long OSC) --------------------------------------

    #[rstest]
    // An OSC whose parameters overflow the buffer is dropped, without panicking...
    #[case::overlong_osc_is_dropped(b"\x1b]".to_vec(), vec![], vec![])]
    #[case::overlong_marker_is_dropped(
        format!("\x1b]18188735;D;{HID}").into_bytes(),
        vec![],
        vec![],
    )]
    // ...and the parser is back in a state where it recognises the next marker.
    #[case::parser_recovers(b"\x1b]0;".to_vec(), start(HID), vec![Event::OutputStart(hid(HID))])]
    fn an_overlong_osc_does_not_panic(
        #[case] prefix: Vec<u8>,
        #[case] suffix: Vec<u8>,
        #[case] expected: Vec<Event>,
    ) {
        let mut data = prefix;
        data.extend(std::iter::repeat_n(b'x', MAX_PARAMS_SIZE * 2));
        data.push(BEL);
        data.extend_from_slice(&suffix);

        assert_eq!(parse_events(&data), expected);
    }

    // -- Fused ----------------------------------------------------------------

    #[rstest]
    fn iterator_is_fused() {
        let data = [start(HID).as_slice(), b"tail"].concat();
        let mut parser = Parser::new();
        let mut iter = parser.push(&data);

        assert!(iter.next().is_some());
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
        assert_eq!(iter.trailing_data(), b"tail");
    }

    #[rstest]
    fn parser_default_matches_new() {
        assert_eq!(push(&mut Parser::default(), &start(HID)).events(), vec![Event::OutputStart(
            hid(HID)
        )]);
    }

    // -- Properties -----------------------------------------------------------

    /// Bytes drawn mostly from the characters markers are built of, so that generated input is
    /// dense with sequences that are complete, partial, or nearly markers.
    fn stream() -> impl Strategy<Value = Vec<u8>> {
        let piece = prop_oneof![
            Just(start(HID)),
            Just(end(HID_ONE)),
            Just(b"\x1b]18188735;".to_vec()),
            Just(b"\x1b]0;title".to_vec()),
            Just(b"\x1b".to_vec()),
            Just(b"\x07".to_vec()),
            Just(b"\x1b\\".to_vec()),
            Just(vec![C1_ST]),
            prop::collection::vec(any::<u8>(), 0..8),
        ];
        prop::collection::vec(piece, 0..16).prop_map(|pieces| pieces.concat())
    }

    /// Push `data` split at the given points, collecting every event and every byte handed back.
    fn push_split(data: &[u8], splits: &[prop::sample::Index]) -> (Vec<Event>, Vec<u8>) {
        let mut cuts: Vec<usize> = splits.iter().map(|index| index.index(data.len() + 1)).collect();
        cuts.push(0);
        cuts.push(data.len());
        cuts.sort_unstable();

        let mut parser = Parser::new();
        let mut events = Vec::new();
        let mut rebuilt = Vec::new();
        for window in cuts.windows(2) {
            let result = push(&mut parser, &data[window[0]..window[1]]);
            events.extend(result.events());
            for chunk in result.chunks {
                rebuilt.extend(chunk.data);
            }
            rebuilt.extend(result.trailing_data);
        }
        (events, rebuilt)
    }

    proptest! {
        /// Whatever the parser makes of the stream, every byte handed to it has to come back out:
        /// the caller passes this data straight on to a terminal.
        #[rstest]
        fn chunks_and_trailing_data_reconstruct_the_input(
            data in stream(),
            splits in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
        ) {
            let (_, rebuilt) = push_split(&data, &splits);
            prop_assert_eq!(rebuilt, data);
        }

        #[rstest]
        fn events_do_not_depend_on_how_the_stream_is_chunked(
            data in stream(),
            splits in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
        ) {
            let (events, _) = push_split(&data, &splits);
            prop_assert_eq!(events, parse_events(&data));
        }
    }
}
