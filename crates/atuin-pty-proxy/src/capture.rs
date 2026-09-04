use std::num::NonZeroU16;

use atuin_client::history::HistoryId;
use atuin_common::string::{BoundedBuffer, TrimExt as _};

use crate::osc133::{self, Event, EventChunk, EventChunks, Param, Zone};

/// Clears the screen while maintaining cursor position.
///
/// We specifically want to clear *without* keeping the current attributes; otherwise the cleared
/// cells could have background colors etc. So, we save the cursor and attributes with `ESC 7`,
/// reset the attributes with `CSI m`, clear the screen with `CSI 2 J`, and restore the cursor and
/// attributes with `ESC 8`.
const CLEAR_SCREEN_CONTENTS: &[u8] = b"\x1b7\x1b[m\x1b[2J\x1b8";
const DISABLE_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049l";

const HISTORY_ID_PARAM: &[u8] = b"history_id";
const SESSION_ID_PARAM: &[u8] = b"session_id";

/// The maximum number of bytes captured per zone.
///
/// During the process of capturing, the buffer might grow past this point, but never by more than
/// one screenful, which is almost certainly only a small fraction of this limit.
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

pub type CommandCaptureSink = Box<dyn Fn(CommandCapture) + Send + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCapture {
    /// The rendered output of the command.
    ///
    /// Contains SGR escape sequences. Contains no other escape sequences, and no control characters
    /// except `'\n'`.
    pub output: String,
    pub exit_code: Option<i32>,
    pub history_id: HistoryId,
    pub session_id: Option<String>,
    pub output_observed_bytes: u64,
    pub output_truncated: bool,
    pub terminal_width: u16,
    pub terminal_height: u16,
}

/// The state of an in-progress command capture.
#[derive(Default)]
struct CaptureState {
    output: String,
    output_truncated: bool,
    output_observed_bytes: u64,
    session_id: Option<String>,
    exit_code: Option<i32>,
}

impl CaptureState {
    /// Clear the [`CaptureState`], resetting it to the default state.
    ///
    /// This may be more efficient than creating a new [`CaptureState`], because it preserves
    /// the capacity of [`Self::output`].
    fn clear(&mut self) {
        self.output.clear();
        self.output_truncated = false;
        self.output_observed_bytes = 0;
        self.session_id = None;
        self.exit_code = None;
    }
}

/// Type implementing [`vt100::Callbacks`], used for capturing terminal scrollback.
struct Scrollback {
    buffer: BoundedBuffer,
    state: vt100::capture::BasicFormattedCaptureState,
    zone: Zone,
}

impl Scrollback {
    pub fn new() -> Self {
        Self {
            buffer: BoundedBuffer::new(MAX_CAPTURE_BYTES),
            state: Default::default(),
            zone: Zone::Unknown,
        }
    }
}

impl vt100::Callbacks for Scrollback {
    fn on_scroll(&mut self, contents: vt100::capture::RowContents<'_>, alternate_screen: bool) {
        if !alternate_screen && self.zone == Zone::Output {
            let _ = contents.write_formatted_basic(&mut self.buffer, &mut self.state);
        }
    }
}

/// Represents rendered terminal output.
struct RenderedOutput {
    /// The terminal data. This may contain SGR escape sequences.
    pub data: String,
    /// Whether the data has been truncated (exceeded the maximum buffer size).
    pub truncated: bool,
}

/// The "core" of a [`CommandCaptureTracker`].
///
/// This is a separate type to satisfy Rust's borrowing rules. [`CommandCaptureTracker::push`] can't
/// call other [`CommandCaptureTracker`] methods while [`CommandCaptureTracker::osc_parser`] is
/// borrowed, so instead we put those methods in a separate type, [`TrackerCore`].
struct TrackerCore {
    capture: CaptureState,
    emulator: vt100::Parser<Scrollback>,
    sink: CommandCaptureSink,
}

