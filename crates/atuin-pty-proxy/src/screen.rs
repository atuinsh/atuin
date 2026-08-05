//! The proxy's screen model and socket server.
//!
//! A single parser thread owns the vt100 model and the subscriber registry;
//! the socket server accepts both legacy one-shot snapshot clients and
//! framed-v2 subscribers (see [`crate::protocol`]).

use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

use rand::RngCore;

use crate::protocol;

/// How long the greeting sniff waits for a v2 magic before falling back to
/// the legacy one-shot path. The legacy popup client never writes, so every
/// legacy connection pays this delay; its own read timeout is 2 s.
const GREETING_SNIFF_TIMEOUT: Duration = Duration::from_millis(100);

/// How long a v2 connection may take to follow its magic with the Subscribe
/// frame. A same-uid client that greets and then idles must not park a
/// reader thread and its fd for the proxy's lifetime; a real subscriber
/// writes magic and Subscribe back to back. Cleared once the subscriber is
/// registered — steady-state input reads may legitimately idle for hours.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the legacy one-shot snapshot reply may take to drain into the
/// socket. A same-uid client that greets and then idles must not park a
/// writer thread and its fd for the proxy's lifetime; a snapshot larger than
/// the socket buffer parks `write_all` forever otherwise. Set far above the
/// popup client's own 2 s read timeout, so it only ever fires for a peer that
/// has already stopped caring — a shorter one would truncate a large
/// legitimate snapshot mid-write.
const LEGACY_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-subscriber frame queue depth. A subscriber whose queue fills is
/// dropped (disconnect-on-overflow); the client resyncs by reconnecting and
/// receiving a fresh keyframe.
const SUBSCRIBER_QUEUE_FRAMES: usize = 128;

/// How many accepted connections may be in flight at once.
///
/// Legitimate demand is tiny: the search popup connects, reads one snapshot
/// and closes, and `--active` holds a single v2 subscriber. The cap exists
/// because connections are *not* free to hold — a legacy one owns a server fd
/// and a thread for up to [`GREETING_SNIFF_TIMEOUT`] + [`LEGACY_WRITE_TIMEOUT`],
/// and a registered subscriber owns an fd and two threads for the proxy's
/// lifetime — so a same-uid client can open them far faster than they retire.
/// Uncapped, that lands on the *process's* fd limit, i.e. on the user's shell,
/// which is the one thing this proxy must never take down.
///
/// It also bounds the parser's in-flight work: each connection thread can have
/// at most one blocking `Msg::ScreenRequest` outstanding, so the number of
/// snapshot renders queued ahead of the terminal pump is bounded by this
/// number rather than by how many clients showed up (see the blocking-send
/// invariant on [`spawn_parser_thread`]).
///
/// Over the cap the stream is dropped immediately: the peer sees a clean EOF
/// rather than a hang, and no fd or thread is committed to it.
const MAX_LIVE_CONNECTIONS: usize = 32;

/// How many v2 subscribers may be registered with the parser at once.
///
/// Well above real demand (one `--active` share, plus a transient overlap
/// while a resync replaces its connection) and far below anything that makes
/// fan-out expensive: every frame is cloned per subscriber. Refusing the
/// registration drops the subscriber's frame channel, which ends that
/// connection the same way an overflowing queue does.
const MAX_SUBSCRIBERS: usize = 8;

/// How long the accept loop waits before retrying after a resource-exhaustion
/// `accept()` failure (EMFILE/ENFILE and friends).
///
/// Long enough that a process at its fd ceiling is not spun on, short enough
/// that recovery is imperceptible. See [`AcceptFailure`].
const ACCEPT_RETRY_PAUSE: Duration = Duration::from_millis(50);

