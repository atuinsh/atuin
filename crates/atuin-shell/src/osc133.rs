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
//! The wire format is `ESC ] 133 ; <cmd> [; <params>] ST` where ST is either
//! BEL (0x07) or ESC \ (0x1B 0x5C).
//!
//! # Design goals
//!
//! * **Zero-copy** — the parser observes the byte stream without buffering or
//!   modifying it.
//! * **Zero-alloc** — after construction no heap allocation occurs.
//! * **Non-blocking** — [`Parser::push`] processes whatever bytes are available
//!   and returns immediately.
//! * **Transparent** — the caller is responsible for forwarding bytes to their
//!   destination; the parser only emits [`Event`]s through a callback.

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
const BACKSLASH: u8 = b'\\';
const RIGHT_BRACKET: u8 = b']';

/// Maximum bytes we'll buffer for the OSC parameter string. 32 bytes is far
/// more than any valid OSC 133 payload needs (e.g. `133;D;127` is 9 bytes).
/// Longer (non-133) OSC sequences simply stop accumulating once the buffer is
/// full — the dispatch logic will harmlessly ignore them.
const PARAM_BUF_CAP: usize = 32;

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

    /// Process a chunk of bytes, calling `on_event` for every OSC 133 marker
    /// found.
    ///
    /// All bytes in `data` should still be forwarded to the terminal by the
    /// caller — this method only *observes* the stream.
    #[inline]
    pub fn push(&mut self, data: &[u8], mut on_event: impl FnMut(Event)) {
        for &byte in data {
            match self.state {
                State::Ground => {
                    if byte == ESC {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if byte == RIGHT_BRACKET {
                        self.state = State::OscParam;
                        self.param_len = 0;
                    } else {
                        self.state = State::Ground;
                    }
                }
                State::OscParam => {
                    if byte == BEL {
                        self.dispatch(&mut on_event);
                        self.state = State::Ground;
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
                        self.dispatch(&mut on_event);
                    }
                    // Whether we got a valid ST or not, return to ground.
                    // (A new ESC ] would restart accumulation via the Ground
                    // -> Esc -> OscParam path on the *next* byte.)
                    self.state = State::Ground;
                }
            }
        }
    }

    /// Inspect the accumulated parameter buffer.  If it holds an OSC 133
    /// payload, emit the corresponding [`Event`] and update the zone.
    #[inline]
    fn dispatch(&mut self, on_event: &mut impl FnMut(Event)) {
        let params = &self.param_buf[..self.param_len];

        // Must start with "133;"
        if params.len() < 5 || &params[..4] != b"133;" {
            return;
        }

        let cmd = params[4];
        let event = match cmd {
            b'A' => {
                self.zone = Zone::Prompt;
                Event::PromptStart
            }
            b'B' => {
                self.zone = Zone::Input;
                Event::CommandStart
            }
            b'C' => {
                self.zone = Zone::Output;
                Event::CommandExecuted
            }
            b'D' => {
                let exit_code = if params.len() > 6 && params[5] == b';' {
                    std::str::from_utf8(&params[6..])
                        .ok()
                        .and_then(|s| s.parse::<i32>().ok())
                } else {
                    None
                };
                self.zone = Zone::Unknown;
                Event::CommandFinished { exit_code }
            }
            _ => return,
        };

        on_event(event);
    }
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
        data.extend(std::iter::repeat(b'x').take(1000));
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