impl TrackerCore {
    fn zone(&self) -> Zone {
        self.emulator.callbacks().zone
    }

    fn zone_mut(&mut self) -> &mut Zone {
        &mut self.emulator.callbacks_mut().zone
    }

    /// Capture and return the rendered output currently on the screen.
    ///
    /// This also includes rows that have scrolled off the screen. This method resets the scrollback
    /// buffers but not the screen. Most likely, you will not want to call this method again until
    /// you clear the screen.
    fn take_rendered(&mut self) -> RenderedOutput {
        let (screen, scrollback) = self.emulator.screen_and_callbacks_mut();
        let _ = screen.write_contents_formatted_basic(
            &mut scrollback.buffer,
            vt100::capture::BasicFormattedCaptureRange::Full(&mut scrollback.state),
        );

        let buffer = scrollback.buffer.take();
        scrollback.state = Default::default();
        RenderedOutput {
            truncated: buffer.is_truncated(),
            data: buffer.into_data(),
        }
    }

    /// Clear an in-progress capture and the scrollback buffer.
    fn clear_capture(&mut self) {
        self.capture.clear();
        let scrollback = self.emulator.callbacks_mut();
        scrollback.buffer.clear();
        scrollback.state = Default::default();
    }

    /// Enter a new OSC 133 zone.
    ///
    /// This is a no-op if we're already in that zone.
    fn enter_zone(&mut self, zone: Zone) {
        let current_zone = self.zone();
        if zone == self.zone() {
            return;
        }

        if self.emulator.screen().alternate_screen() {
            // If we're in the alternate screen, leave it. We don't capture anything on the
            // alternate screen (see `Scrollback::on_scroll`). We do not expect to be on the
            // alternate screen when switching zones (something has gone wrong in this case,
            // potentially garbage data), so we disable it here as a last resort, just to ensure
            // we're in a consistent state, and to recover as much of the main screen output as
            // possible.
            self.emulator.process(DISABLE_ALTERNATE_SCREEN);
        }

        if matches!(
            (current_zone, zone),
            (Zone::Unknown, _) | (Zone::Output, Zone::Prompt | Zone::Input)
        ) {
            // If we're coming from the `Unknown` zone, clear the capture to ensure we start in a
            // fresh state -- there could be a stale capture for which we never received a history
            // ID.
            //
            // If we're in the `Output` zone (capturing command output) but we transition directly
            // into `Prompt` or `Input` (starting a new command), also clear the capture. Without a
            // history ID, we can't do anything with it.
            self.clear_capture();
        } else if current_zone == Zone::Output {
            let mut rendered = self.take_rendered();
            // Trim leading and trailing newlines; these correspond to blank lines in the terminal.
            // Don't trim spaces since indentation is meaningful.
            //
            // Note that we will end up trimming leading/trailing space that is technically part of
            // the output itself too, as it cannot be easily distinguished from empty parts of the
            // terminal (in some cases it is effectively impossible).
            rendered.data.trim_matches_in_place('\n');
            self.capture.output = rendered.data;
            self.capture.output_truncated |= rendered.truncated;
        }

        if zone == Zone::Output {
            // Clear the screen before the command starts producing output, so we can obtain just
            // the command's output without confusing it for other data that was already in the
            // terminal.
            self.emulator.process(CLEAR_SCREEN_CONTENTS);
        }
        *self.zone_mut() = zone;
    }

    fn handle_chunk<'a>(&mut self, chunk: EventChunk<'_>, params: impl Iterator<Item = Param<'a>>) {
        let prev_zone = self.zone();
        self.enter_zone(chunk.event.zone());

        let Event::CommandFinished { exit_code } = chunk.event else {
            return;
        };

        let count = &mut self.capture.output_observed_bytes;
        // If we were just in the output zone, the OSC 133 "command finished" bytes were counted
        // toward the total. Correct the count by subtracting them. Note that we cannot safely do
        // this if the count is `u64::MAX` because it might have saturated, so we don't know the
        // true count. This case is exceedingly unlikely however.
        if prev_zone == Zone::Output && *count != u64::MAX {
            *count = count.saturating_sub(u64::try_from(chunk.osc_len).unwrap_or(u64::MAX));
        }

