//! Streaming parser for OSC 133 (FinalTerm semantic prompt) escape sequences.
//!
//! OSC 133 marks four regions of a shell interaction:
//!
//! | Marker | Meaning                              |
//! |--------|--------------------------------------|
//! | A      | Prompt is about to be printed        |
//! | B      | Prompt ended — command input begins   |
//! | C      | Command submitted — output begins     |
//! | D[;n]  | Command finished with exit code *n*   |
//!
//! The wire format is `ESC ] 133 ; <cmd> [; <params>] ST` where ST is BEL
//! (0x07), ESC \ (0x1B 0x5C), or C1 ST (0x9C).
//!
//! # Design goals
//!
//! * **Transparent** — the parser observes the byte stream without modifying it;
//!   the caller remains responsible for forwarding bytes to their destination.
//! * **Bounded** — OSC parameter buffering is capped so malformed output cannot
//!   grow memory without limit.
//! * **Non-blocking** — [`Parser::push`] processes whatever bytes are available
//!   and returns immediately.
//! * **Extensible** — marker parameters are preserved so Atuin-specific metadata
//!   can ride alongside standard OSC 133 markers.

/// Events emitted when an OSC 133 marker is detected.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Parameters attached to an OSC 133 marker.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Params {
    items: Vec<Param>,
}

impl Params {
    /// Iterate over all marker parameters in order.
    #[cfg(test)]
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &Param> {
        self.items.iter()
    }

    /// Return the value for the first `key=value` parameter with this key.
    #[inline]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.items.iter().find_map(|item| match item {
            Param::KeyValue {
                key: item_key,
                value,
            } if item_key == key => Some(value.as_str()),
            Param::Value(_) | Param::KeyValue { .. } => None,
        })
    }
}

/// A single OSC 133 marker parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Param {
    /// A positional parameter without an equals sign.
    Value(String),
    /// A `key=value` parameter.
    KeyValue { key: String, value: String },
}

/// An OSC 133 event with its position in the most recent input chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedEvent {
    /// The OSC 133 event that was parsed.
    pub event: Event,
    /// Offset where this marker starts in the current chunk.
    ///
    /// If a marker started in an earlier [`Parser::push_located`] call, this is
    /// `0` in the chunk that completed the marker.
    pub start_offset: usize,
    /// Offset immediately after this marker's terminator in the current chunk.
    ///
    /// If a marker spans multiple [`Parser::push_located`] calls, this is still
    /// the offset in the chunk that completed the marker.
    pub offset: usize,
    /// The semantic zone after applying this event.
    pub zone: Zone,
    /// Metadata parameters attached to this marker.
    pub params: Params,
}

/// The current semantic zone as determined by the most recent OSC 133 marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
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

// ---------------------------------------------------------------------------
// Internal constants
// ---------------------------------------------------------------------------

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const C1_ST: u8 = 0x9C;
const BACKSLASH: u8 = b'\\';
const RIGHT_BRACKET: u8 = b']';

