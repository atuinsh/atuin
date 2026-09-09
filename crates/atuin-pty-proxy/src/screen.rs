use std::io::Write;
use std::num::NonZeroU16;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

use atuin_common::os::unix::tty::TtyId;
use atuin_common::os::unix::{SecureTempDirError, create_secure_temp_dir};
use easy_cast::Conv;

use crate::capture::{CaptureConfig, CommandCaptureTracker};
use crate::debug::Osc133DebugHighlighter;

pub enum Msg {
    Data(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    ScreenRequest(mpsc::Sender<Vec<u8>>),
}

/// The path to the PTY proxy socket for the given terminal.
pub fn socket_path(tty_id: TtyId) -> Result<PathBuf, SecureTempDirError> {
    Ok(socket_dir()?.join(socket_name(tty_id)))
}

/// The name of the PTY proxy socket for the given terminal.
#[must_use]
fn socket_name(tty_id: TtyId) -> String {
    format!("pty-proxy-{}-{}.sock", tty_id.dev, tty_id.rdev)
}

/// The directory in which PTY proxy sockets are stored.
fn socket_dir() -> Result<PathBuf, SecureTempDirError> {
    let uid = atuin_common::os::unix::uid();
    let dir = atuin_common::os::unix::tmp_dir().join(format!("atuin-{uid}"));
    create_secure_temp_dir(dir)
}

/// The socket path of the PTY proxy that this process is running in.
///
/// This process must be directly running inside an Atuin PTY proxy -- that is, its terminal must be
/// the child PTY created by the PTY proxy. Otherwise, this function will return [`None`].
#[must_use]
pub fn parent_socket_path() -> Option<PathBuf> {
    live_socket(socket_dir().ok()?, TtyId::current()?)
}

/// Whether this process is running directly inside an Atuin PTY proxy.
///
/// To qualify, this process's terminal must be the child PTY created by the PTY proxy. If there is
/// another PTY in between (e.g., from tmux or screen), this will return false.
#[must_use]
pub fn is_pty_proxy_child() -> bool {
    parent_socket_path().is_some()
}

/// Check whether `socket_dir` contains a live socket corresponding to `tty_id`.
///
/// If so, returns the path to the socket.
fn live_socket(mut socket_dir: PathBuf, tty: TtyId) -> Option<PathBuf> {
    socket_dir.push(socket_name(tty));
    std::os::unix::net::UnixStream::connect(&socket_dir).ok()?;
    Some(socket_dir)
}

pub struct ParserOptions {
    pub command_capture: Option<CaptureConfig>,
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
            tracker: options
                .command_capture
                .map(|c| CommandCaptureTracker::new(rows, cols, c.sink, c.max_output_bytes)),
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

/// The PTY proxy server.
pub struct SocketServer {
    listener: UnixListener,
}

impl SocketServer {
    /// Create a new socket server.
    ///
    /// The server will start running once [`Self::spawn`] is called.
    pub fn new(path: &std::path::Path) -> std::io::Result<Self> {
        Ok(Self {
            listener: UnixListener::bind(path)?,
        })
    }

    /// Start running the socket server on the current thread.
    pub fn run(self, msg_tx: &SyncSender<Msg>) {
        for stream in self.listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };

            let (reply_tx, reply_rx) = mpsc::channel();
            if msg_tx.send(Msg::ScreenRequest(reply_tx)).is_err() {
                break;
            }
            if let Ok(data) = reply_rx.recv() {
                let _ = stream.write_all(&data);
                let _ = stream.flush();
            }
        }
    }

