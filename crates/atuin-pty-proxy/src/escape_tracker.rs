//! Following the escape sequences in the child's output, to know where bytes of our own can go.
//!
//! The proxy writes titles of its own into the output it forwards. A title written inside one of
//! the child's escape sequences, or between the bytes of a UTF-8 character, would garble both, so
//! [`EscapeTracker`] follows the output through the states of a VT parser
//! (<https://vt100.net/emu/dec_ansi_parser>), the way `vte` implements them, and says when the
//! output is between sequences and characters. Where terminals disagree it errs towards waiting:
//! C1 controls are text, because a UTF-8 terminal reads `0x9c` as part of a character rather than
//! as a string terminator.
//!
//! On the way it notices the child setting a title itself, with `OSC 0`, `1` or `2`.

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1A;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Between sequences, with this many bytes of a UTF-8 character still to come.
    Ground {
        continuation: u8,
    },
    /// After `ESC`.
    Escape,
    /// After `ESC` and an intermediate byte.
    EscapeIntermediate,
    /// Inside a control sequence (`ESC [`).
    Csi,
    /// Inside an operating system command (`ESC ]`), before its number.
    OscStart,
    /// Inside an `OSC 0`, `1` or `2`, before the `;` that starts its title.
    OscTitleNumber,
    /// Inside an operating system command's text, which is a title if `title`.
    OscText {
        title: bool,
    },
    /// Inside a device control string, or a start of string, privacy message or application
    /// program command.
    ControlString,
}

/// Follows a stream of terminal output through its escape sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscapeTracker {
    state: State,
}

impl Default for EscapeTracker {
    fn default() -> Self {
        Self {
            state: State::Ground { continuation: 0 },
        }
    }
}

impl EscapeTracker {
    /// Follow `data`, returning whether it set a window title or icon name.
    pub fn push(&mut self, data: &[u8]) -> bool {
        let mut titled = false;
        let mut rest = data;
        loop {
            // Plain text is most of any output, and only an `ESC` leads out of it.
            if let State::Ground { continuation } = self.state {
                let Some(escape) = memchr::memchr(ESC, rest) else {
                    self.state = State::Ground {
                        continuation: Self::continuation_after(continuation, rest),
                    };
                    return titled;
                };
                self.state = State::Escape;
                rest = &rest[escape + 1..];
            }
            let Some((&byte, tail)) = rest.split_first() else {
                return titled;
            };
            titled |= self.advance(byte);
            rest = tail;
        }
    }

    /// Whether the output is between escape sequences and characters, where bytes of our own can't
    /// change the meaning of the child's.
    pub const fn at_ground(&self) -> bool {
        matches!(self.state, State::Ground { continuation: 0 })
    }

    /// Follow one byte, returning whether it ended a title.
    fn advance(&mut self, byte: u8) -> bool {
        let ground = State::Ground { continuation: 0 };
        let next = match (self.state, byte) {
            (State::Ground { .. }, ESC) => State::Escape,
            (State::Ground { continuation }, _) => State::Ground {
                continuation: Self::continuation_after(continuation, &[byte]),
            },
            // These two end any sequence, an unfinished command included, which `vte` still acts on.
            (_, CAN | SUB) => ground,
            (_, ESC) => State::Escape,
            (State::Escape, 0x20..=0x2F) => State::EscapeIntermediate,
            (State::Escape, b'[') => State::Csi,
            (State::Escape, b']') => State::OscStart,
            (State::Escape, b'P' | b'X' | b'^' | b'_') => State::ControlString,
            (State::Escape | State::EscapeIntermediate, 0x30..=0x7E)
            | (State::Csi, 0x40..=0x7E) => ground,
            (State::OscStart | State::OscTitleNumber | State::OscText { .. }, BEL) => ground,
            (State::OscStart, b'0'..=b'2') => State::OscTitleNumber,
            (State::OscTitleNumber, b';') => State::OscText { title: true },
            // Controls inside a command are ignored, even in its number.
            (State::OscStart | State::OscTitleNumber, 0x00..=0x1F) => self.state,
            (State::OscStart | State::OscTitleNumber, _) => State::OscText { title: false },
            // Anything else leaves a sequence where it was: its parameters, intermediates and
            // text, and the controls a terminal runs or ignores in the middle of it.
            (state, _) => state,
        };
        let titled = matches!(self.state, State::OscText { title: true })
            && !matches!(next, State::OscText { .. });
        self.state = next;
        titled
    }