pub(crate) enum Msg {
    Data(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    ScreenRequest(mpsc::Sender<Vec<u8>>),
    /// Register a subscriber under `id`. The parser immediately queues
    /// Hello + Keyframe so every subscriber's stream starts with an honest
    /// screen. `input_granted` was decided by the connection's reader thread
    /// (token check) and is echoed into the Hello frame.
    Subscribe {
        id: SubscriberId,
        frames_tx: SyncSender<Vec<u8>>,
        input_granted: bool,
    },
    /// The connection behind `id` is over: forget it.
    ///
    /// Sent by the connection's own reader thread on every exit path. Without
    /// it a registry entry (and the writer thread parked on its channel)
    /// survives until the *next* output frame happens to fail on it — which
    /// on an idle shell may be never, so a client that reconnects in silence
    /// could pile up entries and threads and, with [`MAX_SUBSCRIBERS`] in
    /// force, lock out honest subscribers.
    Unsubscribe(SubscriberId),
    /// The PTY read loop exited: broadcast End and drop all subscribers.
    Eof,
}

/// Identifies one registered subscriber for its lifetime, so
/// [`Msg::Unsubscribe`] can name the entry to drop. Process-unique and
/// monotonic — never reused, so a late unsubscribe from a dead connection
/// cannot evict a live one.
type SubscriberId = u64;

/// Source of [`SubscriberId`]s. Wrapping at 2^64 is not a concern: one id per
/// accepted v2 connection.
static NEXT_SUBSCRIBER_ID: AtomicU64 = AtomicU64::new(0);

/// Default per-proxy directory: `$TMPDIR/atuin-pty-proxy-<pid>-<8 hex>`.
pub(crate) fn default_proxy_dir() -> PathBuf {
    std::env::temp_dir().join(proxy_dir_name())
}

/// Fallback when the default directory would overflow `sockaddr_un.sun_path`
/// (macOS TMPDIR paths can be long): a literal `/tmp` location.
pub(crate) fn fallback_proxy_dir() -> PathBuf {
    Path::new("/tmp").join(proxy_dir_name())
}

/// The per-proxy directory name: the pid for debuggability, plus 8 random
/// hex characters so the name is unpredictable. The temp dir (and the `/tmp`
/// fallback always) can be world-writable: with a name derived from the pid
/// alone, another local user could pre-create the directory for plausible
/// pids and deny proxy startup — [`create_proxy_dir`] fails closed rather
/// than ever reusing a foreign directory. Discovery never depends on the
/// name: children find the socket through `$ATUIN_PTY_PROXY_SOCKET`.
fn proxy_dir_name() -> String {
    let mut suffix = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut suffix);
    format!(
        "atuin-pty-proxy-{}-{}",
        std::process::id(),
        hex_encode(&suffix)
    )
}

pub(crate) fn socket_path_in(dir: &Path) -> PathBuf {
    dir.join("sock")
}

pub(crate) fn token_path_in(dir: &Path) -> PathBuf {
    dir.join("token")
}

/// Whether `path` fits in `sockaddr_un.sun_path`. The real limit is 104
/// (macOS) or 108 (Linux) bytes including the NUL; 100 is conservative.
/// Checked *before* the path is exported to the child's environment, so we
/// never advertise a socket we cannot bind.
pub(crate) fn socket_path_fits(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().len() < 100
}

/// Create the per-proxy directory with mode 0700, replacing any stale
/// leftover at the same path. A foreign directory that cannot be removed
/// (the sticky bit protects other users' entries in `/tmp`) makes the
/// `create` below fail with `AlreadyExists`: fail closed — never reuse a
/// directory this process did not create.
pub(crate) fn create_proxy_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let _ = std::fs::remove_dir_all(dir);
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

/// Write a fresh input token (32 random bytes, hex-encoded to 64 ASCII
/// characters) to `<dir>/token` with mode 0600 and return its bytes.
pub(crate) fn write_token(dir: &Path) -> io::Result<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = hex_encode(&raw).into_bytes();

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(token_path_in(dir))?;
    file.write_all(&token)?;
    Ok(token)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Spawn the parser thread: the single owner of the vt100 model and the
/// subscriber registry, fed in order by the output pump and resize handler.
///
/// # Blocking-send invariant
///
/// Producers (the PTY output pump, the SIGWINCH handler, the stdin-side
/// input path) use *blocking* `send` into the bounded message channel so the
/// subscriber feed is lossless. That is safe only because this thread never
/// blocks: vt100 processing is bounded CPU work, legacy snapshot replies go
/// through an unbounded channel, and every per-subscriber send is a
/// `try_send` (a slow subscriber is dropped, never waited on). A blocking
/// producer `send` can therefore only briefly backpressure the terminal pump
/// while the parser catches up — it can never wedge it — and if this thread
/// dies the channel disconnects, making `send` return `Err` immediately.
///
/// "Briefly" is also bounded on the *other* side: connections are served on
/// their own threads, so several `Msg::ScreenRequest`s (one snapshot render
/// each) can be queued ahead of the pump's `Msg::Data` at once — at most
/// [`MAX_LIVE_CONNECTIONS`] of them, since each connection thread can have
/// only one outstanding. That cap is what keeps the bound a bound.
pub(crate) fn spawn_parser_thread(rows: u16, cols: u16, msg_rx: Receiver<Msg>) {
    std::thread::spawn(move || {
        let mut state = ParserState::new(rows, cols);

        loop {
            let first = match msg_rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            };

            state.handle(first);

            while let Ok(msg) = msg_rx.try_recv() {
                state.handle(msg);
            }
        }
    });
}

struct ParserState {
    parser: vt100::Parser,
    /// The registered subscribers, each paired with the id its connection
    /// will name in [`Msg::Unsubscribe`]. Capped at [`MAX_SUBSCRIBERS`].
    subscribers: Vec<(SubscriberId, SyncSender<Vec<u8>>)>,
    /// The clamped grid size, tracked outside the parser so panic recovery
    /// ([`Self::vt100_guarded`]) never has to query a model a panic may have
    /// left mid-update.
    rows: u16,
    cols: u16,
}