/// Maximum bytes we'll buffer for the OSC parameter string. This is large enough
/// for Atuin metadata such as history/session IDs while still bounding malformed
/// OSC sequences.
const PARAM_BUF_CAP: usize = 512;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
/// Feed arbitrary byte slices into [`Parser::push`].  The parser detects
/// OSC 133 markers and reports [`Event`]s through a caller-supplied callback
/// without modifying the data.  It can sit transparently between a PTY reader
/// and stdout.
pub struct Parser {
    state: State,
    zone: Zone,
    sequence_start: Option<usize>,
    param_buf: [u8; PARAM_BUF_CAP],
    param_len: usize,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    /// Create a new parser in the initial (ground / unknown-zone) state.
    #[inline]
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            zone: Zone::Unknown,
            sequence_start: None,
            param_buf: [0u8; PARAM_BUF_CAP],
            param_len: 0,
        }
    }

    /// The current semantic zone based on markers seen so far.
    #[inline]
    #[allow(dead_code)]
    pub fn zone(&self) -> Zone {
        self.zone
    }

    /// Start offset of an incomplete OSC sequence in the most recent chunk.
    #[inline]
    pub(crate) fn incomplete_osc_sequence_start(&self) -> Option<usize> {
        matches!(self.state, State::OscParam | State::OscEsc)
            .then(|| self.sequence_start.unwrap_or(0))
    }

    /// Process a chunk of bytes, calling `on_event` for every OSC 133 marker
    /// found.
    ///
    /// All bytes in `data` should still be forwarded to the terminal by the
    /// caller — this method only *observes* the stream.
    #[cfg(test)]
    #[inline]
    pub fn push(&mut self, data: &[u8], mut on_event: impl FnMut(Event)) {
        self.push_located(data, |located| on_event(located.event));
    }

    /// Process a chunk of bytes, calling `on_event` for every OSC 133 marker
    /// found with its byte offset in this chunk.
    ///
    /// The offset points to the first byte after the marker terminator, making
    /// it suitable for callers that need to split the original chunk at marker
    /// boundaries.
    #[inline]
    pub fn push_located(&mut self, data: &[u8], mut on_event: impl FnMut(LocatedEvent)) {
        self.sequence_start = (self.state != State::Ground).then_some(0);

        for (offset, &byte) in data.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if byte == ESC {
                        self.state = State::Esc;
                        self.sequence_start = Some(offset);
                    }
                }
                State::Esc => {
                    if byte == RIGHT_BRACKET {
                        self.state = State::OscParam;
                        self.param_len = 0;
                    } else {
                        self.state = State::Ground;
                        self.sequence_start = None;
                    }
                }
                State::OscParam => {
                    if byte == BEL || byte == C1_ST {
                        self.dispatch(offset + 1, &mut on_event);
                        self.state = State::Ground;
                        self.sequence_start = None;
                    } else if byte == ESC {
                        self.state = State::OscEsc;
                    } else if self.param_len < PARAM_BUF_CAP {
                        self.param_buf[self.param_len] = byte;
                        self.param_len += 1;
                    }
                    // If param_len == PARAM_BUF_CAP we silently stop
                    // accumulating — dispatch will ignore non-133 sequences.
                }
                State::OscEsc => {
                    if byte == BACKSLASH {
                        self.dispatch(offset + 1, &mut on_event);
                    }
                    // Whether we got a valid ST or not, return to ground.
                    // (A new ESC ] would restart accumulation via the Ground
                    // -> Esc -> OscParam path on the *next* byte.)
                    self.state = State::Ground;
                    self.sequence_start = None;
                }
            }
        }
    }

    /// Inspect the accumulated parameter buffer.  If it holds an OSC 133
    /// payload, emit the corresponding [`Event`] and update the zone.
    #[inline]
    fn dispatch(&mut self, offset: usize, on_event: &mut impl FnMut(LocatedEvent)) {
        let payload = &self.param_buf[..self.param_len];

        if payload.len() < 5 || &payload[..4] != b"133;" {
            return;
        }

        if payload.len() > 5 && payload[5] != b';' {
            return;
        }

        let metadata = payload.get(6..).unwrap_or_default();
        let cmd = payload[4];
        let (event, params) = match cmd {
            b'A' => {
                self.zone = Zone::Prompt;
                (Event::PromptStart, parse_params(metadata))
            }
            b'B' => {
                self.zone = Zone::Input;
                (Event::CommandStart, parse_params(metadata))
            }
            b'C' => {
                self.zone = Zone::Output;
                (Event::CommandExecuted, parse_params(metadata))
            }
            b'D' => {
                let (exit_code, params) = parse_command_finished_params(metadata);
                self.zone = Zone::Unknown;
                (Event::CommandFinished { exit_code }, params)
            }
            _ => return,
        };

        on_event(LocatedEvent {
            event,
            start_offset: self.sequence_start.unwrap_or(0),
            offset,
            zone: self.zone,
            params,
        });
    }
}

