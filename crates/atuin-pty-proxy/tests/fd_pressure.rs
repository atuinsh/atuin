//! The socket server must survive running the process out of file
//! descriptors.
//!
//! **This test binary lowers its own `RLIMIT_NOFILE`**, which is process-wide
//! and would sabotage anything sharing the process (cargo runs the tests in a
//! binary as threads). It therefore lives alone in its own integration-test
//! file, and must stay that way — do not add a second `#[test]` here.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use atuin_pty_proxy::protocol::{self, Snapshot};
use atuin_pty_proxy::{ProxyCore, ProxyCoreConfig};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// The fd ceiling the flood below drives the process into. High enough that
/// the proxy, the PTY and the test harness itself all start cleanly, low
/// enough that a few hundred client connections exhaust it.
const NOFILE_LIMIT: u64 = 128;
/// How many connections the flood opens (or tries to). Well past the limit:
/// the point is to reach EMFILE, on the client side *and* inside the server's
/// `accept()`, which runs in this same process.
const FLOOD: usize = 400;
/// Long enough to cover the server's own per-connection lifetimes
/// (`GREETING_SNIFF_TIMEOUT` + `LEGACY_WRITE_TIMEOUT` ~= 5.1 s), so the
/// probes below run when the transient condition is provably over.
const RECOVERY_WINDOW: Duration = Duration::from_secs(7);
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(10);
const PAINT_DEADLINE: Duration = Duration::from_secs(30);

struct TestProxy {
    core: ProxyCore,
    child: Box<dyn Child + Send + Sync>,
    _master: Box<dyn MasterPty + Send>,
    _tmp: tempfile::TempDir,
    sock: PathBuf,
}

impl Drop for TestProxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
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
    let child = pair
        .slave
        .spawn_command(CommandBuilder::new("/bin/cat"))
        .expect("spawn /bin/cat");
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().expect("clone reader");
    let writer = pair.master.take_writer().expect("take writer");
    let tmp = tempfile::tempdir().expect("tempdir");

    let core = ProxyCore::spawn(ProxyCoreConfig {
        reader,
        writer,
        mirror: Box::new(std::io::sink()),
        rows,
        cols,
        dir: tmp.path().join("proxy"),
        debug_osc133: false,
        command_capture_sink: None,
    })
    .expect("spawn proxy core");

    let sock = core.socket_path().to_path_buf();
    TestProxy {
        core,
        child,
        _master: pair.master,
        _tmp: tmp,
        sock,
    }
}