impl ParserState {
    fn new(rows: u16, cols: u16) -> Self {
        // vt100 0.16 underflows (and panics in debug builds) on a 0-row or
        // 0-column grid; clamp here and in the Resize arm.
        let (rows, cols) = (rows.max(1), cols.max(1));
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            subscribers: Vec::new(),
            rows,
            cols,
        }
    }

    /// Run a vt100 operation, absorbing any panic inside the library.
    ///
    /// vt100 0.16 has known panic paths beyond the 0-dimension underflow the
    /// clamps remove — e.g. text that wraps on a 1-row grid panics in
    /// `col_wrap`. The parser thread must survive them: it is the single
    /// owner of the screen model, so its death silently ends every service
    /// built on that model — legacy/probe connections get no snapshot at all
    /// and every `Subscribe` fails, taking `--active` with it for the shell's
    /// remaining lifetime. (The accept loop itself survives: connections are
    /// served on their own threads — see [`spawn_socket_server`] — so a dead
    /// parser no longer cascades into a dead server.) On a caught
    /// panic the model is rebuilt blank at the tracked size; the next full
    /// redraw repaints it, and fan-out of the raw bytes is unaffected —
    /// subscribers keep receiving the exact PTY stream. (The panic hook's
    /// one-line report still reaches stderr; noisy, but the pre-guard
    /// behaviour was a silently dead subscriber server.)
    fn vt100_guarded(&mut self, op: impl FnOnce(&mut vt100::Parser)) {
        let caught =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op(&mut self.parser)));
        if caught.is_err() {
            self.parser = vt100::Parser::new(self.rows, self.cols, 0);
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Data(data) => {
                self.vt100_guarded(|parser| parser.process(&data));
                if !self.subscribers.is_empty() {
                    // Data chunks are at most the pump's 8 KiB read buffer
                    // (the debug highlighter expands them boundedly), far
                    // below the frame cap.
                    let frame = protocol::encode_frame(protocol::FRAME_OUTPUT, &data);
                    self.fan_out(&frame);
                }
            }
            Msg::Resize { rows, cols } => {
                let (rows, cols) = (rows.max(1), cols.max(1));
                self.rows = rows;
                self.cols = cols;
                self.vt100_guarded(|parser| parser.screen_mut().set_size(rows, cols));
                let frame = protocol::resize_frame(rows, cols);
                self.fan_out(&frame);
            }
            Msg::ScreenRequest(reply_tx) => {
                let _ = reply_tx.send(self.snapshot_blob());
            }
            Msg::Subscribe {
                id,
                frames_tx,
                input_granted,
            } => {
                let blob = self.snapshot_blob();
                if blob.len() > protocol::MAX_FRAME_LEN {
                    // A screen too large to frame: refuse the subscriber
                    // (its channel closes, the connection ends) rather than
                    // panic the parser thread.
                    return;
                }
                // Registry cap: refuse rather than grow without bound (see
                // [`MAX_SUBSCRIBERS`]). Dropping `frames_tx` here ends that
                // connection exactly as an overflowing queue does — its writer
                // thread sees the disconnect and shuts the socket down.
                if self.subscribers.len() >= MAX_SUBSCRIBERS {
                    return;
                }
                let hello = protocol::hello_frame(input_granted);
                let keyframe = protocol::encode_frame(protocol::FRAME_KEYFRAME, &blob);
                // A fresh queue always has room for these two frames.
                if frames_tx.try_send(hello).is_ok() && frames_tx.try_send(keyframe).is_ok() {
                    self.subscribers.push((id, frames_tx));
                }
            }
            Msg::Unsubscribe(id) => {
                // Dropping the sender is what ends the writer thread parked on
                // it; see [`Msg::Unsubscribe`] for why waiting for a fan-out
                // failure is not good enough.
                self.subscribers.retain(|(known, _)| *known != id);
            }
            Msg::Eof => {
                let end = protocol::end_frame();
                for (_, subscriber) in self.subscribers.drain(..) {
                    // Best-effort: dropping the sender is what actually ends
                    // each connection (writer drains, then EOF).
                    let _ = subscriber.try_send(end.clone());
                }
            }
        }
    }

    /// Encode the current screen, falling back to a blank snapshot of the
    /// tracked size if vt100 panics mid-render (same rationale as
    /// [`Self::vt100_guarded`]; `&self` here, so no rebuild — the next
    /// guarded mutation recovers the model).
    fn snapshot_blob(&self) -> Vec<u8> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            protocol::Snapshot::from_screen(self.parser.screen()).encode()
        }))
        .unwrap_or_else(|_| {
            protocol::Snapshot {
                rows: self.rows,
                cols: self.cols,
                cursor_row: 0,
                cursor_col: 0,
                rows_data: Vec::new(),
            }
            .encode()
        })
    }

    /// `try_send` to every subscriber, dropping any whose queue is full or
    /// whose connection is gone. Never blocks (see the invariant on
    /// [`spawn_parser_thread`]).
    fn fan_out(&mut self, frame: &[u8]) {
        self.subscribers
            .retain(|(_, subscriber)| subscriber.try_send(frame.to_vec()).is_ok());
    }
}

