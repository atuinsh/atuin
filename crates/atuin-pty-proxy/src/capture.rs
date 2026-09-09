use std::num::NonZeroU16;

pub use atuin_client::history::CommandCapture;
use atuin_client::history::HistoryId;
use atuin_common::string::TrimExt as _;
use atuin_common::string::bounded_buffer::{self, BoundedBuffer, BufferContents};

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

pub type CommandCaptureSink = Box<dyn Fn(HistoryId, CommandCapture) + Send + 'static>;

/// Configuration for a `CommandCaptureTracker`: where captures go, and how large a single
/// command's captured output may grow before its middle is dropped.
pub struct CaptureConfig {
    pub sink: CommandCaptureSink,
    /// The total number of bytes kept for a single command, split evenly across the start and
    /// end of its output once the middle has to be dropped.
    pub max_output_bytes: usize,
}

/// The state of an in-progress command capture.
#[derive(Default)]
struct CaptureState {
    output_start: String,
    output_end: Option<String>,
    output_observed_bytes: u64,
}

/// Type implementing [`vt100::Callbacks`], used for capturing terminal scrollback.
struct Scrollback {
    buffer: BoundedBuffer,
    state: vt100::capture::BasicFormattedCaptureState,
    zone: Zone,
}

impl Scrollback {
    pub fn new(limit: bounded_buffer::Limit) -> Self {
        Self {
            buffer: BoundedBuffer::new(limit),
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
    fn take_rendered(&mut self) -> BufferContents {
        let (screen, scrollback) = self.emulator.screen_and_callbacks_mut();
        let _ = screen.write_contents_formatted_basic(
            &mut scrollback.buffer,
            vt100::capture::BasicFormattedCaptureRange::Full(&mut scrollback.state),
        );

        scrollback.state = Default::default();
        scrollback.buffer.take()
    }

    /// Clear an in-progress capture and the scrollback buffer.
    fn clear_capture(&mut self) {
        self.capture = CaptureState::default();
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
            let mut contents = self.take_rendered();
            // Trim leading and trailing newlines; these correspond to blank lines in the terminal.
            // Don't trim spaces since indentation is meaningful.
            //
            // Note that we will end up trimming leading/trailing space that is technically part of
            // the output itself too, as it cannot be easily distinguished from empty parts of the
            // terminal (in some cases it is effectively impossible).
            if let Some(end) = &mut contents.end {
                // Technically we could fail to trim all the relevant blank lines here if the
                // terminal height is greater than the start limit and end limit combined --
                // trailing blank lines, for example, would get split across the start and end
                // chunks, but we wouldn't trim the trailing newlines from the start chunk. This is
                // because we wouldn't know whether the missing middle chunk consisted entirely of
                // blank lines or had other data (which would make the trailing newlines in the
                // start chunk actually part of the command output and thus something we *shouldn't*
                // trim).
                //
                // This case is very unlikely in practice, as we expect the output capture limits to
                // significantly exceed the terminal height -- otherwise not much useful information
                // could actually be captured. In any case, we err on the side of keeping "too much"
                // data rather than discarding it.
                end.trim_end_matches_in_place('\n');
                contents.start.trim_start_matches_in_place('\n');

                // Ensure the start chunk doesn't end in the middle of a line, and the end chunk
                // doesn't start in the middle of a line. This is not just to make the output nicer
                // but is also important for secret redaction -- if a secret got split across the
                // start and end chunks, we would fail to redact it later. For example, if the
                // output of a command were one byte over the limit and we happened to truncate the
                // `=` in `...AWS_SECRET_ACCESS_KEY=SOME_SECRET_VALUE...`, we would fail to redact
                // the secret. Removing partial lines from the chunks avoids the issue.
                contents.start.truncate(contents.start.rfind('\n').unwrap_or(0));
                end.drain(..end.find('\n').map_or(end.len(), |n| n + 1));
            } else {
                contents.start.trim_matches_in_place('\n');
            }
            self.capture.output_start = contents.start;
            self.capture.output_end = contents.end;
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

        let Event::CommandFinished { .. } = chunk.event else {
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
        for param in params {
            if let Param::KeyValue {
                key: HISTORY_ID_PARAM,
                value,
            } = param
            {
                history_id = Some(value);
            }
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
        (self.sink)(history_id, CommandCapture {
            output_start: state.output_start,
            output_end: state.output_end,
            output_observed_bytes: state.output_observed_bytes,
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
    pub fn new(rows: NonZeroU16, cols: NonZeroU16, config: CaptureConfig) -> Self {
        let CaptureConfig {
            sink,
            max_output_bytes,
        } = config;
        Self {
            osc_parser: osc133::Parser::new(),
            core: TrackerCore {
                capture: CaptureState::default(),
                emulator: vt100::Parser::new_with_callbacks(
                    rows,
                    cols,
                    0,
                    Scrollback::new(bounded_buffer::Limit::split_evenly(max_output_bytes)),
                ),
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

    /// Roomy enough that nothing below is truncated unless the case asks for it.
    const LIMIT: usize = 128 * 1024;

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
        received: Receiver<(HistoryId, CommandCapture)>,
        collected: Vec<(HistoryId, CommandCapture)>,
    }

    impl Tracker {
        fn new(rows: u16, cols: u16, max_output_bytes: usize) -> Self {
            let (sender, received) = mpsc::channel();
            Self {
                inner: CommandCaptureTracker::new(nonzero(rows), nonzero(cols), CaptureConfig {
                    sink: Box::new(move |history_id, capture| {
                        sender.send((history_id, capture)).expect("test receiver is still alive");
                    }),
                    max_output_bytes,
                }),
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

        /// Every `(history id, capture)` reported so far.
        fn captures(&mut self) -> Vec<(HistoryId, CommandCapture)> {
            self.collected.extend(self.received.try_iter());
            self.collected.clone()
        }

        /// Assert that exactly one capture was reported, and return it with its history id.
        fn only_capture(&mut self) -> (HistoryId, CommandCapture) {
            let mut captures = self.captures();
            assert_eq!(captures.len(), 1, "expected exactly one capture, got {captures:#?}");
            captures.pop().expect("length checked above")
        }
    }

    fn nonzero(value: u16) -> NonZeroU16 {
        NonZeroU16::new(value).expect("test dimensions are non-zero")
    }

    /// The rendered output of a capture that kept all of it.
    ///
    /// Most cases below are far too short to be truncated, and an `output_end` appearing where
    /// one is not expected is itself a failure -- so assert that here rather than in every case.
    fn whole_output(capture: &CommandCapture) -> &str {
        assert_eq!(capture.output_end, None, "expected an untruncated capture");
        &capture.output_start
    }

    /// A `D` marker carrying the metadata Atuin's shell integration sends.
    ///
    /// Its own bytes are discounted from `output_observed_bytes`, so the totals asserted
    /// below are the raw command output alone.
    fn finished(exit_code: i32, history_id: &str) -> Vec<u8> {
        format!("\x1b]133;D;{exit_code};history_id={history_id}\x07").into_bytes()
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
            &finished(0, HID),
        ]
        .concat()
    }

    #[fixture]
    fn tracker(
        #[default(ROWS)] rows: u16,
        #[default(COLS)] cols: u16,
        #[default(LIMIT)] max_output_bytes: usize,
    ) -> Tracker {
        Tracker::new(rows, cols, max_output_bytes)
    }

    // -- The happy path -------------------------------------------------------

    #[rstest]
    #[case::full_interaction(
        interaction("$ ", "echo hi", "hi\r\n"),
        CommandCapture {
            output_start: "hi".to_string(),
            output_end: None,
            output_observed_bytes: u64::conv(b"hi\r\n".len()),
            terminal_width: COLS,
            terminal_height: ROWS,
        },
    )]
    // Only the execute and finish markers: no prompt or command line in the stream at all.
    #[case::bare_execute_and_finish_markers(
        [COMMAND_EXECUTED, b"line one\r\n", &finished(0, HID)].concat(),
        CommandCapture {
            output_start: "line one".to_string(),
            output_end: None,
            output_observed_bytes: u64::conv(b"line one\r\n".len()),
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
        let (history_id, capture) = tracker.only_capture();
        assert_eq!(history_id, hid(HID));
        assert_eq!(capture, expected);
    }

    #[rstest]
    // A command that prints nothing.
    #[case::no_output(interaction("$ ", "true", ""))]
    // Enter pressed on an empty prompt.
    #[case::empty_command(interaction("$ ", "", ""))]
    fn a_command_with_no_output_is_still_captured(mut tracker: Tracker, #[case] input: Vec<u8>) {
        tracker.push(&input);

        let (history_id, capture) = tracker.only_capture();
        assert_eq!(whole_output(&capture), "");
        assert_eq!(history_id, hid(HID));
        // The command produced nothing, and its `D` marker doesn't count as output.
        assert_eq!(capture.output_observed_bytes, 0);
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
        tracker.push(&[COMMAND_EXECUTED, output, &finished(0, HID)].concat());
        assert_eq!(whole_output(&tracker.only_capture().1), expected);
    }

    #[rstest]
    fn output_that_scrolls_off_the_screen_is_kept(#[with(4, 20)] mut tracker: Tracker) {
        let mut data = COMMAND_EXECUTED.to_vec();
        for i in 0..10 {
            data.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        data.extend_from_slice(&finished(0, HID));
        tracker.push(&data);

        let expected: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        assert_eq!(whole_output(&tracker.only_capture().1), expected.join("\n"));
    }

    #[rstest]
    fn keeps_basic_formatting(mut tracker: Tracker) {
        tracker.push(&interaction("\x1b[32m%\x1b[0m ", "ls", "\x1b[31mfile\x1b[0m\r\n"));

        assert_eq!(whole_output(&tracker.only_capture().1), "\x1b[31mfile");
    }

    #[rstest]
    fn attributes_left_set_do_not_fill_the_capture_with_blanks(
        #[with(6, 20)] mut tracker: Tracker,
    ) {
        // Erasing the screen at the start of the output zone uses the current attributes, so a
        // command that leaves a background colour set would otherwise turn the whole screen
        // into non-default cells, and every one of them into a space in the next capture.
        tracker
            .push(&[COMMAND_EXECUTED, b"out\r\n\x1b[41m", &finished(0, HID)].concat())
            .push(&interaction("$ ", "id", "\x1b[0mok\r\n"));

        let captures = tracker.captures();
        assert_eq!(captures.len(), 2);
        assert_eq!(whole_output(&captures[0].1), "out");
        assert_eq!(whole_output(&captures[1].1), "ok");
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
                &finished(0, HID),
            ]
            .concat(),
        );

        let capture = tracker.only_capture().1;
        assert_eq!(whole_output(&capture), "");
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
                &finished(0, HID),
            ]
            .concat(),
        );

        assert_eq!(whole_output(&tracker.only_capture().1), "");
    }

    #[rstest]
    fn the_output_zone_never_sees_what_the_prompt_drew(#[with(6, 20)] mut tracker: Tracker) {
        tracker
            .push(&interaction("$ ", "first", "aaa\r\n"))
            .push(&interaction("$ ", "second", "bbb\r\n"));

        let captures = tracker.captures();
        assert_eq!(captures.len(), 2);
        assert_eq!(whole_output(&captures[0].1), "aaa");
        assert_eq!(whole_output(&captures[1].1), "bbb");
    }

    #[rstest]
    fn a_long_prompt_does_not_leak_into_the_output(#[with(4, 20)] mut tracker: Tracker) {
        // A prompt and command line long enough to scroll the screen. The output capture is
        // built from the scrollback buffer plus the screen, so rows that scroll away outside the
        // output zone must never reach that buffer in the first place.
        let long = "p".repeat(4 * 20);
        tracker.push(&interaction(&long, &long, "hi\r\n"));

        assert_eq!(whole_output(&tracker.only_capture().1), "hi");
    }

    #[rstest]
    fn resets_between_consecutive_bare_command_cycles(mut tracker: Tracker) {
        tracker.push(
            &[
                COMMAND_EXECUTED,
                b"first\r\n",
                &finished(0, HID_ONE),
                COMMAND_EXECUTED,
                b"second\r\n",
                &finished(1, HID_TWO),
            ]
            .concat(),
        );

        let captures = tracker.captures();
        assert_eq!(captures.len(), 2);
        assert_eq!(whole_output(&captures[0].1), "first");
        assert_eq!(captures[0].0, hid(HID_ONE));
        assert_eq!(whole_output(&captures[1].1), "second");
        assert_eq!(captures[1].0, hid(HID_TWO));
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
                &finished(0, HID),
            ]
            .concat(),
        );

        assert_eq!(whole_output(&tracker.only_capture().1), "hi");
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
                &finished(0, HID),
            ]
            .concat(),
        );

        assert_eq!(whole_output(&tracker.only_capture().1), "hi");
    }

    #[rstest]
    fn a_new_prompt_abandons_an_unreported_capture(mut tracker: Tracker) {
        // The first command finishes without metadata, so it is never reported. The next
        // prompt must not inherit any of it.
        tracker
            .push(&[COMMAND_EXECUTED, b"stale\r\n\x1b]133;D;0\x07"].concat())
            .push(&interaction("$ ", "echo hi", "hi\r\n"));

        let capture = tracker.only_capture().1;
        assert_eq!(whole_output(&capture), "hi");
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

        assert_eq!(whole_output(&tracker.only_capture().1), "hi");
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
                &finished(0, HID),
            ]
            .concat(),
        );

