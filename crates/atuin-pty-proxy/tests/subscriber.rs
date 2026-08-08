//! Integration tests for the framed subscriber protocol, run against a real
//! PTY (`/bin/cat`) and a real Unix socket in a tempdir. Raw mode, SIGWINCH,
//! and stdin never enter the picture: the tests drive `ProxyCore` directly.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use atuin_pty_proxy::protocol::{self, Snapshot};
use atuin_pty_proxy::{ProxyCore, ProxyCoreConfig};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(30);
const DEADLINE: Duration = Duration::from_secs(30);

struct TestProxy {
    core: ProxyCore,
    child: Box<dyn Child + Send + Sync>,
    /// Keeps the PTY master fd alive for the duration of the test.
    _master: Box<dyn MasterPty + Send>,
    _tmp: tempfile::TempDir,
    sock: PathBuf,
    token: Vec<u8>,
}

impl Drop for TestProxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn disable_echo(master: &dyn MasterPty) {
    use nix::sys::termios;
    use std::os::fd::BorrowedFd;

    let raw_fd = master.as_raw_fd().expect("MasterPty::as_raw_fd");
    // SAFETY: The file descriptor is owned by `master`, which cannot get dropped during the life
    // of the `BorrowedFd` because we hold a shared reference to it.
    let fd = unsafe { BorrowedFd::borrow_raw(raw_fd) };

    let mut attrs = termios::tcgetattr(fd).expect("tcgetattr");
    attrs.local_flags.remove(termios::LocalFlags::ECHO);
    termios::tcsetattr(fd, termios::SetArg::TCSANOW, &attrs).expect("tcsetattr");
}

fn spawn_proxy(rows: u16, cols: u16) -> TestProxy {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    // Terminal echo is unreliable; under load, characters can be dropped, which results in lines
    // that don't stretch the full width of the screen, causing certain tests to fail. Disable echo
    // so we're only testing the output of `cat`, which is reliable.
    disable_echo(&*pair.master);
    let child = pair
        .slave
        .spawn_command(CommandBuilder::new("/bin/cat"))
        .expect("spawn /bin/cat");
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().expect("clone reader");
    let writer = pair.master.take_writer().expect("take writer");

    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("proxy");

    let core = ProxyCore::spawn(ProxyCoreConfig {
        reader,
        writer,
        mirror: Box::new(std::io::sink()),
        rows,
        cols,
        dir,
        debug_osc133: false,
        command_capture_sink: None,
    })
    .expect("spawn proxy core");

    let sock = core.socket_path().to_path_buf();
    let token = std::fs::read(core.token_path()).expect("read token");

    TestProxy {
        core,
        child,
        _master: pair.master,
        _tmp: tmp,
        sock,
        token,
    }
}

struct Client {
    stream: UnixStream,
}

impl Client {
    fn connect(sock: &Path, want_input: bool, token: &[u8]) -> Self {
        let mut stream = UnixStream::connect(sock).expect("connect");
        stream
            .set_read_timeout(Some(CLIENT_READ_TIMEOUT))
            .expect("set read timeout");
        stream.write_all(&protocol::MAGIC).expect("write magic");
        stream
            .write_all(&protocol::subscribe_frame(want_input, token))
            .expect("write subscribe");
        Self { stream }
    }

    fn read_frame(&mut self) -> (u8, Vec<u8>) {
        protocol::read_frame(&mut self.stream)
            .expect("read frame")
            .expect("unexpected EOF")
    }

    fn expect_hello(&mut self, want_granted: bool) {
        let (frame_type, payload) = self.read_frame();
        assert_eq!(frame_type, protocol::FRAME_HELLO);
        let (version, granted) = protocol::decode_hello(&payload).expect("decode hello");
        assert_eq!(version, protocol::PROTOCOL_VERSION);
        assert_eq!(granted, want_granted);
    }

    fn expect_keyframe(&mut self) -> Snapshot {
        let (frame_type, payload) = self.read_frame();
        assert_eq!(frame_type, protocol::FRAME_KEYFRAME);
        Snapshot::decode(&payload).expect("decode keyframe")
    }
}

#[test]
fn subscribe_gets_hello_and_keyframe_at_the_pty_size() {
    let proxy = spawn_proxy(24, 80);
    let mut client = Client::connect(&proxy.sock, false, b"");

    client.expect_hello(false);
    let snapshot = client.expect_keyframe();
    assert_eq!((snapshot.rows, snapshot.cols), (24, 80));
    assert_eq!(snapshot.rows_data.len(), 24);
}