/// What an `accept()` error means for the accept loop.
///
/// The loop's lifetime is the shell's, and everything built on this socket —
/// the search popup and every `--active` attach — dies with it, so "is the
/// listener still usable?" has to be answered per error kind rather than
/// assumed. Treating *any* failure as terminal turned a passing condition
/// (out of file descriptors) into a permanently dead server.
enum AcceptFailure {
    /// This one connection failed, the listener is fine: EINTR (a signal
    /// landed mid-accept) or ECONNABORTED (the peer went away between its
    /// connect and our accept). Retry at once.
    Retry,
    /// The *process* is out of a resource, not the listener: EMFILE/ENFILE
    /// (fd table full), ENOMEM/ENOBUFS. Transient by nature — the fds
    /// holding the table down are released when their owners exit — so pause
    /// briefly and keep accepting. Also catches a spurious WouldBlock/TimedOut
    /// on a blocking listener, where the pause is what stops a hot spin.
    Pause,
    /// The listener itself is gone (EBADF, ENOTSOCK, EINVAL): there is
    /// nothing left to accept and no recovery, so the loop ends.
    Fatal,
}

/// Classify an `accept()` error. `raw_os_error` rather than [`io::ErrorKind`]
/// for the exhaustion cases: Rust has no stable `ErrorKind` for EMFILE/ENFILE
/// (they arrive as the unnameable `Uncategorized`), so matching on kind alone
/// would file them under "unknown" and kill the server.
fn classify_accept_error(e: &io::Error) -> AcceptFailure {
    match e.kind() {
        io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted => AcceptFailure::Retry,
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => AcceptFailure::Pause,
        _ => match e.raw_os_error() {
            Some(code)
                if code == nix::libc::EMFILE
                    || code == nix::libc::ENFILE
                    || code == nix::libc::ENOMEM
                    || code == nix::libc::ENOBUFS =>
            {
                AcceptFailure::Pause
            }
            _ => AcceptFailure::Fatal,
        },
    }
}

/// One live connection's slot in the [`MAX_LIVE_CONNECTIONS`] budget,
/// released when the connection's thread ends (including by panic).
struct ConnectionSlot(Arc<AtomicUsize>);

impl ConnectionSlot {
    /// Claim a slot, or `None` when the cap is already reached.
    fn acquire(live: &Arc<AtomicUsize>) -> Option<Self> {
        // `fetch_update` rather than fetch_add-then-undo: the count must never
        // read above the cap, even transiently, or two concurrent acquires
        // could both see room that only one of them has.
        live.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < MAX_LIVE_CONNECTIONS).then_some(n + 1)
        })
        .ok()
        .map(|_| Self(Arc::clone(live)))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Spawn the socket server on an already-bound listener.
///
/// # Accept-loop invariant
///
/// This loop does nothing but accept and hand off: **no per-connection work
/// happens here**, because every step of it can block on a peer that stopped
/// reading — the greeting sniff waits up to [`GREETING_SNIFF_TIMEOUT`], the
/// legacy reply waits on the parser and then on the client's socket buffer
/// (a screenful is larger than it), and a v2 connection lives for the whole
/// session. Serving any of that inline let one same-uid client that connects
/// and never reads wedge the loop, and with it *both* client kinds and
/// `--active`. Everything post-accept is therefore
/// [`serve_connection`], on its own thread; the loop owns only cloneable
/// handles.
///
/// # Survival invariant
///
/// The loop ends only when the listener is genuinely gone
/// ([`AcceptFailure::Fatal`]). Everything else — a signal, an aborted
/// connect, or the process running out of file descriptors — is retried,
/// because this server's death is permanent for the shell: no search popup
/// and no `--active` attach for as long as it lives. Handing off to a thread
/// per connection is what makes fd pressure reachable at all, so the two
/// belong together, along with [`MAX_LIVE_CONNECTIONS`], which stops one
/// same-uid client from creating that pressure in the first place.
pub(crate) fn spawn_socket_server(
    listener: UnixListener,
    msg_tx: SyncSender<Msg>,
    token: Vec<u8>,
    input_tx: SyncSender<Vec<u8>>,
) {
    std::thread::spawn(move || {
        let live = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(s) => s,
                Err(e) => match classify_accept_error(&e) {
                    AcceptFailure::Retry => continue,
                    AcceptFailure::Pause => {
                        std::thread::sleep(ACCEPT_RETRY_PAUSE);
                        continue;
                    }
                    AcceptFailure::Fatal => break,
                },
            };
            // Over the cap: close now. The peer gets a clean EOF (the popup's
            // `read_to_end` returns an empty snapshot and gives up) instead of
            // this process committing another fd and thread to it.
            let Some(slot) = ConnectionSlot::acquire(&live) else {
                drop(stream);
                continue;
            };
            let msg_tx = msg_tx.clone();
            let token = token.clone();
            let input_tx = input_tx.clone();
            std::thread::spawn(move || {
                // Held for the whole connection; released on any exit path.
                let _slot = slot;
                serve_connection(stream, &msg_tx, &token, &input_tx);
            });
        }
    });
}

