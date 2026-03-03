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

/// Events emitted when an OSC 133 marker is detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    PromptStart,
    CommandStart,
    CommandExecuted,
    CommandFinished { exit_code: Option<i32> },
}

/// The current semantic zone as determined by the most recent OSC 133 marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Unknown,
    Prompt,
    Input,
    Output,
}

impl Default for Zone {
    fn default() -> Self {
        Self::Unknown
    }
}

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const BACKSLASH: u8 = b'\\';
const RIGHT_BRACKET: u8 = b']';
const PARAM_BUF_CAP: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    OscParam,
    OscEsc,
}

/// A streaming, zero-allocation parser for OSC 133 escape sequences.
///
/// Feed byte slices into [`Parser::push`]. The parser detects OSC 133 markers
/// and reports [`Event`]s with their byte offsets through a callback without
/// modifying the data.
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
    #[inline]
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            zone: Zone::Unknown,
            param_buf: [0u8; PARAM_BUF_CAP],
            param_len: 0,
        }
    }

    #[inline]
    pub fn zone(&self) -> Zone {
        self.zone
    }

    /// Process a chunk of bytes, calling `on_event` for every OSC 133 marker.
    ///
    /// The `usize` passed to the callback is the byte offset of the sequence's
    /// terminator within `data`. Bytes after that offset belong to the new zone.
    #[inline]
    pub fn push(&mut self, data: &[u8], mut on_event: impl FnMut(Event, usize)) {
        for (i, &byte) in data.iter().enumerate() {
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
                        self.dispatch(i, &mut on_event);
                        self.state = State::Ground;
                    } else if byte == ESC {
                        self.state = State::OscEsc;
                    } else if self.param_len < PARAM_BUF_CAP {
                        self.param_buf[self.param_len] = byte;
                        self.param_len += 1;
                    }
                }
                State::OscEsc => {
                    if byte == BACKSLASH {
                        self.dispatch(i, &mut on_event);
                    }
                    self.state = State::Ground;
                }
            }
        }
    }

    #[inline]
    fn dispatch(&mut self, offset: usize, on_event: &mut impl FnMut(Event, usize)) {
        let params = &self.param_buf[..self.param_len];

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

        on_event(event, offset);
    }
}

