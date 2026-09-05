use std::io::Write;
use std::num::NonZeroU16;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

use atuin_common::os::unix::{SecureTempDirError, create_secure_temp_dir};
use easy_cast::Conv;

use crate::capture::{CommandCaptureSink, CommandCaptureTracker};
use crate::debug::Osc133DebugHighlighter;

pub enum Msg {
    Data(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    ScreenRequest(mpsc::Sender<Vec<u8>>),
}

pub fn socket_path() -> Result<PathBuf, SecureTempDirError> {
    let uid = atuin_common::os::unix::uid();
    let dir = atuin_common::os::unix::tmp_dir().join(format!("atuin-{uid}"));
    let dir = create_secure_temp_dir(dir)?;
    Ok(dir.join(format!("pty-proxy-{}.sock", std::process::id())))
}

pub struct ParserOptions {
    pub sink: Option<CommandCaptureSink>,
    pub debug_osc133: bool,
}

struct Parser {
    emulator: vt100::Parser,
    tracker: Option<CommandCaptureTracker>,
    highlighter: Option<Osc133DebugHighlighter>,
}

impl Parser {
    /// How many lines of scrollback the snapshot emulator can hold.
    ///
    /// Scrollback allows for better handling of terminal resizes: without scrollback, if the
    /// terminal height shrinks and then grows again, we'll know that lines from the scrollback got
    /// added back to the top of the terminal, but we won't actually know what they contain, so we
    /// won't be able to restore them when the search UI is opened and closed.
    ///
    /// Note that there is a best-effort fallback in the case where the terminal is resized by a
    /// larger amount than our scrollback capacity -- atuin-vt100 tracks how much scrollback there
    /// *would* be if the capacity were unbounded, and will fall back to inserting blank rows at the
    /// top. Compared to inserting blank rows at the bottom, which is what a terminal emulator would
    /// do when it genuinely ran of scrollback, this maintains the correct positioning of everything
    /// in the terminal -- otherwise our emulator would badly drift from the parent terminal.
    const SCROLLBACK_CAPACITY: usize = 50;

    fn new(rows: NonZeroU16, cols: NonZeroU16, options: ParserOptions) -> Self {
        Self {
            emulator: vt100::Parser::new(rows, cols, Self::SCROLLBACK_CAPACITY),
            tracker: options.sink.map(|f| CommandCaptureTracker::new(rows, cols, f)),
            highlighter: options.debug_osc133.then(Osc133DebugHighlighter::new),
        }
    }

    fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Data(raw_data) => {
                if let Some(tracker) = &mut self.tracker {
                    tracker.push(&raw_data);
                }

                let highlighted;
                let data: &[u8] = if let Some(highlighter) = &mut self.highlighter {
                    highlighted = highlighter.render(&raw_data);
                    &highlighted
                } else {
                    &raw_data
                };
                self.emulator.process(data);
            }
            Msg::Resize { rows, cols } => {
                // `vt100` dimensions can't be 0. Upstream would panic; now our `atuin-vt100` fork
                // requires dimensions to be `NonZeroU16` to ensure we don't hit those panics. Clamp
                // dimensions to 1.
                let rows = NonZeroU16::new(rows).unwrap_or(NonZeroU16::MIN);
                let cols = NonZeroU16::new(cols).unwrap_or(NonZeroU16::MIN);
                self.emulator.screen_mut().set_size(rows, cols);
                if let Some(tracker) = &mut self.tracker {
                    tracker.resize(rows, cols);
                }
            }
            Msg::ScreenRequest(reply_tx) => {
                let _ = reply_tx.send(encode_screen(&self.emulator));
            }
        }
    }
}

pub fn spawn_parser_thread(
    rows: u16,
    cols: u16,
    screen_rx: Receiver<Msg>,
    options: ParserOptions,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        // `vt100` dimensions can't be 0. Upstream would panic; now our `atuin-vt100` fork requires
        // dimensions to be `NonZeroU16` to ensure we don't hit those panics. Clamp dimensions to 1.
        let rows = NonZeroU16::new(rows).unwrap_or(NonZeroU16::MIN);
        let cols = NonZeroU16::new(cols).unwrap_or(NonZeroU16::MIN);
        let mut parser = Parser::new(rows, cols, options);
        for msg in screen_rx {
            parser.handle_msg(msg);
        }
    })
}

pub fn spawn_socket_server(sock_path: PathBuf, screen_tx: SyncSender<Msg>) {
    std::thread::spawn(move || {
        let listener = match UnixListener::bind(&sock_path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("atuin pty-proxy: failed to bind socket: {e}");
                return;
            }
        };

        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };

            let (reply_tx, reply_rx) = mpsc::channel();
            if screen_tx.send(Msg::ScreenRequest(reply_tx)).is_err() {
                break;
            }
            if let Ok(data) = reply_rx.recv() {
                let _ = stream.write_all(&data);
                let _ = stream.flush();
            }
        }
    });
}