/// One accepted connection, start to finish, on its own thread: sniff the v2
/// magic, then serve either the legacy one-shot snapshot (byte-identical to
/// the pre-v2 server) or the v2 subscriber protocol behind a peer-uid check.
///
/// Every failure here is local to this connection — a dead parser thread ends
/// this reply, not the accept loop.
fn serve_connection(
    mut stream: UnixStream,
    msg_tx: &SyncSender<Msg>,
    token: &[u8],
    input_tx: &SyncSender<Vec<u8>>,
) {
    let _ = stream.set_read_timeout(Some(GREETING_SNIFF_TIMEOUT));
    let mut first = [0u8; 4];
    let n = read_greeting(&mut stream, &mut first);

    match protocol::classify_greeting(&first[..n]) {
        protocol::Greeting::Legacy => {
            let (reply_tx, reply_rx) = mpsc::channel();
            if msg_tx.send(Msg::ScreenRequest(reply_tx)).is_err() {
                return;
            }
            if let Ok(data) = reply_rx.recv() {
                // Bound the write (see [`LEGACY_WRITE_TIMEOUT`]): a client
                // that never reads would otherwise park this thread and its
                // fd for the proxy's lifetime.
                let _ = stream.set_write_timeout(Some(LEGACY_WRITE_TIMEOUT));
                let _ = stream.write_all(&data);
                let _ = stream.flush();
            }
            // The reply terminates by the SERVER CLOSING: the popup client
            // (`fetch_screen_state`) does `read_to_end` and depends on this
            // EOF, which dropping `stream` at thread exit provides. Never
            // stash this stream anywhere.
        }
        protocol::Greeting::V2 => {
            // Keep a bounded timeout across the Subscribe read (see
            // [`SUBSCRIBE_TIMEOUT`]); `serve_subscriber` clears it once the
            // subscriber is registered.
            let _ = stream.set_read_timeout(Some(SUBSCRIBE_TIMEOUT));
            // Gate 2 of 3 (with the 0700 directory and the token): only the
            // same euid may speak v2. Fail closed when peer credentials are
            // unavailable. (Deliberately not applied to the legacy arm: those
            // bytes are frozen, byte-identical to the pre-v2 server.)
            if !peer_uid_matches(&stream) {
                return;
            }
            serve_subscriber(stream, msg_tx, token, input_tx);
        }
    }
}

/// Read up to 4 greeting bytes, stopping at EOF, timeout, or error. Returns
/// how many bytes were read; fewer than 4 always classifies as legacy.
fn read_greeting(stream: &mut UnixStream, buf: &mut [u8; 4]) -> usize {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            // Timeout (WouldBlock/TimedOut) or a hard error: stop sniffing.
            Err(_) => break,
        }
    }
    filled
}

/// One v2 connection's reader side, running on the connection's own thread
/// (see [`serve_connection`]): validate the Subscribe (still under
/// [`SUBSCRIBE_TIMEOUT`], so a client that greets and idles cannot park this
/// thread forever), register with the parser, then loop on authenticated
/// Input frames. Any protocol violation (unknown frame, duplicate Subscribe,
/// Input without a grant) closes the connection.
///
/// Registration is strictly paired: once [`Msg::Subscribe`] is sent, every
/// exit path sends the matching [`Msg::Unsubscribe`], so the registry never
/// holds an entry for a connection that is over.
fn serve_subscriber(
    mut stream: UnixStream,
    msg_tx: &SyncSender<Msg>,
    token: &[u8],
    input_tx: &SyncSender<Vec<u8>>,
) {
    let Ok(Some((frame_type, payload))) = protocol::read_frame(&mut stream) else {
        return;
    };
    if frame_type != protocol::FRAME_SUBSCRIBE {
        return;
    }
    let Some((want_input, client_token)) = protocol::decode_subscribe(&payload) else {
        return;
    };
    let input_granted = want_input && !token.is_empty() && client_token == token;

    let id = NEXT_SUBSCRIBER_ID.fetch_add(1, Ordering::Relaxed);
    let (frames_tx, frames_rx) = mpsc::sync_channel::<Vec<u8>>(SUBSCRIBER_QUEUE_FRAMES);
    if msg_tx
        .send(Msg::Subscribe {
            id,
            frames_tx,
            input_granted,
        })
        .is_err()
    {
        return;
    }
    relay_subscriber_input(stream, frames_rx, input_tx, input_granted);
    // The connection is over: release the registry slot (and with it the
    // writer thread parked on the frame channel) now, rather than waiting for
    // some later frame to fail on a dead socket.
    let _ = msg_tx.send(Msg::Unsubscribe(id));
}