    /// How many bytes of a UTF-8 character are still to come after `run`, if `continuation` were
    /// before it.
    fn continuation_after(continuation: u8, run: &[u8]) -> u8 {
        // A character has at most three bytes after its first, so whatever came before the last
        // three has ended or been cut short by them.
        let (start, continuation) = match run.len().checked_sub(3) {
            Some(start) => (start, 0),
            None => (0, continuation),
        };
        run[start..].iter().fold(continuation, |continuation, byte| match byte {
            0x80..=0xBF => continuation.saturating_sub(1),
            0xC2..=0xDF => 1,
            0xE0..=0xEF => 2,
            0xF0..=0xF4 => 3,
            _ => 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::nothing(b"")]
    #[case::text(b"hello\r\n")]
    #[case::sgr(b"\x1b[1;31mred\x1b[0m")]
    #[case::charset(b"\x1b(B")]
    #[case::keypad(b"\x1b=")]
    #[case::string_terminator(b"\x1b\\")]
    #[case::title_bel(b"\x1b]0;title\x07")]
    #[case::title_st(b"\x1b]2;title\x1b\\")]
    #[case::device_control(b"\x1bP1$r0m\x1b\\")]
    #[case::application_command(b"\x1b_Gf=24;AAAA\x1b\\")]
    #[case::cancelled(b"\x1b[31\x18")]
    #[case::two_byte_character("é".as_bytes())]
    #[case::four_byte_character("🦀".as_bytes())]
    #[case::stray_continuation(b"\x80")]
    fn at_ground_after_complete_output(#[case] data: &[u8]) {
        let mut tracker = EscapeTracker::default();
        let _ = tracker.push(data);
        assert!(tracker.at_ground());
    }

    #[rstest]
    #[case::escape(b"\x1b")]
    #[case::control_sequence(b"\x1b[1;3")]
    #[case::control_sequence_with_a_newline(b"\x1b[1\n")]
    #[case::charset(b"\x1b(")]
    #[case::command(b"\x1b]0;tit")]
    #[case::command_number(b"\x1b]13")]
    #[case::command_terminator(b"\x1b]0;title\x1b")]
    #[case::c1_terminator(b"\x1b]0;title\x9c")]
    #[case::device_control(b"\x1bP1$r0m")]
    #[case::device_control_bel(b"\x1bPq\x07")]
    #[case::first_of_two_bytes(&"é".as_bytes()[..1])]
    #[case::second_of_three_bytes(&"€".as_bytes()[..2])]
    #[case::third_of_four_bytes(&"🦀".as_bytes()[..3])]
    fn not_at_ground_inside_a_sequence(#[case] data: &[u8]) {
        let mut tracker = EscapeTracker::default();
        let _ = tracker.push(data);
        assert!(!tracker.at_ground());
    }

    #[rstest]
    #[case::window_and_icon(b"\x1b]0;title\x07", true)]
    #[case::icon(b"\x1b]1;title\x07", true)]
    #[case::window(b"\x1b]2;title\x07", true)]
    #[case::string_terminator(b"\x1b]2;title\x1b\\", true)]
    #[case::empty(b"\x1b]2;\x07", true)]
    #[case::semicolon_in_the_title(b"\x1b]2;a;b\x07", true)]
    #[case::ended_by_cancel(b"\x1b]2;title\x18", true)]
    #[case::ended_by_another_sequence(b"\x1b]2;title\x1b[m", true)]
    #[case::control_in_the_number(b"\x1b]\x052;title\x07", true)]
    #[case::unfinished(b"\x1b]2;title", false)]
    #[case::no_title(b"\x1b]2\x07", false)]
    #[case::directory(b"\x1b]7;file:///tmp\x07", false)]
    #[case::pointer_shape(b"\x1b]22;xterm\x07", false)]
    #[case::foreground_color(b"\x1b]10;?\x07", false)]
    #[case::atuin_marker(b"\x1b]18188735;C;id\x07", false)]
    #[case::inside_a_device_control_string(b"\x1bPq]2;title\x07\x1b\\", false)]
    fn notices_titles(#[case] data: &[u8], #[case] titled: bool) {
        assert_eq!(EscapeTracker::default().push(data), titled);
    }

    #[rstest]
    fn notices_a_title_split_across_pushes() {
        let mut tracker = EscapeTracker::default();
        assert!(!tracker.push(b"\x1b]2;ti"));
        assert!(tracker.push(b"tle\x07"));
    }

    /// Records what `vte` does: each character it prints, `None` for anything else, and whether it
    /// saw a title.
    #[derive(Default)]
    struct Reference {
        actions: Vec<Option<char>>,
        titled: bool,
    }

    impl vte::Perform for Reference {
        fn print(&mut self, c: char) {
            self.actions.push(Some(c));
        }

        fn execute(&mut self, _byte: u8) {
            self.actions.push(None);
        }

        fn hook(&mut self, _: &vte::Params, _: &[u8], _: bool, _: char) {
            self.actions.push(None);
        }

        fn put(&mut self, _byte: u8) {
            self.actions.push(None);
        }

        fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
            self.actions.push(None);
            self.titled |= params.len() >= 2 && matches!(params[0], b"0" | b"1" | b"2");
        }

        fn csi_dispatch(&mut self, _: &vte::Params, _: &[u8], _: bool, _: char) {
            self.actions.push(None);
        }

        fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {
            self.actions.push(None);
        }
    }

    /// Output that is mostly escape sequences, broken off and spliced together at random.
    fn stream() -> impl Strategy<Value = Vec<u8>> {
        let piece = prop_oneof![
            Just(b"\x1b]0;title\x07".to_vec()),
            Just(b"\x1b]2;".to_vec()),
            Just(b"\x1b]7;".to_vec()),
            Just(b"\x1b]2".to_vec()),
            Just(b"\x1b[1;31m".to_vec()),
            Just(b"\x1b[1;3".to_vec()),
            Just(b"\x1b[?".to_vec()),
            Just(b"\x1bP1$r".to_vec()),
            Just(b"\x1b_".to_vec()),
            Just(b"\x1b(".to_vec()),
            Just(b"\x1b".to_vec()),
            Just(b"\x07".to_vec()),
            Just(b"\x1b\\".to_vec()),
            Just(vec![CAN]),
            Just(vec![SUB]),
            Just(vec![0x9C]),
            Just("é€🦀".as_bytes().to_vec()),
            Just(vec![0xE2]),
            Just(vec![0x82]),
            prop::collection::vec(any::<u8>(), 0..8),
        ];
        prop::collection::vec(piece, 0..16).prop_map(|pieces| pieces.concat())
    }

    /// Push `data` in pieces cut at `splits`, returning the tracker and whether it saw a title.
    fn push_split(data: &[u8], splits: &[prop::sample::Index]) -> (EscapeTracker, bool) {
        let mut cuts: Vec<usize> = splits.iter().map(|index| index.index(data.len() + 1)).collect();
        cuts.push(0);
        cuts.push(data.len());
        cuts.sort_unstable();

        let mut tracker = EscapeTracker::default();
        let titled =
            cuts.windows(2).fold(false, |titled, cut| tracker.push(&data[cut[0]..cut[1]]) | titled);
        (tracker, titled)
    }

    proptest! {
        /// Output reaches the proxy in whatever pieces the pty hands over, so where it is cut must
        /// make no difference to what the tracker makes of it.
        #[rstest]
        fn reads_output_the_same_however_it_is_cut(
            data in stream(),
            splits in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
        ) {
            let whole = push_split(&data, &[]);
            prop_assert_eq!(&push_split(&data, &splits), &whole);

            let mut tracker = EscapeTracker::default();
            let titled = data.iter().fold(false, |titled, byte| tracker.push(&[*byte]) | titled);
            prop_assert_eq!(&(tracker, titled), &whole);
        }

        /// A title written where the tracker claims ground must reach a real parser as text of its
        /// own, not as part of something the child started: `vte` given an `X` there prints just
        /// the `X`. Both must also agree on whether the child set a title.
        #[rstest]
        fn agrees_with_vte(data in stream()) {
            let mut tracker = EscapeTracker::default();
            let titled = tracker.push(&data);
            let mut parser = vte::Parser::new();
            let mut reference = Reference::default();
            parser.advance(&mut reference, &data);
            prop_assert_eq!(titled, reference.titled);

            if tracker.at_ground() {
                reference.actions.clear();
                parser.advance(&mut reference, b"X");
                prop_assert_eq!(reference.actions, [Some('X')], "vte was mid-sequence after {:?}", data);
            }
        }
    }
}