        let mut history_id = None;
        let mut session_id = None;
        for param in params {
            match param {
                Param::KeyValue {
                    key: HISTORY_ID_PARAM,
                    value,
                } => {
                    history_id = Some(value);
                }
                Param::KeyValue {
                    key: SESSION_ID_PARAM,
                    value,
                } => {
                    session_id = Some(value);
                }
                _ => {}
            }
        }

        if let Some(code) = exit_code {
            self.capture.exit_code = Some(code);
        }
        if let Some(id) = session_id {
            self.capture.session_id = Some(String::from_utf8_lossy(id).into_owned());
        }
        let Some(history_id) = history_id
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|s| s.parse::<HistoryId>().ok())
        else {
            // We can't finish the capture without a valid history ID. Hold on to the capture for
            // now in case we get another `CommandFinished` event that does supply one.
            return;
        };

        let state = std::mem::take(&mut self.capture);
        let (rows, cols) = self.emulator.screen().size();
        (self.sink)(CommandCapture {
            output: state.output,
            exit_code: state.exit_code,
            history_id,
            session_id: state.session_id,
            output_observed_bytes: state.output_observed_bytes,
            output_truncated: state.output_truncated,
            terminal_width: cols.get(),
            terminal_height: rows.get(),
        });
    }

    /// Pass data to the vt100 emulator, adding it to the output total if necessary.
    fn process_counted(&mut self, data: &[u8]) {
        self.emulator.process(data);
        if self.zone() == Zone::Output {
            let count = &mut self.capture.output_observed_bytes;
            let data_len = u64::try_from(data.len()).unwrap_or(u64::MAX);
            *count = count.saturating_add(data_len);
        }
    }

    fn handle_chunks(&mut self, mut chunks: EventChunks<'_, '_>) {
        while let Some(chunk) = chunks.next() {
            self.process_counted(chunk.data);
            self.handle_chunk(chunk, chunks.params());
        }
        self.process_counted(chunks.trailing_data());
    }
}

pub struct CommandCaptureTracker {
    osc_parser: osc133::Parser,
    core: TrackerCore,
}

impl CommandCaptureTracker {
    pub fn new(rows: NonZeroU16, cols: NonZeroU16, sink: CommandCaptureSink) -> Self {
        Self {
            osc_parser: osc133::Parser::new(),
            core: TrackerCore {
                capture: CaptureState::default(),
                emulator: vt100::Parser::new_with_callbacks(rows, cols, 0, Scrollback::new()),
                sink,
            },
        }
    }

    pub fn resize(&mut self, rows: NonZeroU16, cols: NonZeroU16) {
        self.core.emulator.screen_mut().set_size(rows, cols);
    }

    pub fn push(&mut self, data: &[u8]) {
        self.core.handle_chunks(self.osc_parser.push(data));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};

    use easy_cast::Conv;
    use rstest::{fixture, rstest};

    use super::*;

    const ROWS: u16 = 24;
    const COLS: u16 = 80;

    const PROMPT_START: &[u8] = b"\x1b]133;A\x07";
    const COMMAND_START: &[u8] = b"\x1b]133;B\x07";
    const COMMAND_EXECUTED: &[u8] = b"\x1b]133;C\x07";

    // The shell integration only ever sends UUIDs, which `HistoryId` now parses, so fixtures
    // use real ones.
    const HID: &str = "00000000-0000-0000-0000-0000000000a1";
    const HID_ONE: &str = "00000000-0000-0000-0000-000000000001";
    const HID_TWO: &str = "00000000-0000-0000-0000-000000000002";

    fn hid(s: &str) -> HistoryId {
        s.parse().expect("valid history id")
    }