/// The registered half of a v2 connection: pump authenticated Input frames
/// until the connection ends. Split out of [`serve_subscriber`] so that
/// function has exactly one post-registration exit to pair the unsubscribe
/// with.
fn relay_subscriber_input(
    mut stream: UnixStream,
    frames_rx: Receiver<Vec<u8>>,
    input_tx: &SyncSender<Vec<u8>>,
    input_granted: bool,
) {
    // Registered: clear the handshake timeout — steady-state Input reads
    // may legitimately idle for hours (or forever, on a read-only tap).
    let _ = stream.set_read_timeout(None);

    let Ok(writer_stream) = stream.try_clone() else {
        return;
    };
    std::thread::spawn(move || write_frames(writer_stream, &frames_rx));

    loop {
        match protocol::read_frame(&mut stream) {
            Ok(Some((protocol::FRAME_INPUT, data))) if input_granted => {
                // Input is lossless: blocking send into the pty-writer
                // thread's queue. Safe on this per-connection thread.
                if input_tx.send(data).is_err() {
                    break;
                }
            }
            // EOF, error, Input without a grant, or any other frame.
            _ => break,
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// One v2 connection's writer side: drain pre-framed bytes to the socket.
/// Exits when the parser drops the subscriber (queue overflow or session
/// end) or the socket dies, then shuts the connection down so the reader
/// side unblocks.
fn write_frames(mut stream: UnixStream, frames_rx: &Receiver<Vec<u8>>) {
    for frame in frames_rx {
        if stream.write_all(&frame).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly"
))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    use nix::sys::socket::{getsockopt, sockopt::LocalPeerCred};
    getsockopt(stream, LocalPeerCred)
        .ok()
        .map(|cred| cred.uid())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    getsockopt(stream, PeerCredentials)
        .ok()
        .map(|cred| cred.uid())
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "linux",
    target_os = "android"
)))]
fn peer_uid(_stream: &UnixStream) -> Option<u32> {
    // No peer-credential API known for this platform: fail closed (v2 is
    // refused; the legacy snapshot path still works).
    None
}