/// Wire format written to the Unix socket:
///
/// ```text
/// [rows: u16 BE][cols: u16 BE][cursor_row: u16 BE][cursor_col: u16 BE]
/// [row_0_len: u32 BE][row_0_bytes...]
/// [row_1_len: u32 BE][row_1_bytes...]
/// ...
/// ```
///
/// Each row's bytes come from `screen.rows_formatted(0, cols)` and contain
/// pre-built ANSI escape sequences. The client can write them directly to
/// stdout without needing its own vt100 parser.
fn encode_screen(parser: &vt100::Parser) -> Vec<u8> {
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();

    let rows = rows.get();
    let cols = cols.get();

    let mut buf = Vec::with_capacity(256 + (usize::from(rows) * usize::from(cols)));
    buf.extend_from_slice(&rows.to_be_bytes());
    buf.extend_from_slice(&cols.to_be_bytes());
    buf.extend_from_slice(&cursor_row.to_be_bytes());
    buf.extend_from_slice(&cursor_col.to_be_bytes());

    for row_data in screen.rows_formatted(0, cols) {
        let len = u32::conv(row_data.len());
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(row_data.as_bytes());
    }

    buf
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use atuin_client::history::HistoryId;
    use rstest::{fixture, rstest};

    use super::*;
    use crate::capture::CommandCapture;

    const TIMEOUT: Duration = Duration::from_secs(5);

    const HID: &str = "00000000-0000-0000-0000-0000000000a1";

    fn hid(s: &str) -> HistoryId {
        s.parse().expect("valid history id")
    }

    /// Get the `rows` and `cols` values from an [`encode_screen`] blob.
    fn size_of(blob: &[u8]) -> (u16, u16) {
        (u16::from_be_bytes([blob[0], blob[1]]), u16::from_be_bytes([blob[2], blob[3]]))
    }

    /// Get the cursor position from an [`encode_screen`] blob.
    fn cursor_of(blob: &[u8]) -> (u16, u16) {
        (u16::from_be_bytes([blob[4], blob[5]]), u16::from_be_bytes([blob[6], blob[7]]))
    }

    /// Get the per-row payloads from an [`encode_screen`] blob.
    fn rows_of(blob: &[u8]) -> Vec<String> {
        let (rows, _) = size_of(blob);
        let mut rest = &blob[8..];
        (0..rows)
            .map(|_| {
                let (len, body) = rest.split_at(4);
                let len = usize::conv(u32::from_be_bytes(len.try_into().expect("4 bytes")));
                let (row, remainder) = body.split_at(len);
                rest = remainder;
                String::from_utf8(row.to_vec()).expect("rows are valid UTF-8")
            })
            .collect()
    }

    /// Ask a parser thread for its screen, waiting for it to work through the queue first.
    fn request_screen(msg_tx: &SyncSender<Msg>) -> Vec<u8> {
        let (reply_tx, reply_rx) = mpsc::channel();
        msg_tx.send(Msg::ScreenRequest(reply_tx)).expect("parser thread alive");
        reply_rx.recv_timeout(TIMEOUT).expect("parser thread still answering")
    }

    #[rstest]
    fn init_small_and_wrap(#[values(0, 1, 2, 3)] rows: u16, #[values(0, 1, 2, 3)] cols: u16) {
        let (msg_tx, msg_rx) = mpsc::sync_channel(8);
        spawn_parser_thread(rows, cols, msg_rx, plain());
        msg_tx.send(Msg::Data(b"hello world".to_vec())).expect("parser thread alive");

        // Dimensions are clamped to (1, 1) because vt100 dimensions must be positive.
        assert_eq!(size_of(&request_screen(&msg_tx)), (rows.max(1), cols.max(1)));
    }

    #[rstest]
    fn resize_small_and_wrap(
        #[with(1, 1)] mut parser: Parser,
        #[values(0, 1, 2, 3)] rows: u16,
        #[values(0, 1, 2, 3)] cols: u16,
    ) {
        parser.handle_msg(Msg::Resize { rows, cols });
        parser.handle_msg(Msg::Data(b"hello world".to_vec()));

        // Dimensions are clamped to (1, 1) because vt100 dimensions must be positive.
        assert_eq!(size_of(&encode_screen(&parser.emulator)), (rows.max(1), cols.max(1)));
    }

    #[rstest]
    fn encodes_the_screen_contents_and_cursor(#[with(3, 10)] mut parser: Parser) {
        parser.handle_msg(Msg::Data(b"one\r\ntwo".to_vec()));

        let blob = encode_screen(&parser.emulator);
        assert_eq!(size_of(&blob), (3, 10));
        assert_eq!(cursor_of(&blob), (1, 3));

        let rows = rows_of(&blob);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].contains("one"), "{rows:?}");
        assert!(rows[1].contains("two"), "{rows:?}");
    }

    #[rstest]
    fn a_resize_is_forwarded_to_the_capture_tracker() {
        let (sink, captures) = capture_sink();
        let mut parser = Parser::new(nonzero(6), nonzero(20), ParserOptions {
            sink: Some(sink),
            debug_osc133: false,
        });

        parser.handle_msg(Msg::Data(b"\x1b]133;C\x07abcdefghij".to_vec()));
        parser.handle_msg(Msg::Resize { rows: 6, cols: 5 });
        parser.handle_msg(Msg::Data(
            format!("klmno\r\n\x1b]133;D;0;history_id={HID}\x07").into_bytes(),
        ));

        let captures: Vec<_> = captures.try_iter().collect();
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].1.output, "abcdklmno");
    }

    #[rstest]
    fn the_parser_thread_feeds_the_capture_sink() {
        let (sink, captures) = capture_sink();
        let (msg_tx, msg_rx) = mpsc::sync_channel(8);
        spawn_parser_thread(24, 80, msg_rx, ParserOptions {
            sink: Some(sink),
            debug_osc133: false,
        });

        msg_tx
            .send(Msg::Data(
                format!(
                    "\x1b]133;A\x07$ \x1b]133;B\x07echo \
                     hi\r\n\x1b]133;C\x07hi\r\n\x1b]133;D;0;history_id={HID}\x07"
                )
                .into_bytes(),
            ))
            .expect("parser thread alive");
        // A screen request only comes back once the data above has been handled.
        let blob = request_screen(&msg_tx);
        assert_eq!(size_of(&blob), (24, 80));

        let captures: Vec<_> = captures.try_iter().collect();
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].1.output, "hi");
        assert_eq!(captures[0].0, hid(HID));
    }

    #[rstest]
    fn debug_highlighting_reaches_the_screen_but_not_the_capture() {
        // The highlighter's labels are a debugging aid for the terminal and the screen
        // snapshot. They are not terminal output the shell produced, so the capture tracker
        // has to see the raw stream -- otherwise the captured output is prefixed with a
        // label and `output_observed_bytes` counts them.
        let (sink, captures) = capture_sink();
        let mut parser = Parser::new(nonzero(6), nonzero(40), ParserOptions {
            sink: Some(sink),
            debug_osc133: true,
        });

        parser.handle_msg(Msg::Data(
            [
                b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\r\n".as_slice(),
                format!("\x1b]133;C\x07hi\r\n\x1b]133;D;0;history_id={HID}\x07").as_bytes(),
            ]
            .concat(),
        ));

        let captures: Vec<_> = captures.try_iter().collect();
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].1.output, "hi");
        assert_eq!(captures[0].1.output_observed_bytes, u64::conv(b"hi\r\n".len()));

        // The screen snapshot, on the other hand, is where the labels belong.
        let rows = rows_of(&encode_screen(&parser.emulator)).join("\n");
        assert!(rows.contains("[OSC133:A prompt]"), "{rows:?}");
        assert!(rows.contains("[OSC133:D exit=0]"), "{rows:?}");
    }

    #[rstest]
    fn a_parser_without_a_sink_still_tracks_the_screen(#[with(6, 20)] mut parser: Parser) {
        parser.handle_msg(Msg::Data(b"\x1b]133;C\x07hi\r\n\x1b]133;D;0\x07".to_vec()));

        assert!(rows_of(&encode_screen(&parser.emulator))[0].contains("hi"));
    }

    fn nonzero(value: u16) -> NonZeroU16 {
        NonZeroU16::new(value).expect("test dimensions are non-zero")
    }

    /// A [`Parser`] with no capture sink, for the tests that only look at the screen.
    #[fixture]
    fn parser(#[default(24)] rows: u16, #[default(80)] cols: u16) -> Parser {
        Parser::new(nonzero(rows), nonzero(cols), plain())
    }

    /// Parser options with nothing enabled.
    fn plain() -> ParserOptions {
        ParserOptions {
            sink: None,
            debug_osc133: false,
        }
    }

    /// A capture sink that funnels every `(history id, capture)` into the returned receiver.
    fn capture_sink() -> (CommandCaptureSink, Receiver<(HistoryId, CommandCapture)>) {
        let (sender, received) = mpsc::channel();
        let sink = Box::new(move |history_id, capture| {
            sender.send((history_id, capture)).expect("test receiver is still alive");
        });
        (sink, received)
    }
}