/// ANSI background color to inject for a given zone transition (debug mode).
pub fn debug_color(event: &Event) -> &'static [u8] {
    match event {
        Event::PromptStart => b"\x1b[42m",          // green bg
        Event::CommandStart => b"\x1b[44m",          // blue bg
        Event::CommandExecuted => b"\x1b[43m",       // yellow bg
        Event::CommandFinished { .. } => b"\x1b[0m", // reset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_events(data: &[u8]) -> Vec<Event> {
        let mut parser = Parser::new();
        let mut events = Vec::new();
        parser.push(data, |e, _| events.push(e));
        events
    }

    fn parse_events_with_offsets(data: &[u8]) -> Vec<(Event, usize)> {
        let mut parser = Parser::new();
        let mut out = Vec::new();
        parser.push(data, |e, off| out.push((e, off)));
        out
    }

    // -- Basic event detection ------------------------------------------------

    #[test]
    fn detect_prompt_start_bel() {
        assert_eq!(parse_events(b"\x1b]133;A\x07"), vec![Event::PromptStart]);
    }

    #[test]
    fn detect_prompt_start_st() {
        assert_eq!(
            parse_events(b"\x1b]133;A\x1b\\"),
            vec![Event::PromptStart]
        );
    }

    #[test]
    fn detect_command_start_bel() {
        assert_eq!(parse_events(b"\x1b]133;B\x07"), vec![Event::CommandStart]);
    }

    #[test]
    fn detect_command_start_st() {
        assert_eq!(
            parse_events(b"\x1b]133;B\x1b\\"),
            vec![Event::CommandStart]
        );
    }

    #[test]
    fn detect_command_executed_bel() {
        assert_eq!(
            parse_events(b"\x1b]133;C\x07"),
            vec![Event::CommandExecuted]
        );
    }

    #[test]
    fn detect_command_executed_st() {
        assert_eq!(
            parse_events(b"\x1b]133;C\x1b\\"),
            vec![Event::CommandExecuted]
        );
    }

    #[test]
    fn detect_command_finished_no_exit_code() {
        assert_eq!(
            parse_events(b"\x1b]133;D\x07"),
            vec![Event::CommandFinished { exit_code: None }]
        );
    }

    #[test]
    fn detect_command_finished_exit_zero() {
        assert_eq!(
            parse_events(b"\x1b]133;D;0\x07"),
            vec![Event::CommandFinished {
                exit_code: Some(0)
            }]
        );
    }

    #[test]
    fn detect_command_finished_exit_nonzero() {
        assert_eq!(
            parse_events(b"\x1b]133;D;127\x07"),
            vec![Event::CommandFinished {
                exit_code: Some(127)
            }]
        );
    }

    #[test]
    fn detect_command_finished_negative_exit_code() {
        assert_eq!(
            parse_events(b"\x1b]133;D;-1\x07"),
            vec![Event::CommandFinished {
                exit_code: Some(-1)
            }]
        );
    }

    #[test]
    fn detect_command_finished_exit_code_st() {
        assert_eq!(
            parse_events(b"\x1b]133;D;42\x1b\\"),
            vec![Event::CommandFinished {
                exit_code: Some(42)
            }]
        );
    }

    #[test]
    fn invalid_exit_code_yields_none() {
        assert_eq!(
            parse_events(b"\x1b]133;D;abc\x07"),
            vec![Event::CommandFinished { exit_code: None }]
        );
    }

    // -- Zone tracking --------------------------------------------------------

    #[test]
    fn zone_starts_unknown() {
        assert_eq!(Parser::new().zone(), Zone::Unknown);
    }

    #[test]
    fn full_zone_cycle() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b]133;A\x07", |e, _| events.push(e));
        assert_eq!(parser.zone(), Zone::Prompt);

        parser.push(b"\x1b]133;B\x07", |e, _| events.push(e));
        assert_eq!(parser.zone(), Zone::Input);

        parser.push(b"\x1b]133;C\x07", |e, _| events.push(e));
        assert_eq!(parser.zone(), Zone::Output);

        parser.push(b"\x1b]133;D;0\x07", |e, _| events.push(e));
        assert_eq!(parser.zone(), Zone::Unknown);

        assert_eq!(
            events,
            vec![
                Event::PromptStart,
                Event::CommandStart,
                Event::CommandExecuted,
                Event::CommandFinished {
                    exit_code: Some(0)
                },
            ]
        );
    }

    // -- Multiple events in one push ------------------------------------------

    #[test]
    fn multiple_events_single_push() {
        let data =
            b"\x1b]133;A\x07$ \x1b]133;B\x07ls\n\x1b]133;C\x07file.txt\n\x1b]133;D;0\x07";
        assert_eq!(
            parse_events(data),
            vec![
                Event::PromptStart,
                Event::CommandStart,
                Event::CommandExecuted,
                Event::CommandFinished {
                    exit_code: Some(0)
                },
            ]
        );
    }

    // -- Split across push boundaries -----------------------------------------

    #[test]
    fn split_esc_and_bracket() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b", |e, _| events.push(e));
        assert!(events.is_empty());

        parser.push(b"]133;A\x07", |e, _| events.push(e));
        assert_eq!(events, vec![Event::PromptStart]);
    }

    #[test]
    fn split_mid_param() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b]13", |e, _| events.push(e));
        assert!(events.is_empty());

        parser.push(b"3;D;42\x07", |e, _| events.push(e));
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

        parser.push(b"\x1b]133;B", |e, _| events.push(e));
        assert!(events.is_empty());

        parser.push(b"\x07", |e, _| events.push(e));
        assert_eq!(events, vec![Event::CommandStart]);
    }

    #[test]
    fn split_esc_backslash_terminator() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b]133;C\x1b", |e, _| events.push(e));
        assert!(events.is_empty());

        parser.push(b"\\", |e, _| events.push(e));
        assert_eq!(events, vec![Event::CommandExecuted]);
    }

    // -- Interleaved normal text ----------------------------------------------

    #[test]
    fn normal_text_before_and_after() {
        let data = b"hello world\x1b]133;A\x07prompt text\x1b]133;B\x07command";
        assert_eq!(
            parse_events(data),
            vec![Event::PromptStart, Event::CommandStart]
        );
    }

    // -- Non-133 OSC sequences ignored ----------------------------------------

    #[test]
    fn non_133_osc_ignored() {
        assert_eq!(
            parse_events(b"\x1b]0;window title\x07\x1b]133;A\x07"),
            vec![Event::PromptStart]
        );
    }

    #[test]
    fn osc_7_ignored() {
        assert!(parse_events(b"\x1b]7;file:///home/user\x07").is_empty());
    }

    #[test]
    fn unknown_command_ignored() {
        assert!(parse_events(b"\x1b]133;Z\x07").is_empty());
    }

    // -- Malformed sequences --------------------------------------------------

    #[test]
    fn esc_followed_by_non_bracket() {
        assert_eq!(
            parse_events(b"\x1b[31m\x1b]133;A\x07"),
            vec![Event::PromptStart]
        );
    }

    #[test]
    fn lone_esc_at_end_of_chunk() {
        let mut parser = Parser::new();
        let mut events = Vec::new();

        parser.push(b"\x1b", |e, _| events.push(e));
        assert!(events.is_empty());

        parser.push(b"x\x1b]133;A\x07", |e, _| events.push(e));
        assert_eq!(events, vec![Event::PromptStart]);
    }

    #[test]
    fn truncated_133_prefix() {
        assert!(parse_events(b"\x1b]13\x07").is_empty());
    }

    #[test]
    fn empty_osc() {
        assert!(parse_events(b"\x1b]\x07").is_empty());
    }

    #[test]
    fn very_long_osc_does_not_panic() {
        let mut data = Vec::new();
        data.extend_from_slice(b"\x1b]");
        data.extend(std::iter::repeat(b'x').take(1000));
        data.push(BEL);
        assert!(parse_events(&data).is_empty());
    }

    // -- Empty / plain input --------------------------------------------------

    #[test]
    fn empty_input() {
        assert!(parse_events(b"").is_empty());
    }

    #[test]
    fn only_normal_text() {
        assert!(parse_events(b"just some regular terminal output\r\n").is_empty());
    }

    // -- Repeated prompts (empty command) ------------------------------------

    #[test]
    fn repeated_prompt_cycle() {
        let mut parser = Parser::new();
        let mut events = Vec::new();
        let data =
            b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;D\x07\x1b]133;A\x07$ \x1b]133;B\x07";
        parser.push(data, |e, _| events.push(e));

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
            parser.push(&[byte], |e, _| events.push(e));
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
        assert_eq!(
            parse_events(b"\x1b]133;A\x07\x1b]133;B\x1b\\\x1b]133;C\x07\x1b]133;D;1\x1b\\"),
            vec![
                Event::PromptStart,
                Event::CommandStart,
                Event::CommandExecuted,
                Event::CommandFinished {
                    exit_code: Some(1)
                },
            ]
        );
    }

    // -- Default trait --------------------------------------------------------

    #[test]
    fn parser_default() {
        assert_eq!(Parser::default().zone(), Zone::Unknown);
    }

    #[test]
    fn zone_default() {
        assert_eq!(Zone::default(), Zone::Unknown);
    }

    // -- D with empty exit code field -----------------------------------------

    #[test]
    fn d_with_semicolon_but_empty_code() {
        assert_eq!(
            parse_events(b"\x1b]133;D;\x07"),
            vec![Event::CommandFinished { exit_code: None }]
        );
    }

    // -- Back-to-back sequences -----------------------------------------------

    #[test]
    fn back_to_back_osc_no_gap() {
        assert_eq!(
            parse_events(b"\x1b]133;A\x07\x1b]133;B\x07"),
            vec![Event::PromptStart, Event::CommandStart]
        );
    }

    // -- CSI sequences interleaved --------------------------------------------

    #[test]
    fn csi_sequences_ignored() {
        assert_eq!(
            parse_events(b"\x1b[32m\x1b]133;A\x07\x1b[0m$ \x1b]133;B\x07"),
            vec![Event::PromptStart, Event::CommandStart]
        );
    }

    // -- Large exit codes -----------------------------------------------------

    #[test]
    fn large_exit_code() {
        assert_eq!(
            parse_events(b"\x1b]133;D;2147483647\x07"),
            vec![Event::CommandFinished {
                exit_code: Some(i32::MAX)
            }]
        );
    }

    #[test]
    fn overflow_exit_code_yields_none() {
        assert_eq!(
            parse_events(b"\x1b]133;D;9999999999999\x07"),
            vec![Event::CommandFinished { exit_code: None }]
        );
    }

    // -- Byte offsets ---------------------------------------------------------

    #[test]
    fn offset_points_to_bel_terminator() {
        //                  0123456789
        let data = b"abc\x1b]133;A\x07def";
        let results = parse_events_with_offsets(data);
        // ESC=3 ]=4 1=5 3=6 3=7 ;=8 A=9 BEL=10
        assert_eq!(results, vec![(Event::PromptStart, 10)]);
    }

    #[test]
    fn offset_points_to_st_backslash() {
        //                  01234567 8
        let data = b"\x1b]133;B\x1b\\rest";
        let results = parse_events_with_offsets(data);
        // ESC=0 ]=1 1=2 3=3 3=4 ;=5 B=6 ESC=7 \=8
        assert_eq!(results, vec![(Event::CommandStart, 8)]);
    }

    #[test]
    fn multiple_offsets() {
        let data = b"\x1b]133;A\x07xx\x1b]133;B\x07";
        let results = parse_events_with_offsets(data);
        // First BEL at 7, then "xx" at 8,9, then ESC=10 ]=11 1=12 3=13 3=14 ;=15 B=16 BEL=17
        assert_eq!(
            results,
            vec![(Event::PromptStart, 7), (Event::CommandStart, 17)]
        );
    }

    #[test]
    fn offset_in_split_chunk_is_relative_to_chunk() {
        let mut parser = Parser::new();
        let mut offsets = Vec::new();

        parser.push(b"\x1b]133;", |_, off| offsets.push(off));
        assert!(offsets.is_empty());

        // "A\x07" — A=0, BEL=1 in this chunk
        parser.push(b"A\x07", |_, off| offsets.push(off));
        assert_eq!(offsets, vec![1]);
    }

    // -- Debug colors ---------------------------------------------------------

    #[test]
    fn debug_color_returns_expected_escapes() {
        assert_eq!(debug_color(&Event::PromptStart), b"\x1b[42m");
        assert_eq!(debug_color(&Event::CommandStart), b"\x1b[44m");
        assert_eq!(debug_color(&Event::CommandExecuted), b"\x1b[43m");
        assert_eq!(
            debug_color(&Event::CommandFinished { exit_code: None }),
            b"\x1b[0m"
        );
    }

    // -- Write-with-injections helper -----------------------------------------

    #[test]
    fn write_colorized_injects_at_boundaries() {
        let data = b"abc\x1b]133;A\x07def\x1b]133;B\x07ghi";
        let mut parser = Parser::new();
        let mut injections = Vec::new();
        parser.push(data, |event, offset| {
            injections.push((offset, debug_color(&event)));
        });

        let mut out = Vec::new();
        let mut from = 0;
        for &(offset, color) in &injections {
            let end = offset + 1;
            out.extend_from_slice(&data[from..end]);
            out.extend_from_slice(color);
            from = end;
        }
        out.extend_from_slice(&data[from..]);

        // "abc" + OSC A sequence + green_bg + "def" + OSC B sequence + blue_bg + "ghi"
        let mut expected = Vec::new();
        expected.extend_from_slice(b"abc\x1b]133;A\x07");
        expected.extend_from_slice(b"\x1b[42m");
        expected.extend_from_slice(b"def\x1b]133;B\x07");
        expected.extend_from_slice(b"\x1b[44m");
        expected.extend_from_slice(b"ghi");
        assert_eq!(out, expected);
    }
}