/// Paint the screen so the legacy snapshot is comfortably larger than a
/// socket buffer, and wait until it is: a blank screen would let every reply
/// fit and never exercise a held connection.
fn paint_and_wait(proxy: &TestProxy, rows: u16, cols: u16, min_bytes: usize) {
    let input_tx = proxy.core.input_sender();
    let line = [b"x".repeat(cols as usize), b"\n".to_vec()].concat();

    let deadline = Instant::now() + PAINT_DEADLINE;
    loop {
        // Repaint every round rather than once up front. Terminal echo is
        // unreliable under load, so a one-shot paint leaves the screen
        // permanently short of the target and the wait below spins until the
        // deadline against a size that will never grow. Resending is what makes
        // the loop a wait rather than a gamble.
        for _ in 0..(rows as usize * 2) {
            input_tx.send(line.clone()).expect("paint send");
        }

        let len = legacy_snapshot(&proxy.sock).map_or(0, |blob| blob.len());
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

/// The search popup's contract: connect, write nothing, read to EOF.
fn legacy_snapshot(sock: &Path) -> Result<Vec<u8>, String> {
    let mut stream = UnixStream::connect(sock).map_err(|e| format!("connect: {e}"))?;
    stream
        .set_read_timeout(Some(CLIENT_READ_TIMEOUT))
        .map_err(|e| format!("set read timeout: {e}"))?;
    let mut blob = Vec::new();
    stream
        .read_to_end(&mut blob)
        .map_err(|e| format!("read: {e}"))?;
    Ok(blob)
}

fn set_nofile(limit: u64) -> u64 {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};
    let (soft, hard) = getrlimit(Resource::RLIMIT_NOFILE).expect("getrlimit");
    let want = limit.min(hard);
    setrlimit(Resource::RLIMIT_NOFILE, want, hard).expect("setrlimit");
    soft
}

/// D2, second half: the accept loop must ride out fd exhaustion.
///
/// A same-uid client that opens connections faster than they retire drives
/// this process to its `RLIMIT_NOFILE`, at which point the server's own
/// `accept()` fails with EMFILE. Treating that as terminal dropped the
/// listener and left every later connect with ECONNREFUSED — permanently, for
/// the shell's remaining lifetime, killing both the search popup and every
/// `--active` attach. It is strictly worse than the stall
/// `a_stalled_client_does_not_wedge_the_socket_server` covers, which cleared
/// when the stalled client left.
///
/// The flood is released and a full recovery window waited out before the
/// assertions, so what is being asserted is genuinely "the server came back",
/// not "the server was briefly slow".
#[test]
fn socket_server_survives_fd_exhaustion() {
    const ROWS: u16 = 80;
    const COLS: u16 = 200;
    /// One unix-socket buffer's worth; the reply must exceed it so held
    /// connections genuinely block server-side.
    const MIN_SNAPSHOT_BYTES: usize = 12_000;

    let proxy = spawn_proxy(ROWS, COLS);
    paint_and_wait(&proxy, ROWS, COLS, MIN_SNAPSHOT_BYTES);
    assert!(
        legacy_snapshot(&proxy.sock).is_ok(),
        "the server must serve before the flood"
    );

    // Everything that needs an fd to start is up; squeeze the process.
    let original_soft = set_nofile(NOFILE_LIMIT);

    // The flood: connect, write nothing (so each is served as a legacy
    // one-shot reply), never read. Every one holds a client fd here and a
    // server fd plus a thread over there.
    let mut held = Vec::new();
    let mut v2 = Vec::new();
    for i in 0..FLOOD {
        let Ok(mut stream) = UnixStream::connect(&proxy.sock) else {
            break; // EMFILE on our side; the server has hit it too
        };
        // Every fourth connection asks to be a v2 subscriber: those are the
        // ones whose read timeout the server clears, so they would otherwise
        // hold their fds for the proxy's whole lifetime.
        if i % 4 == 0 {
            let _ = stream.write_all(&protocol::MAGIC);
            let _ = stream.write_all(&protocol::subscribe_frame(false, b""));
            v2.push(stream);
        } else {
            held.push(stream);
        }
    }
    assert!(
        held.len() + v2.len() > 32,
        "the flood must actually reach the fd ceiling; opened {}",
        held.len() + v2.len()
    );

    // Let the server hit EMFILE in accept() while the pressure is on.
    std::thread::sleep(Duration::from_millis(1500));

    // Release everything and give the server longer than any per-connection
    // timeout to drain: the transient condition is now fully over.
    drop(held);
    drop(v2);
    set_nofile(original_soft);
    std::thread::sleep(RECOVERY_WINDOW);

    // The search popup still works...
    let blob = legacy_snapshot(&proxy.sock).expect("legacy snapshot after fd exhaustion");
    let snapshot = Snapshot::decode(&blob).expect("decode legacy snapshot");
    assert_eq!((snapshot.rows, snapshot.cols), (ROWS, COLS));

    // ...and so does a v2 subscribe, the other client of the same loop.
    let mut client = UnixStream::connect(&proxy.sock).expect("v2 connect after fd exhaustion");
    client
        .set_read_timeout(Some(CLIENT_READ_TIMEOUT))
        .expect("set read timeout");
    client.write_all(&protocol::MAGIC).expect("write magic");
    client
        .write_all(&protocol::subscribe_frame(false, b""))
        .expect("write subscribe");
    let (frame_type, payload) = protocol::read_frame(&mut client)
        .expect("read hello")
        .expect("hello, not EOF");
    assert_eq!(frame_type, protocol::FRAME_HELLO);
    assert!(protocol::decode_hello(&payload).is_some());
    let (frame_type, payload) = protocol::read_frame(&mut client)
        .expect("read keyframe")
        .expect("keyframe, not EOF");
    assert_eq!(frame_type, protocol::FRAME_KEYFRAME);
    let snapshot = Snapshot::decode(&payload).expect("decode keyframe");
    assert_eq!((snapshot.rows, snapshot.cols), (ROWS, COLS));
}