        let capture = tracker.only_capture().1;
        assert_eq!(whole_output(&capture), "fresh");
        assert_eq!(capture.output_observed_bytes, u64::conv(b"fresh\r\n".len()));
    }

    #[rstest]
    fn metadata_from_a_later_finish_marker_is_used(mut tracker: Tracker) {
        const BARE_FINISH: &[u8] = b"\x1b]133;D;1\x07";
        tracker.push(&[COMMAND_EXECUTED, b"line one\r\n", BARE_FINISH, &finished(0, HID)].concat());

        let (history_id, capture) = tracker.only_capture();
        assert_eq!(history_id, hid(HID));
        assert_eq!(capture, CommandCapture {
            output_start: "line one".to_string(),
            output_end: None,
            // The first `D` ends the output zone and is discounted; the second arrives
            // after it, in the unknown zone, so it was never counted to begin with.
            output_observed_bytes: u64::conv(b"line one\r\n".len()),
            terminal_width: COLS,
            terminal_height: ROWS,
        });
    }

    #[rstest]
    fn a_marker_split_across_pushes_is_still_recognised(mut tracker: Tracker) {
        // Split the marker partway through the history ID, so neither push holds a whole one.
        let (head, tail) = HID.split_at(20);
        tracker.push(
            &[COMMAND_EXECUTED, format!("line one\r\n\x1b]133;D;0;history_id={head}").as_bytes()]
                .concat(),
        );
        assert_eq!(tracker.captures(), vec![]);

        tracker.push(format!("{tail}\x07").as_bytes());

        let (history_id, capture) = tracker.only_capture();
        assert_eq!(whole_output(&capture), "line one");
        assert_eq!(history_id, hid(HID));
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

        let capture = tracker.only_capture().1;
        assert_eq!(whole_output(&capture), "hi");
        assert_eq!(capture.output_observed_bytes, u64::conv(b"hi\r\n".len()));
    }

    #[rstest]
    fn splitting_a_marker_across_pushes_does_not_change_the_byte_count(mut tracker: Tracker) {
        let (head, tail) = HID.split_at(20);
        tracker
            .push(
                &[
                    COMMAND_EXECUTED,
                    format!("line one\r\n\x1b]133;D;0;history_id={head}").as_bytes(),
                ]
                .concat(),
            )
            .push(format!("{tail}\x07").as_bytes());

        // The same total as if the whole marker had arrived in one push: the marker is
        // discounted in full, however it was split up.
        assert_eq!(
            tracker.only_capture().1.output_observed_bytes,
            u64::conv(b"line one\r\n".len())
        );
    }

    #[rstest]
    fn markers_split_at_every_byte_boundary(mut tracker: Tracker) {
        let input = interaction("$ ", "echo hi", "hi\r\n");
        for byte in &input {
            tracker.push(std::slice::from_ref(byte));
        }

        let capture = tracker.only_capture().1;
        assert_eq!(whole_output(&capture), "hi");
        assert_eq!(capture.output_observed_bytes, u64::conv(b"hi\r\n".len()));
    }

    // -- Limits ---------------------------------------------------------------

    /// A limit small enough to overflow within a test, but wider than the terminal so that whole
    /// lines land either side of the cut.
    const SMALL_LIMIT: usize = 600;

    /// Numbered lines, one per row, so any kept fragment says where in the output it came from.
    fn numbered_lines(count: usize) -> Vec<u8> {
        let mut input = COMMAND_EXECUTED.to_vec();
        for i in 0..count {
            input.extend_from_slice(format!("line {i:04}\r\n").as_bytes());
        }
        input.extend_from_slice(&finished(0, HID));
        input
    }

    #[rstest]
    fn oversized_output_keeps_its_start_and_its_end(
        #[with(ROWS, COLS, SMALL_LIMIT)] mut tracker: Tracker,
    ) {
        const LINES: usize = 500;
        tracker.push(&numbered_lines(LINES));

        let capture = tracker.only_capture().1;
        let end = capture.output_end.expect("output this long has to lose its middle");

        // Neither half outgrows its budget -- half the total, split evenly...
        assert!(capture.output_start.len() <= SMALL_LIMIT / 2, "{:?}", capture.output_start);
        assert!(end.len() <= SMALL_LIMIT / 2, "{end:?}");
        // ...the start really is the beginning of the output...
        assert!(capture.output_start.starts_with("line 0000\n"), "{:?}", capture.output_start);
        // ...and the end really is the end of it. Keeping only the first bytes, as the capture
        // used to, threw away exactly the part a user is most likely to want.
        assert!(end.ends_with(&format!("line {:04}", LINES - 1)), "{end:?}");
        // The middle is what went, so the two halves cannot account for every line.
        assert!(capture.output_start.lines().count() + end.lines().count() < LINES);
        // The byte count is of everything the terminal saw, not of what was kept.
        assert_eq!(capture.output_observed_bytes, u64::conv(LINES * b"line 0000\r\n".len()));
    }

    #[rstest]
    fn output_that_fits_the_limit_is_not_split(
        #[with(ROWS, COLS, SMALL_LIMIT)] mut tracker: Tracker,
    ) {
        // Ten numbered lines are 100 rendered bytes, well inside the 600 the limit allows.
        const LINES: usize = 10;
        tracker.push(&numbered_lines(LINES));

        let capture = tracker.only_capture().1;
        let expected: Vec<String> = (0..LINES).map(|i| format!("line {i:04}")).collect();
        assert_eq!(whole_output(&capture), expected.join("\n"));
    }

    #[rstest]
    fn a_capture_that_lost_its_middle_still_reports_every_byte_observed(
        #[with(ROWS, COLS, 0)] mut tracker: Tracker,
    ) {
        // Nothing can be kept at all, but the observed-byte count is independent of the limit --
        // it is what the terminal saw, before rendering and before any truncation.
        const LINES: usize = 50;
        tracker.push(&numbered_lines(LINES));

        let capture = tracker.only_capture().1;
        assert_eq!(capture.output_start, "");
        assert_eq!(capture.output_end.as_deref(), Some(""));
        assert_eq!(capture.output_observed_bytes, u64::conv(LINES * b"line 0000\r\n".len()));
    }

    // -- Partial lines at the cut ---------------------------------------------

    /// A short terminal, so the blank rows trailing the output do not eat the whole end budget
    /// at the small limits these cases use.
    const SHORT_ROWS: u16 = 4;

    /// One numbered assignment per line, so a kept fragment says both where in the output it came
    /// from and whether a credential survived the cut.
    const SECRET_LINE_LEN: usize = "AWS_SECRET_ACCESS_KEY=hunter0000".len();

    fn secret_lines(count: usize) -> Vec<u8> {
        let mut input = COMMAND_EXECUTED.to_vec();
        for i in 0..count {
            input.extend_from_slice(format!("AWS_SECRET_ACCESS_KEY=hunter{i:04}\r\n").as_bytes());
        }
        input.extend_from_slice(&finished(0, HID));
        input
    }

    fn split_capture(bytes: usize) -> (String, String) {
        // `bytes` is the budget for each side, which `split_evenly` gives it back from an even total.
        let mut tracker = Tracker::new(SHORT_ROWS, COLS, 2 * bytes);
        tracker.push(&secret_lines(60));

        let capture = tracker.only_capture().1;
        let end = capture.output_end.expect("this output has to lose its middle");
        (capture.output_start, end)
    }

    /// Sweep the limit across a line boundary and well past it, so the cut lands mid-line as
    /// often as not. At the real 512 KiB limits a chunk spans many rows, but where the cut falls
    /// *within* a row is arbitrary, and that is what these cover.
    #[rstest]
    fn neither_chunk_keeps_a_partial_line(
        #[values(36, 40, 45, 50, 60, 65, 70, 80, 99)] bytes: usize,
    ) {
        let (start, end) = split_capture(bytes);
        assert!(!start.is_empty() && !end.is_empty(), "nothing kept, so nothing is proven");

        for line in start.lines().chain(end.lines()) {
            assert_eq!(
                line.len(),
                SECRET_LINE_LEN,
                "a line the cut broke was kept with limit {bytes}: {line:?}",
            );
            assert!(line.starts_with("AWS_SECRET_ACCESS_KEY=hunter"), "{line:?}");
        }
    }

    /// The reason the partial lines go. Once the middle is discarded the two chunks are redacted
    /// separately, so a credential split across the cut matches neither half -- an assignment
    /// whose name ended one chunk leaves a bare value at the head of the next.
    #[rstest]
    fn a_credential_split_across_the_cut_does_not_survive(
        #[values(36, 40, 45, 50, 60, 65, 70, 80, 99)] bytes: usize,
    ) {
        let (start, end) = split_capture(bytes);
        assert!(!start.is_empty() && !end.is_empty(), "nothing kept, so nothing is proven");

        for chunk in [&start, &end] {
            let redacted = atuin_common::secrets::redact(chunk);
            assert!(
                !redacted.contains("hunter"),
                "a credential survived redaction with limit {bytes}: {redacted:?}",
            );
            // And it was redaction that removed it, not the guard removing everything.
            assert!(redacted.contains("AWS_SECRET_ACCESS_KEY=****"), "{redacted:?}");
        }
    }

    #[rstest]
    fn whole_lines_either_side_of_the_cut_are_kept() {
        // A budget reaching just past a line's newline keeps that whole line: the guard only ever
        // removes a line the cut had already broken.
        let (start, end) = split_capture(SECRET_LINE_LEN + 4);
        assert_eq!(start, "AWS_SECRET_ACCESS_KEY=hunter0000");
        assert_eq!(end, "AWS_SECRET_ACCESS_KEY=hunter0059");
    }

    #[rstest]
    fn a_chunk_with_no_newline_is_dropped_whole() {
        // The budget ends on the last byte of a line, so the chunk holds what is really a whole
        // line -- but its newline is on the far side of the cut, and a chunk with no newline in it
        // cannot be told apart from one the cut broke. Dropping it is the safe reading: keeping it
        // would leave a bare `hunter0059`, with the name that makes it recognisable on the other
        // side of the gap. It costs nothing at the real limits, where a chunk spans many rows.
        let (start, end) = split_capture(SECRET_LINE_LEN);
        assert_eq!(start, "");
        assert_eq!(end, "");
    }

    #[rstest]
    fn an_untruncated_capture_keeps_its_first_and_last_lines() {
        // The guard runs only where a middle was discarded. Nothing was, so no line is at risk
        // and none may be dropped.
        let mut tracker = Tracker::new(ROWS, COLS, LIMIT);
        tracker.push(&secret_lines(3));

        let capture = tracker.only_capture().1;
        assert_eq!(capture.output_end, None);
        assert_eq!(capture.output_start.lines().count(), 3);
        assert!(capture.output_start.starts_with("AWS_SECRET_ACCESS_KEY=hunter0000"));
        assert!(capture.output_start.ends_with("AWS_SECRET_ACCESS_KEY=hunter0002"));
    }

    // -- Terminal size --------------------------------------------------------

    #[rstest]
    fn resizing_reflows_the_capture(#[with(6, 20)] mut tracker: Tracker) {
        tracker.push(&[COMMAND_EXECUTED, b"abcdefghij"].concat());
        tracker.resize(6, 5);
        tracker.push(&[b"klmno\r\n".as_slice(), &finished(0, HID)].concat());

        // The first ten columns were rendered on a twenty-column screen; narrowing it drops
        // what no longer fits, and the rest is appended at the new width.
        assert_eq!(whole_output(&tracker.only_capture().1), "abcdklmno");
    }

    #[rstest]
    fn tiny_terminals_do_not_panic(#[values(1, 2, 3)] rows: u16, #[values(1, 2, 3)] cols: u16) {
        let mut tracker = Tracker::new(rows, cols, LIMIT);
        tracker.push(&interaction("$ ", "echo hi", "hello world\r\nsecond line\r\n"));

        // Whatever survives on a terminal this small, we must have reported something.
        assert_eq!(tracker.captures().len(), 1);
    }
}