#[test]
fn authenticated_input_roundtrips_through_the_shell() {
    let proxy = spawn_proxy(24, 80);
    let mut client = Client::connect(&proxy.sock, true, &proxy.token);

    client.expect_hello(true);
    client.expect_keyframe();

    let marker = b"itest-marker-4242";
    let mut line = marker.to_vec();
    line.push(b'\n');
    client
        .stream
        .write_all(&protocol::input_frame(&line))
        .expect("write input");

    // Wait for `cat`'s output.
    let mut seen = Vec::new();
    let deadline = Instant::now() + DEADLINE;
    while !contains(&seen, marker) {
        assert!(Instant::now() < deadline, "marker never echoed back");
        let (frame_type, payload) = client.read_frame();
        if frame_type == protocol::FRAME_OUTPUT {
            seen.extend_from_slice(&payload);
        }
    }
}

#[test]
fn resize_hook_emits_a_resize_frame() {
    let proxy = spawn_proxy(24, 80);
    let mut client = Client::connect(&proxy.sock, false, b"");

    client.expect_hello(false);
    client.expect_keyframe();

    proxy.core.handle().resize(30, 100);

    let deadline = Instant::now() + DEADLINE;
    loop {
        assert!(Instant::now() < deadline, "resize frame never arrived");
        let (frame_type, payload) = client.read_frame();
        if frame_type == protocol::FRAME_RESIZE {
            assert_eq!(protocol::decode_resize(&payload), Some((30, 100)));
            break;
        }
    }
}

#[test]
fn input_without_the_token_is_refused_and_closed() {
    let proxy = spawn_proxy(24, 80);
    let mut client = Client::connect(&proxy.sock, true, b"not-the-token");

    // Wrong token: subscribed, but input is not granted.
    client.expect_hello(false);
    client.expect_keyframe();

    client
        .stream
        .write_all(&protocol::input_frame(b"nope\n"))
        .expect("write input");

    // The server must close the connection. Buffered frames may still
    // arrive first; a reset instead of a clean EOF also counts as closed.
    loop {
        match protocol::read_frame(&mut client.stream) {
            Ok(None) => break,
            Ok(Some(_)) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                panic!("server did not close the connection after rejected input");
            }
            Err(_) => break,
        }
    }
}

#[test]
fn flooded_subscriber_is_disconnected_and_can_resync() {
    let proxy = spawn_proxy(24, 80);

    // A subscribes and then never reads: its frame queue must overflow.
    let mut stalled = Client::connect(&proxy.sock, false, b"");

    // Flood the PTY through the input path (what the stdin pump uses):
    // ~2.5 MiB in, echoed back by cat, far beyond the 128-frame queue
    // and every socket buffer in between.
    let input_tx = proxy.core.input_sender();
    let line = [b"x".repeat(512), b"\n".to_vec()].concat();
    for _ in 0..5000 {
        input_tx.send(line.clone()).expect("flood send");
    }

    // The parser drops A; A's writer thread drains what was queued, then
    // shuts the socket down. Draining A must therefore end in EOF/reset —
    // a read timeout means A was never disconnected.
    let mut buf = [0u8; 8192];
    loop {
        match stalled.stream.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                panic!("stalled subscriber was never disconnected");
            }
            Err(_) => break,
        }
    }

    // Reconnect: a fresh keyframe resyncs the client.
    let mut fresh = Client::connect(&proxy.sock, false, b"");
    fresh.expect_hello(false);
    let snapshot = fresh.expect_keyframe();
    assert_eq!((snapshot.rows, snapshot.cols), (24, 80));
}

#[test]
fn legacy_one_shot_snapshot_still_works() {
    // The search popup's contract: connect, write nothing, read the raw
    // snapshot blob until EOF.
    let proxy = spawn_proxy(24, 80);

    let mut stream = UnixStream::connect(&proxy.sock).expect("connect");
    stream
        .set_read_timeout(Some(CLIENT_READ_TIMEOUT))
        .expect("set read timeout");
    let mut blob = Vec::new();
    stream.read_to_end(&mut blob).expect("read snapshot");

    let snapshot = Snapshot::decode(&blob).expect("decode legacy snapshot");
    assert_eq!((snapshot.rows, snapshot.cols), (24, 80));
}

