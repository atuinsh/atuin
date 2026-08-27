use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use atuin_common::os::unix::{SecureTempDirError, create_secure_temp_dir};

use super::capture::{CommandCaptureSink, CommandCaptureTracker};
use super::debug::Osc133DebugHighlighter;
use crate::domain::screen::ScreenSnapshot;

pub enum Msg {
    Data(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    ScreenRequest(mpsc::Sender<ScreenSnapshot>),
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
    fn new(rows: NonZeroU16, cols: NonZeroU16, options: ParserOptions) -> Self {
        Self {
            emulator: vt100::Parser::new(rows, cols, 0),
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
                let _ = reply_tx.send(self.emulator.snapshot());
            }
        }
    }
}

pub fn spawn_parser_thread(
    rows: u16,
    cols: u16,
    screen_rx: Receiver<Msg>,
    options: ParserOptions,
) -> std::thread::JoinHandle<()> {
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

trait SnapshotExt {
    /// Snapshot the current state of the terminal emulator.
    fn snapshot(&self) -> ScreenSnapshot;
}

impl SnapshotExt for vt100::Parser {
    fn snapshot(&self) -> ScreenSnapshot {
        let screen = self.screen();
        let (_, cols) = screen.size();
        ScreenSnapshot::new(
            screen.size(),
            screen.cursor_position(),
            screen.rows_formatted(0, cols).collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::SyncSender;
    use std::time::Duration;

    use rstest::{fixture, rstest};

    use super::*;
    use crate::server::capture::CommandCapture;

    const TIMEOUT: Duration = Duration::from_secs(5);

    /// Ask a parser thread for its screen, waiting for it to work through the queue first.
    fn request_screen(msg_tx: &SyncSender<Msg>) -> ScreenSnapshot {
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
        assert_eq!(request_screen(&msg_tx).screen_dims, (rows.max(1), cols.max(1)));
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
        assert_eq!(parser.emulator.snapshot().screen_dims, (rows.max(1), cols.max(1)));
    }

    #[rstest]
    fn encodes_the_screen_contents_and_cursor(#[with(3, 10)] mut parser: Parser) {
        parser.handle_msg(Msg::Data(b"one\r\ntwo".to_vec()));

        let snapshot = parser.emulator.snapshot();
        assert_eq!(snapshot.screen_dims, (3, 10));
        assert_eq!(snapshot.cursor_pos, (1, 3));

        let rows = &snapshot.rows;
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
        parser.handle_msg(Msg::Data(b"klmno\r\n\x1b]133;D;0;history_id=hist\x07".to_vec()));

        let captures: Vec<_> = captures.try_iter().collect();
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].output, "abcdklmno");
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
                b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07hi\r\n\x1b]133;D;0;history_id=hist;session_id=sess\x07".to_vec(),
            ))
            .expect("parser thread alive");
        // A screen request only comes back once the data above has been handled.
        let snapshot = request_screen(&msg_tx);
        assert_eq!(snapshot.screen_dims, (24, 80));

        let captures: Vec<_> = captures.try_iter().collect();
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].command, "echo hi");
        assert_eq!(captures[0].output, "hi");
        assert_eq!(captures[0].history_id.as_deref(), Some("hist"));
    }

    #[rstest]
    fn debug_highlighting_reaches_the_screen_but_not_the_capture() {
        // The highlighter's labels are a debugging aid for the terminal and the screen
        // snapshot. They are not terminal output the shell produced, so the capture tracker
        // has to see the raw stream -- otherwise every captured field is prefixed with a
        // label and `output_observed_bytes` counts them.
        let (sink, captures) = capture_sink();
        let mut parser = Parser::new(nonzero(6), nonzero(40), ParserOptions {
            sink: Some(sink),
            debug_osc133: true,
        });

        parser.handle_msg(Msg::Data(
            [
                b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\r\n".as_slice(),
                b"\x1b]133;C\x07hi\r\n\x1b]133;D;0;history_id=hist;session_id=sess\x07",
            ]
            .concat(),
        ));

        let captures: Vec<_> = captures.try_iter().collect();
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].prompt, "$");
        assert_eq!(captures[0].command, "echo hi");
        assert_eq!(captures[0].output, "hi");
        assert_eq!(captures[0].output_observed_bytes, b"hi\r\n".len() as u64);

        // The screen snapshot, on the other hand, is where the labels belong.
        let rows = parser.emulator.snapshot().rows.join("\n");
        assert!(rows.contains("[OSC133:A prompt]"), "{rows:?}");
        assert!(rows.contains("[OSC133:D exit=0]"), "{rows:?}");
    }

    #[rstest]
    fn a_parser_without_a_sink_still_tracks_the_screen(#[with(6, 20)] mut parser: Parser) {
        parser.handle_msg(Msg::Data(b"\x1b]133;C\x07hi\r\n\x1b]133;D;0\x07".to_vec()));

        assert!(parser.emulator.snapshot().rows[0].contains("hi"));
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

    /// A capture sink that funnels every capture into the returned receiver.
    fn capture_sink() -> (CommandCaptureSink, Receiver<CommandCapture>) {
        let (sender, received) = mpsc::channel();
        let sink = Box::new(move |capture| {
            sender.send(capture).expect("test receiver is still alive");
        });
        (sink, received)
    }
}