    /// A [`CommandCaptureTracker`] together with the captures its sink has been handed.
    struct Tracker {
        inner: CommandCaptureTracker,
        received: Receiver<CommandCapture>,
        collected: Vec<CommandCapture>,
    }

    impl Tracker {
        fn new(rows: u16, cols: u16) -> Self {
            let (sender, received) = mpsc::channel();
            Self {
                inner: CommandCaptureTracker::new(
                    nonzero(rows),
                    nonzero(cols),
                    Box::new(move |capture| {
                        sender.send(capture).expect("test receiver is still alive");
                    }),
                ),
                received,
                collected: Vec::new(),
            }
        }

        fn push(&mut self, data: &[u8]) -> &mut Self {
            self.inner.push(data);
            self
        }

        fn resize(&mut self, rows: u16, cols: u16) -> &mut Self {
            self.inner.resize(nonzero(rows), nonzero(cols));
            self
        }

        /// Every capture reported so far.
        fn captures(&mut self) -> Vec<CommandCapture> {
            self.collected.extend(self.received.try_iter());
            self.collected.clone()
        }

        /// Assert that exactly one capture was reported, and return it.
        fn only_capture(&mut self) -> CommandCapture {
            let mut captures = self.captures();
            assert_eq!(captures.len(), 1, "expected exactly one capture, got {captures:#?}");
            captures.pop().expect("length checked above")
        }
    }

    fn nonzero(value: u16) -> NonZeroU16 {
        NonZeroU16::new(value).expect("test dimensions are non-zero")
    }

    /// A `D` marker carrying the metadata Atuin's shell integration sends.
    ///
    /// Its own bytes are discounted from `output_observed_bytes`, so the totals asserted
    /// below are the raw command output alone.
    fn finished(exit_code: i32, history_id: &str, session_id: &str) -> Vec<u8> {
        format!("\x1b]133;D;{exit_code};history_id={history_id};session_id={session_id}\x07")
            .into_bytes()
    }

    /// A whole shell interaction, laid out the way a real shell emits it: the prompt, the
    /// echoed command line ending in the newline the shell prints when Enter is pressed,
    /// then the command's output.
    fn interaction(prompt: &str, command: &str, output: &str) -> Vec<u8> {
        [
            PROMPT_START,
            prompt.as_bytes(),
            COMMAND_START,
            command.as_bytes(),
            b"\r\n",
            COMMAND_EXECUTED,
            output.as_bytes(),
            &finished(0, HID, "sess"),
        ]
        .concat()
    }

    #[fixture]
    fn tracker(#[default(ROWS)] rows: u16, #[default(COLS)] cols: u16) -> Tracker {
        Tracker::new(rows, cols)
    }

    // -- The happy path -------------------------------------------------------