fn parse_command_finished_params(metadata: &[u8]) -> (Option<i32>, Params) {
    if metadata.is_empty() {
        return (None, Params::default());
    }

    let Some(separator) = metadata.iter().position(|byte| *byte == b';') else {
        return parse_exit_code(metadata).map_or_else(
            || (None, parse_params(metadata)),
            |exit_code| (Some(exit_code), Params::default()),
        );
    };

    let (first, rest) = metadata.split_at(separator);
    let rest = &rest[1..];

    parse_exit_code(first).map_or_else(
        || (None, parse_params(metadata)),
        |exit_code| (Some(exit_code), parse_params(rest)),
    )
}

fn parse_exit_code(code: &[u8]) -> Option<i32> {
    if code.is_empty() {
        return None;
    }

    std::str::from_utf8(code)
        .ok()
        .and_then(|code| code.parse::<i32>().ok())
}

fn parse_params(metadata: &[u8]) -> Params {
    let items = metadata
        .split(|byte| *byte == b';')
        .filter(|part| !part.is_empty())
        .map(parse_param)
        .collect();

    Params { items }
}

fn parse_param(param: &[u8]) -> Param {
    let param = String::from_utf8_lossy(param);

    if let Some((key, value)) = param.split_once('=') {
        return Param::KeyValue {
            key: key.to_string(),
            value: value.to_string(),
        };
    }

    Param::Value(param.into_owned())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect all events from a single `push` call.
    fn parse_events(data: &[u8]) -> Vec<Event> {
        let mut parser = Parser::new();
        let mut events = Vec::new();
        parser.push(data, |e| events.push(e));
        events
    }

    // -- Basic event detection ------------------------------------------------

    #[test]
    fn detect_prompt_start_bel() {
        let data = b"\x1b]133;A\x07";
        assert_eq!(parse_events(data), vec![Event::PromptStart]);
    }

    #[test]
    fn detect_prompt_start_st() {
        let data = b"\x1b]133;A\x1b\\";
        assert_eq!(parse_events(data), vec![Event::PromptStart]);
    }

    #[test]
    fn detect_command_start_bel() {
        let data = b"\x1b]133;B\x07";
        assert_eq!(parse_events(data), vec![Event::CommandStart]);
    }

    #[test]
    fn detect_command_start_st() {
        let data = b"\x1b]133;B\x1b\\";
        assert_eq!(parse_events(data), vec![Event::CommandStart]);
    }

    #[test]
    fn detect_command_executed_bel() {
        let data = b"\x1b]133;C\x07";
        assert_eq!(parse_events(data), vec![Event::CommandExecuted]);
    }

    #[test]
    fn detect_command_executed_st() {
        let data = b"\x1b]133;C\x1b\\";
        assert_eq!(parse_events(data), vec![Event::CommandExecuted]);
    }

    #[test]
    fn detect_command_finished_no_exit_code() {
        let data = b"\x1b]133;D\x07";
        assert_eq!(
            parse_events(data),
            vec![Event::CommandFinished { exit_code: None }]
        );
    }

    #[test]
    fn detect_command_finished_exit_zero() {
        let data = b"\x1b]133;D;0\x07";
        assert_eq!(
            parse_events(data),
            vec![Event::CommandFinished { exit_code: Some(0) }]
        );
    }

    #[test]
    fn detect_command_finished_exit_nonzero() {
        let data = b"\x1b]133;D;127\x07";
        assert_eq!(
            parse_events(data),
            vec![Event::CommandFinished {
                exit_code: Some(127)
            }]
        );
    }

    #[test]
    fn detect_command_finished_negative_exit_code() {
        let data = b"\x1b]133;D;-1\x07";
        assert_eq!(
            parse_events(data),
            vec![Event::CommandFinished {
                exit_code: Some(-1)
            }]
        );
    }

    #[test]
    fn detect_command_finished_exit_code_st() {
        let data = b"\x1b]133;D;42\x1b\\";
        assert_eq!(
            parse_events(data),
            vec![Event::CommandFinished {
                exit_code: Some(42)
            }]
        );
    }

    #[test]
    fn invalid_exit_code_yields_none() {
        let data = b"\x1b]133;D;abc\x07";
        assert_eq!(
            parse_events(data),
            vec![Event::CommandFinished { exit_code: None }]
        );
    }

    // -- Zone tracking --------------------------------------------------------

    #[test]
    fn zone_starts_unknown() {
        let parser = Parser::new();
        assert_eq!(parser.zone(), Zone::Unknown);
    }

    #[test]
    fn full_zone_cycle() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b]133;A\x07", |e| events.push(e));
        assert_eq!(parser.zone(), Zone::Prompt);

        parser.push(b"\x1b]133;B\x07", |e| events.push(e));
        assert_eq!(parser.zone(), Zone::Input);

        parser.push(b"\x1b]133;C\x07", |e| events.push(e));
        assert_eq!(parser.zone(), Zone::Output);

        parser.push(b"\x1b]133;D;0\x07", |e| events.push(e));
        assert_eq!(parser.zone(), Zone::Unknown);

        assert_eq!(
            events,
            vec![
                Event::PromptStart,
                Event::CommandStart,
                Event::CommandExecuted,
                Event::CommandFinished { exit_code: Some(0) },
            ]
        );
    }

    // -- Multiple events in one push ------------------------------------------

    #[test]
    fn multiple_events_single_push() {
        let data = b"\x1b]133;A\x07$ \x1b]133;B\x07ls\n\x1b]133;C\x07file.txt\n\x1b]133;D;0\x07";
        let events = parse_events(data);
        assert_eq!(
            events,
            vec![
                Event::PromptStart,
                Event::CommandStart,
                Event::CommandExecuted,
                Event::CommandFinished { exit_code: Some(0) },
            ]
        );
    }

    // -- Split across push boundaries -----------------------------------------

    #[test]
    fn split_esc_and_bracket() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b", |e| events.push(e));
        assert!(events.is_empty());

        parser.push(b"]133;A\x07", |e| events.push(e));
        assert_eq!(events, vec![Event::PromptStart]);
    }

    #[test]
    fn split_mid_param() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b]13", |e| events.push(e));
        assert!(events.is_empty());

        parser.push(b"3;D;42\x07", |e| events.push(e));
        assert_eq!(
            events,
            vec![Event::CommandFinished {
                exit_code: Some(42)
            }]
        );
    }

    #[test]
    fn split_before_terminator() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b]133;B", |e| events.push(e));
        assert!(events.is_empty());

        parser.push(b"\x07", |e| events.push(e));
        assert_eq!(events, vec![Event::CommandStart]);
    }

    #[test]
    fn split_esc_backslash_terminator() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b]133;C\x1b", |e| events.push(e));
        assert!(events.is_empty());

        parser.push(b"\\", |e| events.push(e));
        assert_eq!(events, vec![Event::CommandExecuted]);
    }

    // -- Interleaved normal text ----------------------------------------------

    #[test]
    fn normal_text_before_and_after() {
        let data = b"hello world\x1b]133;A\x07prompt text\x1b]133;B\x07command";
        let events = parse_events(data);
        assert_eq!(events, vec![Event::PromptStart, Event::CommandStart]);
    }

    // -- Non-133 OSC sequences (should be ignored) ----------------------------

    #[test]
    fn non_133_osc_ignored() {
        let data = b"\x1b]0;window title\x07\x1b]133;A\x07";
        let events = parse_events(data);
        assert_eq!(events, vec![Event::PromptStart]);
    }

    #[test]
    fn osc_7_ignored() {
        let data = b"\x1b]7;file:///home/user\x07";
        assert!(parse_events(data).is_empty());
    }

    // -- Unknown command letter -----------------------------------------------

    #[test]
    fn unknown_command_ignored() {
        let data = b"\x1b]133;Z\x07";
        assert!(parse_events(data).is_empty());
    }

    #[test]
    fn marker_with_unexpected_trailing_bytes_ignored() {
        let data = b"\x1b]133;ABC\x07";
        assert!(parse_events(data).is_empty());
    }

    // -- Malformed sequences --------------------------------------------------

    #[test]
    fn esc_followed_by_non_bracket() {
        let data = b"\x1b[31m\x1b]133;A\x07";
        let events = parse_events(data);
        assert_eq!(events, vec![Event::PromptStart]);
    }

    #[test]
    fn lone_esc_at_end_of_chunk() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b", |e| events.push(e));
        assert!(events.is_empty());

        // Feed non-bracket to abort the escape, then a real sequence.
        parser.push(b"x\x1b]133;A\x07", |e| events.push(e));
        assert_eq!(events, vec![Event::PromptStart]);
    }

    #[test]
    fn truncated_133_prefix() {
        // "13" followed by terminator — not "133;" so no event.
        let data = b"\x1b]13\x07";
        assert!(parse_events(data).is_empty());
    }

    #[test]
    fn empty_osc() {
        let data = b"\x1b]\x07";
        assert!(parse_events(data).is_empty());
    }

    // -- Buffer overflow (very long non-133 OSC) ------------------------------

    #[test]
    fn very_long_osc_does_not_panic() {
        let mut data = Vec::new();
        data.extend_from_slice(b"\x1b]");
        data.extend(std::iter::repeat_n(b'x', 1000));
        data.push(BEL);
        // Should not panic and should produce no event.
        assert!(parse_events(&data).is_empty());
    }

    // -- Empty input ----------------------------------------------------------

    #[test]
    fn empty_input() {
        assert!(parse_events(b"").is_empty());
    }

    #[test]
    fn only_normal_text() {
        let data = b"just some regular terminal output\r\n";
        assert!(parse_events(data).is_empty());
    }

    // -- Repeated prompts (empty command) ------------------------------------

    #[test]
    fn repeated_prompt_cycle() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        // User hits enter on an empty prompt twice.
        let data = b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;D\x07\x1b]133;A\x07$ \x1b]133;B\x07";
        parser.push(data, |e| events.push(e));

        assert_eq!(
            events,
            vec![
                Event::PromptStart,
                Event::CommandStart,
                Event::CommandFinished { exit_code: None },
                Event::PromptStart,
                Event::CommandStart,
            ]
        );
        assert_eq!(parser.zone(), Zone::Input);
    }

    // -- Byte-at-a-time feeding -----------------------------------------------

    #[test]
    fn byte_at_a_time() {
        let data = b"\x1b]133;D;99\x07";
        let mut parser = Parser::new();
        let mut events = Vec::new();

        for &byte in data {
            parser.push(&[byte], |e| events.push(e));
        }

        assert_eq!(
            events,
            vec![Event::CommandFinished {
                exit_code: Some(99)
            }]
        );
    }

    // -- Mixed terminators ----------------------------------------------------

    #[test]
    fn mixed_bel_and_st_terminators() {
        let data = b"\x1b]133;A\x07\x1b]133;B\x1b\\\x1b]133;C\x07\x1b]133;D;1\x1b\\";
        let events = parse_events(data);
        assert_eq!(
            events,
            vec![
                Event::PromptStart,
                Event::CommandStart,
                Event::CommandExecuted,
                Event::CommandFinished { exit_code: Some(1) },
            ]
        );
    }

    #[test]
    fn detects_c1_st_terminator() {
        let data = b"\x1b]133;A\x9c";
        assert_eq!(parse_events(data), vec![Event::PromptStart]);
    }

    // -- Located event offsets ------------------------------------------------

    #[test]
    fn located_event_reports_offset_after_marker() {
        let data = b"before\x1b]133;A\x07prompt";
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push_located(data, |e| events.push(e));

        assert_eq!(
            events,
            vec![LocatedEvent {
                event: Event::PromptStart,
                start_offset: b"before".len(),
                offset: b"before\x1b]133;A\x07".len(),
                zone: Zone::Prompt,
                params: Params::default(),
            }]
        );
    }

    #[test]
    fn located_event_offset_is_relative_to_completing_chunk() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push_located(b"\x1b]133;", |e| events.push(e));
        parser.push_located(b"D;42\x07after", |e| events.push(e));

        assert_eq!(
            events,
            vec![LocatedEvent {
                event: Event::CommandFinished {
                    exit_code: Some(42)
                },
                start_offset: 0,
                offset: b"D;42\x07".len(),
                zone: Zone::Unknown,
                params: Params::default(),
            }]
        );
    }

    #[test]
    fn located_event_preserves_metadata_params() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push_located(
            b"\x1b]133;D;127;history_id=018f;session_id=abcd;flag\x07",
            |event| events.push(event),
        );

        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(
            event.event,
            Event::CommandFinished {
                exit_code: Some(127)
            }
        );
        assert_eq!(event.params.get("history_id"), Some("018f"));
        assert_eq!(event.params.get("session_id"), Some("abcd"));
        assert!(
            event
                .params
                .iter()
                .any(|param| param == &Param::Value("flag".to_string()))
        );
    }

    #[test]
    fn command_finished_metadata_without_exit_code_is_preserved() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push_located(b"\x1b]133;D;history_id=018f;session_id=abcd\x07", |event| {
            events.push(event);
        });

        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.event, Event::CommandFinished { exit_code: None });
        assert_eq!(event.params.get("history_id"), Some("018f"));
        assert_eq!(event.params.get("session_id"), Some("abcd"));
    }

    // -- Default trait --------------------------------------------------------

    #[test]
    fn parser_default() {
        let parser = Parser::default();
        assert_eq!(parser.zone(), Zone::Unknown);
    }

    #[test]
    fn zone_default() {
        assert_eq!(Zone::default(), Zone::Unknown);
    }

    // -- D with empty exit code field -----------------------------------------

    #[test]
    fn d_with_semicolon_but_empty_code() {
        // "133;D;" — semicolon present but no digits.
        let data = b"\x1b]133;D;\x07";
        assert_eq!(
            parse_events(data),
            vec![Event::CommandFinished { exit_code: None }]
        );
    }

    // -- Consecutive OSC sequences without gap --------------------------------

    #[test]
    fn back_to_back_osc_no_gap() {
        let data = b"\x1b]133;A\x07\x1b]133;B\x07";
        let events = parse_events(data);
        assert_eq!(events, vec![Event::PromptStart, Event::CommandStart]);
    }

    // -- CSI sequences interleaved (should not confuse parser) ----------------

    #[test]
    fn csi_sequences_ignored() {
        // CSI (ESC [) color codes mixed with OSC 133.
        let data = b"\x1b[32m\x1b]133;A\x07\x1b[0m$ \x1b]133;B\x07";
        let events = parse_events(data);
        assert_eq!(events, vec![Event::PromptStart, Event::CommandStart]);
    }

    // -- Large exit codes -----------------------------------------------------

    #[test]
    fn large_exit_code() {
        let data = b"\x1b]133;D;2147483647\x07";
        assert_eq!(
            parse_events(data),
            vec![Event::CommandFinished {
                exit_code: Some(i32::MAX)
            }]
        );
    }

    #[test]
    fn overflow_exit_code_yields_none() {
        let data = b"\x1b]133;D;9999999999999\x07";
        assert_eq!(
            parse_events(data),
            vec![Event::CommandFinished { exit_code: None }]
        );
    }
}