fn peer_uid_matches(stream: &UnixStream) -> bool {
    peer_uid(stream) == Some(nix::unistd::geteuid().as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_dimension_screens_do_not_panic() {
        // Debug builds have overflow checks; vt100 0.16 computes
        // `size.rows - 1` internally, so an unclamped 0x0 grid panics at
        // construction and at set_size. The clamp floors both at 1x1.
        // (Text that *wraps* on a 1-row screen panics inside vt100's
        // `col_wrap` — a distinct upstream bug, absorbed by
        // `vt100_guarded` and covered by the test below — so this test
        // writes a single cell to exercise the clamps alone.)
        let mut state = ParserState::new(0, 0);
        state.handle(Msg::Data(b"h".to_vec()));
        state.handle(Msg::Resize { rows: 0, cols: 0 });
        state.handle(Msg::Data(b"\x1b[Hi".to_vec()));

        let (reply_tx, reply_rx) = mpsc::channel();
        state.handle(Msg::ScreenRequest(reply_tx));
        let blob = reply_rx.recv().unwrap();
        let snapshot = protocol::Snapshot::decode(&blob).unwrap();
        assert_eq!((snapshot.rows, snapshot.cols), (1, 1));
    }

    /// vt100 0.16 panics in `col_wrap` when text wraps on a 1-row grid.
    /// The guard must absorb it: the parser thread is the sole owner of the
    /// screen model, so an unwinding one leaves every connection thread with
    /// nothing to serve — no snapshots, no subscribers, and `--active` gone
    /// for the shell's remaining lifetime. Raw-byte fan-out and the snapshot
    /// service must both survive the panic.
    #[test]
    fn vt100_wrap_panic_does_not_kill_the_parser() {
        let mut state = ParserState::new(1, 3);
        let (frames_tx, frames_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE_FRAMES);
        state.handle(Msg::Subscribe {
            id: 1,
            frames_tx,
            input_granted: false,
        });
        while frames_rx.try_recv().is_ok() {} // drain hello + keyframe

        // Wraps several times on the 1x3 grid: the upstream panic path.
        state.handle(Msg::Data(b"wrap this line".to_vec()));

        // The raw bytes still fanned out to the subscriber...
        let frame = frames_rx.try_recv().expect("fan-out survives the panic");
        assert_eq!(frame[0], protocol::FRAME_OUTPUT);

        // ...and the parser still serves snapshots at the tracked size.
        let (reply_tx, reply_rx) = mpsc::channel();
        state.handle(Msg::ScreenRequest(reply_tx));
        let blob = reply_rx.recv().expect("parser still answering");
        let snapshot = protocol::Snapshot::decode(&blob).unwrap();
        assert_eq!((snapshot.rows, snapshot.cols), (1, 3));
    }

    #[test]
    fn resize_is_clamped_and_fanned_out() {
        let mut state = ParserState::new(24, 80);
        let (frames_tx, frames_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE_FRAMES);
        state.handle(Msg::Subscribe {
            id: 1,
            frames_tx,
            input_granted: false,
        });
        // Hello + keyframe are queued immediately.
        let hello = frames_rx.try_recv().unwrap();
        assert_eq!(hello[0], protocol::FRAME_HELLO);
        let keyframe = frames_rx.try_recv().unwrap();
        assert_eq!(keyframe[0], protocol::FRAME_KEYFRAME);

        state.handle(Msg::Resize { rows: 0, cols: 0 });
        let resize = frames_rx.try_recv().unwrap();
        let mut cursor = std::io::Cursor::new(resize);
        let (frame_type, payload) = protocol::read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(frame_type, protocol::FRAME_RESIZE);
        assert_eq!(protocol::decode_resize(&payload), Some((1, 1)));
    }

    #[test]
    fn full_subscriber_queue_drops_the_subscriber() {
        let mut state = ParserState::new(2, 10);
        let (frames_tx, frames_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE_FRAMES);
        state.handle(Msg::Subscribe {
            id: 1,
            frames_tx,
            input_granted: false,
        });
        // Fill the queue without draining it; the subscriber must be
        // dropped, closing its channel.
        for _ in 0..=SUBSCRIBER_QUEUE_FRAMES {
            state.handle(Msg::Data(b"x".to_vec()));
        }
        // Drain everything that was queued before the drop...
        while frames_rx.try_recv().is_ok() {}
        // ...after which the channel reports disconnection.
        assert!(matches!(
            frames_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    /// The registry is capped: a same-uid client cannot accumulate
    /// subscribers (each one an fd, two threads, and a clone of every frame)
    /// without bound. The refusal ends the refused connection the same way an
    /// overflowing queue does — by dropping its channel — and leaves the
    /// subscribers already registered untouched.
    #[test]
    fn subscriber_registry_refuses_past_its_cap_and_frees_slots_on_unsubscribe() {
        let mut state = ParserState::new(2, 10);
        let mut accepted = Vec::new();
        for id in 0..MAX_SUBSCRIBERS as SubscriberId {
            let (frames_tx, frames_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE_FRAMES);
            state.handle(Msg::Subscribe {
                id,
                frames_tx,
                input_granted: false,
            });
            assert_eq!(frames_rx.recv().unwrap()[0], protocol::FRAME_HELLO);
            assert_eq!(frames_rx.recv().unwrap()[0], protocol::FRAME_KEYFRAME);
            accepted.push(frames_rx);
        }
        assert_eq!(state.subscribers.len(), MAX_SUBSCRIBERS);

        let (frames_tx, refused_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE_FRAMES);
        state.handle(Msg::Subscribe {
            id: 999,
            frames_tx,
            input_granted: false,
        });
        assert_eq!(state.subscribers.len(), MAX_SUBSCRIBERS, "cap holds");
        assert!(
            matches!(refused_rx.recv(), Err(mpsc::RecvError)),
            "the refused subscriber's channel closes, ending its connection"
        );

        // The ones already registered keep receiving.
        state.handle(Msg::Data(b"x".to_vec()));
        for frames_rx in &accepted {
            assert_eq!(frames_rx.recv().unwrap()[0], protocol::FRAME_OUTPUT);
        }

        // A finished connection frees its slot immediately — NOT at the next
        // fan-out, which on an idle shell may never come. Without this an
        // `--active` client that reconnects in silence could exhaust the cap
        // against itself.
        state.handle(Msg::Unsubscribe(0));
        assert_eq!(state.subscribers.len(), MAX_SUBSCRIBERS - 1);
        assert!(
            matches!(accepted[0].recv(), Err(mpsc::RecvError)),
            "unsubscribing drops the channel, ending that connection's writer"
        );
        // An unsubscribe for an id that is not registered changes nothing (a
        // late duplicate must never evict a live subscriber).
        state.handle(Msg::Unsubscribe(0));
        state.handle(Msg::Unsubscribe(999));
        assert_eq!(state.subscribers.len(), MAX_SUBSCRIBERS - 1);

        // ...and the freed slot is usable.
        let (frames_tx, frames_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE_FRAMES);
        state.handle(Msg::Subscribe {
            id: 1000,
            frames_tx,
            input_granted: false,
        });
        assert_eq!(state.subscribers.len(), MAX_SUBSCRIBERS);
        assert_eq!(frames_rx.recv().unwrap()[0], protocol::FRAME_HELLO);
    }

    /// Only a dead listener ends the accept loop. Every other `accept()`
    /// failure is transient, and treating it as fatal killed the socket
    /// server — and with it the search popup and `--active` — for the
    /// shell's remaining lifetime.
    #[test]
    fn accept_errors_are_fatal_only_when_the_listener_is_gone() {
        use nix::libc;

        let classify = |code| classify_accept_error(&io::Error::from_raw_os_error(code));

        // Out of file descriptors: pause and keep accepting. Note the kind is
        // NOT one of the named `ErrorKind`s, which is exactly why the
        // classifier looks at the raw errno.
        for code in [libc::EMFILE, libc::ENFILE, libc::ENOMEM, libc::ENOBUFS] {
            assert!(
                matches!(classify(code), AcceptFailure::Pause),
                "errno {code} must pause, not kill the server"
            );
        }
        // A signal or a peer that gave up between connect and accept.
        for code in [libc::EINTR, libc::ECONNABORTED] {
            assert!(
                matches!(classify(code), AcceptFailure::Retry),
                "errno {code}"
            );
        }
        // The listener itself is gone: nothing left to accept.
        for code in [libc::EBADF, libc::EINVAL, libc::ENOTSOCK] {
            assert!(
                matches!(classify(code), AcceptFailure::Fatal),
                "errno {code}"
            );
        }
    }

    /// The connection budget admits up to the cap, refuses past it, and
    /// recovers as slots are released — a guard's `Drop` is what returns one,
    /// so a panicking connection thread cannot leak its slot.
    #[test]
    fn connection_slots_are_capped_and_released() {
        let live = Arc::new(AtomicUsize::new(0));
        let mut held: Vec<ConnectionSlot> = (0..MAX_LIVE_CONNECTIONS)
            .map(|_| ConnectionSlot::acquire(&live).expect("under the cap"))
            .collect();
        assert_eq!(live.load(Ordering::Acquire), MAX_LIVE_CONNECTIONS);
        assert!(
            ConnectionSlot::acquire(&live).is_none(),
            "the cap must hold"
        );

        held.pop();
        assert_eq!(live.load(Ordering::Acquire), MAX_LIVE_CONNECTIONS - 1);
        let reacquired = ConnectionSlot::acquire(&live).expect("a slot freed up");
        assert_eq!(live.load(Ordering::Acquire), MAX_LIVE_CONNECTIONS);

        drop(held);
        assert_eq!(live.load(Ordering::Acquire), 1, "the re-acquired slot");
        drop(reacquired);
        assert_eq!(live.load(Ordering::Acquire), 0);
    }

    #[test]
    fn eof_broadcasts_end_and_drops_subscribers() {
        let mut state = ParserState::new(2, 10);
        let (frames_tx, frames_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE_FRAMES);
        state.handle(Msg::Subscribe {
            id: 1,
            frames_tx,
            input_granted: true,
        });
        state.handle(Msg::Eof);

        let hello = frames_rx.recv().unwrap();
        assert_eq!(hello[0], protocol::FRAME_HELLO);
        assert_eq!(hello[6], 1, "hello carries the input grant");
        let keyframe = frames_rx.recv().unwrap();
        assert_eq!(keyframe[0], protocol::FRAME_KEYFRAME);
        let end = frames_rx.recv().unwrap();
        assert_eq!(end, protocol::end_frame());
        assert!(frames_rx.recv().is_err(), "registry dropped the sender");
    }

    #[test]
    fn proxy_dir_and_token_have_tight_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proxy");
        create_proxy_dir(&dir).unwrap();
        let token = write_token(&dir).unwrap();

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);

        let token_path = token_path_in(&dir);
        let file_mode = std::fs::metadata(&token_path).unwrap().permissions().mode();
        assert_eq!(file_mode & 0o777, 0o600);

        assert_eq!(token.len(), 64);
        assert!(token.iter().all(u8::is_ascii_hexdigit));
        assert_eq!(std::fs::read(&token_path).unwrap(), token);
    }

    #[test]
    fn create_proxy_dir_replaces_a_stale_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proxy");
        create_proxy_dir(&dir).unwrap();
        write_token(&dir).unwrap();
        // A second create (pid reuse) must not fail on the leftovers.
        create_proxy_dir(&dir).unwrap();
        assert!(!token_path_in(&dir).exists());
    }

    #[test]
    fn socket_path_length_validation() {
        assert!(socket_path_fits(Path::new("/tmp/atuin-pty-proxy-1/sock")));
        let long = format!("/{}/sock", "x".repeat(120));
        assert!(!socket_path_fits(Path::new(&long)));
    }
}