    #[rstest]
    #[case::full_interaction(
        interaction("$ ", "echo hi", "hi\r\n"),
        CommandCapture {
            output: "hi".to_string(),
            exit_code: Some(0),
            history_id: hid(HID),
            session_id: Some("sess".to_string()),
            output_observed_bytes: u64::conv(b"hi\r\n".len()),
            output_truncated: false,
            terminal_width: COLS,
            terminal_height: ROWS,
        },
    )]
    // Only the execute and finish markers: no prompt or command line in the stream at all.
    #[case::bare_execute_and_finish_markers(
        [COMMAND_EXECUTED, b"line one\r\n", &finished(0, HID, "abcd")].concat(),
        CommandCapture {
            output: "line one".to_string(),
            exit_code: Some(0),
            history_id: hid(HID),
            session_id: Some("abcd".to_string()),
            output_observed_bytes: u64::conv(b"line one\r\n".len()),
            output_truncated: false,
            terminal_width: COLS,
            terminal_height: ROWS,
        },
    )]
    fn captures_a_full_command_cycle(
        mut tracker: Tracker,
        #[case] input: Vec<u8>,
        #[case] expected: CommandCapture,
    ) {
        tracker.push(&input);
        assert_eq!(tracker.only_capture(), expected);
    }

    #[rstest]
    // A command that prints nothing.
    #[case::no_output(interaction("$ ", "true", ""))]
    // Enter pressed on an empty prompt.
    #[case::empty_command(interaction("$ ", "", ""))]
    fn a_command_with_no_output_is_still_captured(mut tracker: Tracker, #[case] input: Vec<u8>) {
        tracker.push(&input);

        let capture = tracker.only_capture();
        assert_eq!(capture.output, "");
        assert_eq!(capture.history_id, hid(HID));
        // The command produced nothing, and its `D` marker doesn't count as output.
        assert_eq!(capture.output_observed_bytes, 0);
    }

    #[rstest]
    fn reports_the_exit_code(mut tracker: Tracker) {
        tracker.push(&[COMMAND_EXECUTED, b"nope\r\n", &finished(127, HID, "sess")].concat());

        assert_eq!(tracker.only_capture().exit_code, Some(127));
    }

    #[rstest]
    #[case::no_markers(b"just some regular terminal output\r\n".to_vec())]
    // A finish marker with no history ID can't be attached to anything, and the prompt that
    // follows means no later marker can supply one either.
    #[case::finish_without_a_history_id(
        [COMMAND_EXECUTED, b"line one\r\n\x1b]133;D;0\x07", PROMPT_START, b"$ "].concat()
    )]
    fn reports_nothing(mut tracker: Tracker, #[case] input: Vec<u8>) {
        tracker.push(&input);
        assert_eq!(tracker.captures(), vec![]);
    }

    // -- Rendering ------------------------------------------------------------

    #[rstest]
    // The whole point of driving a terminal emulator: output that moves the cursor to an
    // absolute position is captured as it appears, not as it was written.
    #[case::absolute_cursor_movement(b"one\r\ntwo\r\n\x1b[1;1Hzzz\r\n", "zzz\ntwo")]
    // Progress bars redraw the same line over and over.
    #[case::carriage_returns_overwrite_in_place(b"  0%\r 50%\r100%\r\n", "100%")]
    #[case::backspaces_erase(b"oops\x08\x08\x08\x08done\r\n", "done")]
    #[case::erase_to_end_of_line(b"long line\r\x1b[Kshort\r\n", "short")]
    fn output_is_rendered_not_replayed(
        #[with(6, 20)] mut tracker: Tracker,
        #[case] output: &[u8],
        #[case] expected: &str,
    ) {
        tracker.push(&[COMMAND_EXECUTED, output, &finished(0, HID, "sess")].concat());
        assert_eq!(tracker.only_capture().output, expected);
    }

    #[rstest]
    fn output_that_scrolls_off_the_screen_is_kept(#[with(4, 20)] mut tracker: Tracker) {
        let mut data = COMMAND_EXECUTED.to_vec();
        for i in 0..10 {
            data.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        data.extend_from_slice(&finished(0, HID, "sess"));
        tracker.push(&data);

        let expected: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        assert_eq!(tracker.only_capture().output, expected.join("\n"));
    }

    #[rstest]
    fn keeps_basic_formatting(mut tracker: Tracker) {
        tracker.push(&interaction("\x1b[32m%\x1b[0m ", "ls", "\x1b[31mfile\x1b[0m\r\n"));

        assert_eq!(tracker.only_capture().output, "\x1b[31mfile");
    }

    #[rstest]
    fn attributes_left_set_do_not_fill_the_capture_with_blanks(
        #[with(6, 20)] mut tracker: Tracker,
    ) {
        // Erasing the screen at the start of the output zone uses the current attributes, so a
        // command that leaves a background colour set would otherwise turn the whole screen
        // into non-default cells, and every one of them into a space in the next capture.
        tracker
            .push(&[COMMAND_EXECUTED, b"out\r\n\x1b[41m", &finished(0, HID, "sess")].concat())
            .push(&interaction("$ ", "id", "\x1b[0mok\r\n"));

        let captures = tracker.captures();
        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0].output, "out");
        assert_eq!(captures[1].output, "ok");
    }

    #[rstest]
    fn alternate_screen_output_is_not_captured(#[with(6, 20)] mut tracker: Tracker) {
        tracker.push(
            &[
                PROMPT_START,
                b"$ ",
                COMMAND_START,
                b"vim f\r\n",
                COMMAND_EXECUTED,
                b"\x1b[?1049hEDITOR\r\nSCREEN\x1b[?1049l",
                &finished(0, HID, "sess"),
            ]
            .concat(),
        );

        let capture = tracker.only_capture();
        assert_eq!(capture.output, "");
        // The bytes were still observed, even though none of them were captured.
        let drawn = b"\x1b[?1049hEDITOR\r\nSCREEN\x1b[?1049l".len();
        assert_eq!(capture.output_observed_bytes, u64::conv(drawn));
    }

    #[rstest]
    fn output_still_in_the_alternate_screen_is_not_captured(#[with(6, 20)] mut tracker: Tracker) {
        // The output zone always begins on the main screen, but it can end on the alternate one
        // if the command never leaves it. Leaving the alternate screen has to happen before the
        // output is stored, or the capture is of the alternate screen's contents.
        tracker.push(
            &[
                PROMPT_START,
                b"$ ",
                COMMAND_START,
                b"vim f\r\n",
                COMMAND_EXECUTED,
                b"\x1b[?1049hEDITOR\r\nSCREEN",
                &finished(0, HID, "sess"),
            ]
            .concat(),
        );

        assert_eq!(tracker.only_capture().output, "");
    }

    #[rstest]
    fn the_output_zone_never_sees_what_the_prompt_drew(#[with(6, 20)] mut tracker: Tracker) {
        tracker
            .push(&interaction("$ ", "first", "aaa\r\n"))
            .push(&interaction("$ ", "second", "bbb\r\n"));

        let captures = tracker.captures();
        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0].output, "aaa");
        assert_eq!(captures[1].output, "bbb");
    }

    #[rstest]
    fn a_long_prompt_does_not_leak_into_the_output(#[with(4, 20)] mut tracker: Tracker) {
        // A prompt and command line long enough to scroll the screen. The output capture is
        // built from the scrollback buffer plus the screen, so rows that scroll away outside the
        // output zone must never reach that buffer in the first place.
        let long = "p".repeat(4 * 20);
        tracker.push(&interaction(&long, &long, "hi\r\n"));

        assert_eq!(tracker.only_capture().output, "hi");
    }

    #[rstest]
    fn resets_between_consecutive_bare_command_cycles(mut tracker: Tracker) {
        tracker.push(
            &[
                COMMAND_EXECUTED,
                b"first\r\n",
                &finished(0, HID_ONE, "sess"),
                COMMAND_EXECUTED,
                b"second\r\n",
                &finished(1, HID_TWO, "sess"),
            ]
            .concat(),
        );

        let captures = tracker.captures();
        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0].output, "first");
        assert_eq!(captures[0].exit_code, Some(0));
        assert_eq!(captures[0].history_id, hid(HID_ONE));
        assert_eq!(captures[1].output, "second");
        assert_eq!(captures[1].exit_code, Some(1));
        assert_eq!(captures[1].history_id, hid(HID_TWO));
    }

    // -- Marker handling ------------------------------------------------------

    #[rstest]
    fn a_repeated_prompt_marker_is_tolerated(mut tracker: Tracker) {
        tracker.push(
            &[
                PROMPT_START,
                b"$ ",
                PROMPT_START,
                b"continued ",
                COMMAND_START,
                b"echo hi\r\n",
                COMMAND_EXECUTED,
                b"hi\r\n",
                &finished(0, HID, "sess"),
            ]
            .concat(),
        );

        assert_eq!(tracker.only_capture().output, "hi");
    }

    #[rstest]
    fn a_command_marker_ahead_of_its_prompt_marker_is_tolerated(mut tracker: Tracker) {
        // Some shells get the order wrong and mark the command line before the prompt. Entering
        // the prompt zone from the input zone therefore keeps the capture, and neither zone
        // contributes anything to it.
        tracker.push(
            &[
                COMMAND_START,
                b"leftover\r\n",
                PROMPT_START,
                b"$ ",
                COMMAND_START,
                b"echo hi\r\n",
                COMMAND_EXECUTED,
                b"hi\r\n",
                &finished(0, HID, "sess"),
            ]
            .concat(),
        );

        assert_eq!(tracker.only_capture().output, "hi");
    }

    #[rstest]
    fn a_new_prompt_abandons_an_unreported_capture(mut tracker: Tracker) {
        // The first command finishes without metadata, so it is never reported. The next
        // prompt must not inherit any of it.
        tracker
            .push(&[COMMAND_EXECUTED, b"stale\r\n\x1b]133;D;0\x07"].concat())
            .push(&interaction("$ ", "echo hi", "hi\r\n"));

        let capture = tracker.only_capture();
        assert_eq!(capture.output, "hi");
        assert_eq!(capture.output_observed_bytes, u64::conv(b"hi\r\n".len()));
    }

    #[rstest]
    fn an_abandoned_command_leaves_nothing_on_the_screen(#[with(4, 20)] mut tracker: Tracker) {
        // No `D` marker at all: the command is abandoned when the next prompt starts. Its rows
        // have scrolled off the screen, so dropping the capture is not enough on its own -- the
        // buffer they scrolled into has to be dropped with it.
        let mut abandoned = COMMAND_EXECUTED.to_vec();
        for i in 0..8 {
            abandoned.extend_from_slice(format!("stale {i}\r\n").as_bytes());
        }
        tracker.push(&abandoned).push(&interaction("$ ", "echo hi", "hi\r\n"));

        assert_eq!(tracker.only_capture().output, "hi");
    }

    #[rstest]
    fn an_unreported_capture_is_abandoned_by_a_bare_command_cycle(mut tracker: Tracker) {
        // Same as `a_new_prompt_abandons_an_unreported_capture`, but the next command arrives
        // without a prompt, straight from the unknown zone.
        tracker.push(
            &[
                COMMAND_EXECUTED,
                b"stale\r\n\x1b]133;D;0\x07",
                COMMAND_EXECUTED,
                b"fresh\r\n",
                &finished(0, HID, "sess"),
            ]
            .concat(),
        );

        let capture = tracker.only_capture();
        assert_eq!(capture.output, "fresh");
        assert_eq!(capture.output_observed_bytes, u64::conv(b"fresh\r\n".len()));
    }

    #[rstest]
    fn metadata_from_a_later_finish_marker_is_used(mut tracker: Tracker) {
        const BARE_FINISH: &[u8] = b"\x1b]133;D;1\x07";
        tracker.push(
            &[COMMAND_EXECUTED, b"line one\r\n", BARE_FINISH, &finished(0, HID, "abcd")].concat(),
        );

        assert_eq!(tracker.only_capture(), CommandCapture {
            output: "line one".to_string(),
            exit_code: Some(0),
            history_id: hid(HID),
            session_id: Some("abcd".to_string()),
            // The first `D` ends the output zone and is discounted; the second arrives
            // after it, in the unknown zone, so it was never counted to begin with.
            output_observed_bytes: u64::conv(b"line one\r\n".len()),
            output_truncated: false,
            terminal_width: COLS,
            terminal_height: ROWS,
        });
    }

    #[rstest]
    fn a_marker_split_across_pushes_is_still_recognised(mut tracker: Tracker) {
        tracker.push(
            &[COMMAND_EXECUTED, format!("line one\r\n\x1b]133;D;0;history_id={HID}").as_bytes()]
                .concat(),
        );
        assert_eq!(tracker.captures(), vec![]);

        tracker.push(b";session_id=abcd\x07");

        let capture = tracker.only_capture();
        assert_eq!(capture.output, "line one");
        assert_eq!(capture.history_id, hid(HID));
        assert_eq!(capture.session_id.as_deref(), Some("abcd"));
    }

    #[rstest]
    fn st_terminated_markers_do_not_leak_into_the_capture(mut tracker: Tracker) {
        // A marker has to reach the emulator whole. Handing it over with its `ESC \\`
        // terminator split off would leave the emulator mid-sequence, and the stray
        // backslash would end up printed on the screen we capture.
        tracker.push(
            format!(
                "\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\echo \
                 hi\r\n\x1b]133;C\x1b\\hi\r\n\x1b]133;D;0;history_id={HID}\x1b\\"
            )
            .as_bytes(),
        );

        let capture = tracker.only_capture();
        assert_eq!(capture.output, "hi");
        assert_eq!(capture.output_observed_bytes, u64::conv(b"hi\r\n".len()));
    }

    #[rstest]
    fn splitting_a_marker_across_pushes_does_not_change_the_byte_count(mut tracker: Tracker) {
        tracker
            .push(
                &[
                    COMMAND_EXECUTED,
                    format!("line one\r\n\x1b]133;D;0;history_id={HID}").as_bytes(),
                ]
                .concat(),
            )
            .push(b";session_id=abcd\x07");

        // The same total as if the whole marker had arrived in one push: the marker is
        // discounted in full, however it was split up.
        assert_eq!(tracker.only_capture().output_observed_bytes, u64::conv(b"line one\r\n".len()));
    }

    #[rstest]
    fn markers_split_at_every_byte_boundary(mut tracker: Tracker) {
        let input = interaction("$ ", "echo hi", "hi\r\n");
        for byte in &input {
            tracker.push(std::slice::from_ref(byte));
        }

        let capture = tracker.only_capture();
        assert_eq!(capture.output, "hi");
        assert_eq!(capture.output_observed_bytes, u64::conv(b"hi\r\n".len()));
    }

    // -- Limits ---------------------------------------------------------------

    #[rstest]
    fn output_capture_is_capped_and_reports_observed_bytes(mut tracker: Tracker) {
        const LINE_LEN: usize = 70;
        const LINES: usize = 40_000;

        let finish = finished(0, HID, "session-1");
        let mut input = COMMAND_EXECUTED.to_vec();
        for i in 0..LINES {
            input.extend_from_slice(format!("{i:0LINE_LEN$}\r\n").as_bytes());
        }
        input.extend_from_slice(&finish);
        tracker.push(&input);

        let capture = tracker.only_capture();
        assert!(capture.output_truncated);
        assert_eq!(capture.output.len(), MAX_CAPTURE_BYTES);
        assert_eq!(capture.output_observed_bytes, u64::conv((LINE_LEN + 2) * LINES));
    }

    // -- Terminal size --------------------------------------------------------

    #[rstest]
    fn resizing_reflows_the_capture(#[with(6, 20)] mut tracker: Tracker) {
        tracker.push(&[COMMAND_EXECUTED, b"abcdefghij"].concat());
        tracker.resize(6, 5);
        tracker.push(&[b"klmno\r\n".as_slice(), &finished(0, HID, "sess")].concat());

        // The first ten columns were rendered on a twenty-column screen; narrowing it drops
        // what no longer fits, and the rest is appended at the new width.
        assert_eq!(tracker.only_capture().output, "abcdklmno");
    }

    #[rstest]
    fn tiny_terminals_do_not_panic(#[values(1, 2, 3)] rows: u16, #[values(1, 2, 3)] cols: u16) {
        let mut tracker = Tracker::new(rows, cols);
        tracker.push(&interaction("$ ", "echo hi", "hello world\r\nsecond line\r\n"));

        // Whatever survives on a terminal this small, we must have reported something.
        assert_eq!(tracker.captures().len(), 1);
    }
}