/// Paint every cell of the proxy's screen model, so its snapshot blob is
/// bigger than any socket buffer between the two ends. Written through the
/// input path (what the stdin pump uses); `/bin/cat` echoes it back as PTY
/// output, which is what the parser thread sees.
fn paint_screen(proxy: &TestProxy, rows: u16, cols: u16) {
    let input_tx = proxy.core.input_sender();
    let line = [b"x".repeat(cols as usize), b"\n".to_vec()].concat();
    // Twice the screen height: `rows` is enough to fill the screen; `rows * 2`
    // is extra margin. One row at the bottom will always be blank due to the
    // trailing newline after the last row.
    for _ in 0..(rows as usize * 2) {
        input_tx.send(line.clone()).expect("paint send");
    }
}

/// Block until the legacy snapshot blob is at least `min_bytes` long, so a
/// test that depends on a reply larger than the socket buffer cannot silently
/// run against a blank screen (and pass against unfixed code).
fn wait_for_snapshot_of_at_least(sock: &Path, min_bytes: usize) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let mut stream = UnixStream::connect(sock).expect("connect probe");
        stream
            .set_read_timeout(Some(CLIENT_READ_TIMEOUT))
            .expect("set read timeout");
        let mut blob = Vec::new();
        stream.read_to_end(&mut blob).expect("read snapshot");
        let len = blob.len();
        if len >= min_bytes {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "screen never painted: snapshot is only {len} bytes, wanted {min_bytes}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// D2: one same-uid client that connects and never reads must not wedge the
/// socket server.
///
/// The reply to such a client blocks as soon as the peer's socket buffer
/// fills (~8 KiB), so serving it on the accept loop stalled *every* later
/// connection — the legacy popup and v2 subscribers alike, since one loop
/// serves both. Both are asserted here for that reason.
///
/// The screen is deliberately large (200 columns x 80 rows, painted edge to
/// edge) and the paint is *waited on* until the snapshot is comfortably past
/// the 8192-byte buffer: a small or blank screen makes the reply fit, so it
/// never blocks and the test would pass against the unfixed server.
#[test]
fn a_stalled_client_does_not_wedge_the_socket_server() {
    const WEDGE_TIMEOUT: Duration = Duration::from_secs(5);
    /// One unix-socket buffer's worth (macOS and Linux both default to
    /// 8192 bytes for a peer that never reads); the reply must exceed it by
    /// a wide margin so `write_all` genuinely blocks.
    const MIN_SNAPSHOT_BYTES: usize = 12_000;

    let proxy = spawn_proxy(80, 200);
    paint_screen(&proxy, 80, 200);
    wait_for_snapshot_of_at_least(&proxy.sock, MIN_SNAPSHOT_BYTES);

    // The staller: writes nothing (so it is served the legacy one-shot
    // snapshot) and never reads. Held alive for the rest of the test.
    let _stalled = UnixStream::connect(&proxy.sock).expect("connect staller");
    // Long enough for the greeting sniff to expire and the reply to block.
    std::thread::sleep(Duration::from_millis(500));

    // The search popup's contract, while the staller stalls.
    let start = Instant::now();
    let mut legacy = UnixStream::connect(&proxy.sock).expect("connect legacy");
    legacy
        .set_read_timeout(Some(WEDGE_TIMEOUT))
        .expect("set read timeout");
    let mut blob = Vec::new();
    legacy
        .read_to_end(&mut blob)
        .expect("legacy snapshot must still be served while another client stalls");
    let snapshot = Snapshot::decode(&blob).expect("decode legacy snapshot");
    assert_eq!((snapshot.rows, snapshot.cols), (80, 200));
    assert!(
        start.elapsed() < WEDGE_TIMEOUT,
        "legacy snapshot took {:?} behind a stalled client",
        start.elapsed()
    );

    // ...and a v2 subscribe, the other client of the same loop.
    let start = Instant::now();
    let mut client = Client::connect(&proxy.sock, false, b"");
    client
        .stream
        .set_read_timeout(Some(WEDGE_TIMEOUT))
        .expect("set read timeout");
    client.expect_hello(false);
    let snapshot = client.expect_keyframe();
    assert_eq!((snapshot.rows, snapshot.cols), (80, 200));
    assert!(
        start.elapsed() < WEDGE_TIMEOUT,
        "v2 subscribe took {:?} behind a stalled client",
        start.elapsed()
    );
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