    /// Start running the socket server on a new thread.
    pub fn spawn(self, msg_tx: SyncSender<Msg>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || self.run(&msg_tx))
    }
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
    use tempfile::TempDir;

    use super::*;
    use crate::capture::{CommandCapture, CommandCaptureSink};

    const TIMEOUT: Duration = Duration::from_secs(5);

    const HID: &str = "00000000-0000-0000-0000-0000000000a1";

    fn hid(s: &str) -> HistoryId {
        s.parse().expect("valid history id")
    }

    /// Terminals that differ in either half of their identity must not share a socket name.
    #[rstest]
    // Two panes of a multiplexer are different ptys on the same devpts mount.
    #[case::same_mount_other_pty(TtyId { dev: 24, rdev: 12 }, TtyId { dev: 24, rdev: 19 })]
    // A container gets its own devpts, where pts numbering restarts from zero. Without the
    // filesystem id in the name, its /dev/pts/0 would collide with the host's.
    #[case::same_pty_other_mount(TtyId { dev: 24, rdev: 0 }, TtyId { dev: 31, rdev: 0 })]
    fn socket_names_differ(#[case] one: TtyId, #[case] other: TtyId) {
        assert_ne!(socket_name(one), socket_name(other));
    }

    #[rstest]
    fn socket_name_is_built_from_both_halves_of_the_identity() {
        let tty = TtyId {
            dev: 24,
            rdev: 34828,
        };

        assert_eq!(socket_name(tty), "pty-proxy-24-34828.sock");
    }

    #[rstest]
    fn a_bound_socket_accepts_connections_before_anything_accepts_them(dir: TempDir) {
        // The proxy binds before spawning the shell so that the shell can never observe a
        // missing socket. That only works if `listen` has already happened when `SocketServer::new`
        // returns: a client connecting into the backlog must succeed with nobody accepting yet.
        let path = dir.path().join("pty-proxy-1-2-3.sock");

        let server = SocketServer::new(&path).unwrap();

        std::os::unix::net::UnixStream::connect(&path)
            .expect("a connection must succeed on a bound socket with no accept loop running");
        drop(server);
    }

    #[rstest]
    fn a_listening_proxy_socket_is_found(dir: TempDir) {
        let tty = TtyId { dev: 24, rdev: 12 };
        let path = dir.path().join(socket_name(tty));
        let _server = SocketServer::new(&path).unwrap();

        assert_eq!(live_socket(dir.path().to_path_buf(), tty), Some(path));
    }

    #[rstest]
    fn a_stale_socket_file_is_not_a_live_proxy(dir: TempDir) {
        // A proxy killed with SIGKILL never runs its cleanup, so the file outlives it. Because
        // the kernel reissues the lowest free pts index, a later terminal can legitimately have
        // the same identity -- and must not be fooled into thinking it is proxied.
        let tty = TtyId { dev: 24, rdev: 12 };
        let path = dir.path().join(socket_name(tty));
        let server = SocketServer::new(&path).unwrap();
        drop(server);
        assert!(path.exists(), "dropping a listener must leave the file behind");

        assert_eq!(live_socket(dir.path().to_path_buf(), tty), None);
    }

    #[rstest]
    fn no_socket_means_no_proxy(dir: TempDir) {
        let tty = TtyId { dev: 24, rdev: 12 };

        assert_eq!(live_socket(dir.path().to_path_buf(), tty), None);
    }

    /// Open a fresh pty pair, returning the master, the slave, and the slave's path.
    #[fixture]
    fn pty() -> (std::fs::File, std::fs::File, std::path::PathBuf) {
        use std::os::unix::ffi::OsStrExt;

        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let path = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(name.as_bytes()));
        let slave = std::fs::File::options().read(true).write(true).open(&path).unwrap();
        (std::fs::File::from(master), slave, path)
    }

    /// Serialises the tests that swap this process's stdin.
    static STDIN_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    /// Replace stdin with `fd` for as long as the returned guard lives.
    fn with_stdin(fd: std::os::fd::BorrowedFd<'_>) -> impl Drop {
        struct Restore {
            saved: std::os::fd::OwnedFd,
            _lock: parking_lot::MutexGuard<'static, ()>,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                rustix::stdio::dup2_stdin(&self.saved).unwrap();
            }
        }
        let lock = STDIN_LOCK.lock();
        let saved = rustix::io::dup(std::io::stdin()).unwrap();
        rustix::stdio::dup2_stdin(fd).unwrap();
        Restore { saved, _lock: lock }
    }

    /// A directory to hold proxy sockets, removed when the test ends.
    #[fixture]
    fn dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    /// Bind a socket for a proxy attached to `tty`, as the proxy itself would.
    fn proxy_listening_on(dir: &std::path::Path, tty: TtyId) -> SocketServer {
        SocketServer::new(&dir.join(socket_name(tty))).unwrap()
    }

    #[rstest]
    fn a_shell_in_the_proxys_own_pty_is_attached(
        dir: TempDir,
        pty: (std::fs::File, std::fs::File, std::path::PathBuf),
    ) {
        use std::os::fd::AsFd;

        let (_master, slave, _path) = pty;
        let _proxy = proxy_listening_on(dir.path(), TtyId::from_fd(&slave).unwrap());

        let attached = {
            let _stdin = with_stdin(slave.as_fd());
            live_socket(dir.path().to_path_buf(), TtyId::current().unwrap()).is_some()
        };

        assert!(attached);
    }

    #[rstest]
    fn a_shell_in_a_pty_nested_inside_the_proxy_is_not_attached(
        dir: TempDir,
        #[from(pty)] proxy_pty: (std::fs::File, std::fs::File, std::path::PathBuf),
        // tmux allocates a fresh pty for the pane; the proxy is still running around it.
        #[from(pty)] pane_pty: (std::fs::File, std::fs::File, std::path::PathBuf),
    ) {
        use std::os::fd::AsFd;

        let (_master, proxy_slave, _path) = proxy_pty;
        let _proxy = proxy_listening_on(dir.path(), TtyId::from_fd(&proxy_slave).unwrap());
        let (_pane_master, pane_slave, _pane_path) = pane_pty;

        let attached = {
            let _stdin = with_stdin(pane_slave.as_fd());
            live_socket(dir.path().to_path_buf(), TtyId::current().unwrap()).is_some()
        };

        assert!(!attached, "a multiplexer pane is a different terminal from the proxy's own");
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
            command_capture: Some(CaptureConfig {
                sink,
                max_output_bytes: 1024 * 1024,
            }),
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
            command_capture: Some(CaptureConfig {
                sink,
                max_output_bytes: 1024 * 1024,
            }),
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
            command_capture: Some(CaptureConfig {
                sink,
                max_output_bytes: 1024 * 1024,
            }),
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
            command_capture: None,
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
