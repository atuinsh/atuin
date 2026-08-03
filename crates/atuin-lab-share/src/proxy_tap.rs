//! The `--active` source: a subscriber tap on a running `atuin pty-proxy`.
//!
//! The proxy owns the PTY master of the user's shell from startup, keeps a
//! live vt100 model of its screen, and serves the framed subscriber protocol
//! on `$ATUIN_PTY_PROXY_SOCKET` (see [`atuin_pty_proxy::protocol`]). The tap
//! connects, subscribes, and adapts that feed to the session's
//! [`SourceParts`] shape:
//!
//! * the first Keyframe becomes the session's `bootstrap` chunk (rendered to
//!   ANSI repaint bytes by [`protocol::snapshot_to_ansi`]), and its header
//!   fixes the session geometry — the tap's full size, no bar row reserved;
//! * `Output` frames surface as [`ReadEvent::Output`]; `Resize` frames as
//!   [`ReadEvent::Resize`], **in wire order** — the proxy's parser thread
//!   serialized them, and delivering them through one stream is what keeps
//!   the session's screen model applying a resize exactly between the bytes
//!   written before and after it (see the ordering invariant on
//!   [`ReadEvent`]). The owning terminal is authoritative, so the session
//!   ignores hub `set_size` asks (`follows_hub_resize: false`) and the
//!   resizer is a no-op;
//! * `End` or EOF triggers a bounded reconnect (the proxy drops subscribers
//!   that fall behind; see the resync design in the proxy's protocol docs).
//!   A successful reconnect repaints viewers from the fresh Keyframe; a
//!   failed one is the end of the session, reported as reader end plus the
//!   `wait` closure returning 0;
//! * `stop` only **detaches** — the tapped shell is the user's login shell,
//!   never ours to kill;
//! * `answer_queries` is false: the proxy's real terminal answers CPR/DA
//!   probes itself, and a synthetic reply here would double-answer.

use std::io::{self, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::time::Duration;

use atuin_pty_proxy::protocol::{
    self, FRAME_END, FRAME_HELLO, FRAME_KEYFRAME, FRAME_OUTPUT, FRAME_RESIZE, MAGIC, Snapshot,
};

use crate::render::WriteMode;
use crate::source::{ReadEvent, SessionSource, SourceParts, SourceReader};
use crate::{Error, Size};

/// How many connection attempts the reader makes after losing the feed
/// before declaring the session over.
const RECONNECT_ATTEMPTS: u32 = 5;
/// Pause between reconnect attempts (the first one is immediate).
const RECONNECT_DELAY: Duration = Duration::from_millis(250);
/// Read timeout while the attach handshake runs: a live proxy queues Hello
/// and the Keyframe before we can even ask, so a slow answer means a wedged
/// proxy and must fail the attach rather than hang it. Cleared before
/// streaming starts — steady-state reads may legitimately idle for hours.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

/// State shared between the reader (which replaces the stream on reconnect),
/// the writer (which frames input onto the current stream), and the
/// session's `stop` closure (which shuts the current stream down).
///
/// Locking discipline: critical sections only clone, replace, or shut down
/// the stream — never a blocking read or write — so `stop`, which runs
/// inline on the session's select task, can never be held up behind socket
/// backpressure.
struct TapShared {
    /// The connection currently carrying the feed. The reader swaps in a
    /// fresh stream on every successful resync, so `stop` and the writer
    /// always operate on the live socket, not a dead ancestor.
    stream: Mutex<UnixStream>,
    /// Set by `stop`: detach for good. The reader checks it before and after
    /// every reconnect attempt, so a detach can never race into a fresh
    /// subscription that outlives the session.
    stopped: AtomicBool,
}

/// A live, subscribed connection to the proxy, holding the first keyframe.
///
/// Lives only between [`ProxyTap::attach`] and
/// [`SessionSource::into_parts`], exactly like `Subshell`.
pub(crate) struct ProxyTap {
    shared: Arc<TapShared>,
    /// The clone of the subscribed stream the reader will consume.
    read_stream: UnixStream,
    /// The first keyframe: fixes the session geometry and becomes the
    /// bootstrap chunk.
    snapshot: Snapshot,
    /// The session geometry, taken from `snapshot` and **validated** (never
    /// clamped) by [`attach_size`] while [`ProxyTap::attach`] could still
    /// refuse. See that function for why this one path refuses where every
    /// later geometry is floored.
    size: Size,
    socket_path: PathBuf,
    want_input: bool,
    token: Vec<u8>,
}

impl ProxyTap {
    /// Connect to the proxy socket, subscribe, and read the first keyframe.
    ///
    /// `--write` asks for input access, authenticated by the token file next
    /// to the socket; anything less than a full grant is an error — a share
    /// that silently downgraded to read-only would break the host's promise
    /// to their viewers.
    ///
    /// # Errors
    ///
    /// [`Error::ActiveShareUnsupported`] when nothing answers on the socket
    /// (the ladder's probe raced a dying proxy), [`Error::ProxyTooOld`] /
    /// [`Error::ProxyUnresponsive`] / [`Error::ProxyHandshake`] /
    /// [`Error::ProxyInputDenied`] from the handshake,
    /// [`Error::ProxyTerminalTooSmall`] when the keyframe header reports a
    /// geometry below the `vt100` floor (see [`attach_size`]), and
    /// [`Error::ProxyToken`] when `--write` cannot read the token file.
    ///
    /// [`Error::ProxyTooOld`] and [`Error::ProxyUnresponsive`] are kept
    /// apart on purpose: the first is permanent for this shell (its proxy
    /// binary predates the protocol), the second is transient (something
    /// accepted on the socket and did not answer in time). Conflating them
    /// told users to end their session over a condition that clears by
    /// itself.
    pub(crate) fn attach(socket_path: &Path, write: WriteMode) -> crate::Result<Self> {
        let want_input = write.is_write_enabled();
        let token = read_token(socket_path, want_input)?;
        let mut stream =
            UnixStream::connect(socket_path).map_err(|_| Error::ActiveShareUnsupported)?;
        let snapshot = handshake(&mut stream, want_input, &token)?;
        // Refuse a degenerate geometry HERE, while refusing is still free:
        // nothing has been minted, no link exists (see [`attach_size`]).
        let size = attach_size(&snapshot)?;
        let read_stream = stream.try_clone()?;
        Ok(Self {
            shared: Arc::new(TapShared {
                stream: Mutex::new(stream),
                stopped: AtomicBool::new(false),
            }),
            read_stream,
            snapshot,
            size,
            socket_path: socket_path.to_path_buf(),
            want_input,
            token,
        })
    }

    /// The tap's geometry, from the keyframe header: the proxied terminal's
    /// full size, exactly as the proxy reported it. No bar row is reserved —
    /// a headless session draws no bar — and nothing is clamped, because
    /// [`attach_size`] already refused everything below the floor.
    pub(crate) fn size(&self) -> Size {
        self.size
    }
}

/// The starting geometry for a fresh attach: the proxy's keyframe header,
/// **refused** rather than floored when it is below what `vt100` survives.
///
/// The one geometry path that can still say no. It runs inside
/// [`ProxyTap::attach`], which happens before `connect_to_hub` — no session
/// minted, no link printed, nothing in the world to keep alive — so the
/// subshell path's rule applies unchanged: a session too small to render is
/// better declined than silently shown at a size the host cannot read (see
/// `host_size_from`). Clamping here would instead hand viewers a screen a row
/// taller than the host's real terminal, silently, for the whole session.
///
/// This is genuinely reachable: `ParserState::new` clamps to `(1, 1)`, so an
/// `Nx1` keyframe header is a real value the proxy can send.
///
/// Every *later* geometry — the mid-stream `Resize` frame and the
/// post-reconnect resync — goes through [`clamp_tap_size`] instead, because by
/// then the link IS out in the world and a transient shrink must not kill a
/// live session.
///
/// # Errors
///
/// [`Error::ProxyTerminalTooSmall`], quoting back the proxy's reported size.
fn attach_size(snapshot: &Snapshot) -> crate::Result<Size> {
    if snapshot.cols < crate::MIN_COLS || snapshot.rows < crate::MIN_CHILD_ROWS {
        return Err(Error::ProxyTerminalTooSmall {
            cols: snapshot.cols,
            rows: snapshot.rows,
        });
    }
    Ok(Size {
        cols: snapshot.cols,
        rows: snapshot.rows,
    })
}

/// Floor a proxy-supplied geometry at what `vt100` survives.
///
/// **Clamp, never refuse.** The proxy owns the terminal, its geometry is
/// authoritative, and these arrive asynchronously (`Resize` frame,
/// post-reconnect resync, an out-of-band keyframe) — long after the join link
/// is out in the world. So the tap absorbs the degenerate size instead of
/// ending a live session over it. The one geometry that arrives *before* the
/// link exists is the attach keyframe, and that one refuses: see
/// [`attach_size`].
///
/// The floor is [`crate::MIN_CHILD_ROWS`] rows, not one. The pty-proxy's own
/// clamp is `(1, 1)` and that is *its* contract — pinned by its
/// `zero_dimension_screens_do_not_panic` and `resize_is_clamped_and_fanned_out`
/// tests — but 1x1 is precisely one of the sizes that panics the session's
/// vt100 model (`grid.rs`, subtract-overflow). Raising the floor belongs here,
/// on the consuming side, not in the sibling crate.
///
/// No bar row is subtracted: the tap feeds a headless session, which draws no
/// bar.
fn clamp_tap_size(cols: u16, rows: u16) -> Size {
    Size {
        cols: cols.max(crate::MIN_COLS),
        rows: rows.max(crate::MIN_CHILD_ROWS),
    }
}

/// A snapshot header as a [`Size`], floored (see [`clamp_tap_size`]).
fn snapshot_size(snapshot: &Snapshot) -> Size {
    clamp_tap_size(snapshot.cols, snapshot.rows)
}

impl SessionSource for ProxyTap {
    /// Split the tap into the pieces the session's topology needs. See the
    /// module docs for how each [`SourceParts`] invariant is met.
    fn into_parts(self) -> crate::Result<SourceParts> {
        let Self {
            shared,
            read_stream,
            snapshot,
            // Already consumed by `run_share_active` (via `ProxyTap::size`)
            // to size the session; the reader tracks geometry from here on.
            size: _,
            socket_path,
            want_input,
            token,
        } = self;
        // The end-of-feed signal: the reader drops its sender once the feed
        // is finished (End frame, EOF, or a failed resync), unblocking the
        // `wait` closure on the blocking pool — the session's authoritative
        // end signal, on every path.
        let (eof_tx, eof_rx) = std_mpsc::channel::<()>();
        let bootstrap = protocol::snapshot_to_ansi(&snapshot);
        let reader = TapReader {
            shared: Arc::clone(&shared),
            stream: read_stream,
            pending: Vec::new(),
            eof_tx: Some(eof_tx),
            socket_path,
            want_input,
            token,
        };
        let writer = TapWriter {
            shared: Arc::clone(&shared),
        };
        let stop_shared = shared;
        Ok(SourceParts {
            reader: Box::new(reader),
            writer: Box::new(writer),
            // The tapped terminal is owned elsewhere; nothing of ours to
            // resize.
            resizer: Box::new(|_| {}),
            // Detach, never kill: mark the tap stopped first (so the reader
            // refuses to resync), then shut the live socket down to unblock
            // its read. The user's shell never notices.
            stop: Box::new(move || {
                stop_shared.stopped.store(true, Ordering::SeqCst);
                if let Ok(stream) = stop_shared.stream.lock() {
                    let _ = stream.shutdown(Shutdown::Both);
                }
            }),
            // Tap end is EOF, not an exit code: there is no child of ours to
            // report on, so the answer is always 0.
            wait: Box::new(move || {
                let _ = eof_rx.recv();
                0
            }),
            bootstrap: Some(bootstrap),
            answer_queries: false,
            follows_hub_resize: false,
        })
    }
}

/// The token is the `token` sibling of the socket (64 ASCII hex bytes, sent
/// exactly as read). Read-only subscriptions never request the input grant,
/// so they send an empty token and never touch the file.
fn read_token(socket_path: &Path, want_input: bool) -> crate::Result<Vec<u8>> {
    if !want_input {
        return Ok(Vec::new());
    }
    let path = socket_path
        .parent()
        .map_or_else(|| PathBuf::from("token"), |dir| dir.join("token"));
    std::fs::read(&path).map_err(|source| Error::ProxyToken { path, source })
}

/// The subscribe handshake: magic + Subscribe out, Hello + the first
/// Keyframe back, under [`HANDSHAKE_TIMEOUT`] (cleared before returning).
///
/// A pre-subscriber proxy never answers with a Hello frame: it ignores what
/// we wrote, serves its one-shot snapshot blob, and closes — the blob's
/// first bytes parse as an absurd frame header, and a fast one may even close
/// before our greeting lands (EPIPE on the writes; on macOS even the timeout
/// setsockopt fails once the peer is gone). So a greeting failure, a clean
/// EOF, a decode error, or a non-Hello first frame all mean
/// [`Error::ProxyTooOld`].
///
/// **Silence is not age.** A peer that accepts and then never answers is
/// alive, whatever it is, and says so through [`is_read_timeout`] ->
/// [`Error::ProxyUnresponsive`]: telling that user to start a new shell would
/// destroy a working session over a condition that may clear on its own. What
/// the silence *means* is deliberately left unstated there — see that
/// variant's copy. Both
/// reads of the handshake are classified this way — the first frame and the
/// keyframe loop — so a proxy that stalls *after* its Hello reads the same as
/// one that stalls before it.
///
/// Unknown frame types between Hello and the Keyframe are skipped (forward
/// compatibility).
fn handshake(stream: &mut UnixStream, want_input: bool, token: &[u8]) -> crate::Result<Snapshot> {
    if stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).is_err()
        || stream.write_all(&MAGIC).is_err()
        || stream
            .write_all(&protocol::subscribe_frame(want_input, token))
            .is_err()
    {
        return Err(Error::ProxyTooOld);
    }

    let (frame_type, payload) = match protocol::read_frame(stream) {
        Ok(Some(frame)) => frame,
        // Nothing at all within the timeout: alive but wedged, not old.
        Err(e) if is_read_timeout(&e) => return Err(Error::ProxyUnresponsive),
        // A clean EOF at a frame boundary, the `InvalidData` an old proxy's
        // snapshot blob parses as, an `UnexpectedEof` mid-frame: all of them
        // are a server that answered with something other than this protocol.
        Ok(None) | Err(_) => return Err(Error::ProxyTooOld),
    };
    if frame_type != FRAME_HELLO {
        return Err(Error::ProxyTooOld);
    }
    let (_version, input_granted) = protocol::decode_hello(&payload).ok_or(Error::ProxyTooOld)?;
    if want_input && !input_granted {
        return Err(Error::ProxyInputDenied);
    }

    loop {
        match protocol::read_frame(stream) {
            Ok(Some((FRAME_KEYFRAME, payload))) => {
                let snapshot = Snapshot::decode(&payload)
                    .ok_or_else(|| Error::ProxyHandshake("malformed keyframe".into()))?;
                stream.set_read_timeout(None)?;
                return Ok(snapshot);
            }
            Ok(Some((FRAME_END, _))) | Ok(None) => {
                return Err(Error::ProxyHandshake(
                    "the session ended before the first keyframe".into(),
                ));
            }
            Ok(Some(_)) => {} // unknown server frame: skip
            // Same split as above: a proxy that greets and then stalls is
            // busy, not broken — surfacing that as "invalid reply while
            // attaching: Resource temporarily unavailable" helps nobody.
            Err(e) if is_read_timeout(&e) => return Err(Error::ProxyUnresponsive),
            Err(e) => return Err(Error::ProxyHandshake(e.to_string())),
        }
    }
}

/// Whether an io error is a read timeout, i.e. the peer went silent rather
/// than misbehaving.
///
/// Both kinds must be matched: a timed-out socket read surfaces as
/// `WouldBlock` on some platforms and `TimedOut` on others. The kinds are
/// reliable here because `protocol::read_frame` -> `read_exact_or_eof`
/// propagates the raw `io::Error` untouched.
fn is_read_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// The blocking reader half of the tap, run by the session's detached
/// pty-reader thread. Decodes the framed feed into the session's ordered
/// event stream: `Output` payloads become [`ReadEvent::Output`], `Resize`
/// frames become [`ReadEvent::Resize`] **at their exact position in the
/// stream** (the proxy serialized them; splitting them onto a side channel
/// would let the session apply a resize before or after the redraw bytes it
/// belongs between, mis-wrapping the model and every keyframe minted from
/// it). `End`/EOF/error runs the bounded resync — once that fails (or `stop`
/// detached the tap on purpose) it wakes the `wait` closure and reports
/// `Ok(None)`.
struct TapReader {
    shared: Arc<TapShared>,
    /// The reader's clone of the current connection (swapped on resync).
    stream: UnixStream,
    /// Output bytes queued ahead of the socket: the resync repaint, delivered
    /// as the event after its geometry-announcing `Resize`.
    pending: Vec<u8>,
    /// Dropped when the feed is finished; the `wait` closure blocks on the
    /// receiving end.
    eof_tx: Option<std_mpsc::Sender<()>>,
    socket_path: PathBuf,
    want_input: bool,
    token: Vec<u8>,
}

impl TapReader {
    /// Try to re-establish the feed after losing the socket: reconnect and
    /// re-subscribe, bounded ([`RECONNECT_ATTEMPTS`] tries,
    /// [`RECONNECT_DELAY`] apart). On success the keyframe geometry is
    /// returned — the caller surfaces it as a `Resize` event, in case the
    /// terminal changed size while the feed was down — and the fresh
    /// keyframe is queued as clear-screen-plus-repaint bytes delivered on
    /// the very next read, **after** that resize: viewers see one clean
    /// repaint at the right geometry, and the session's seq continuity is
    /// untouched because the bytes flow through the normal reader path.
    /// Returns `None` when the session should end: every attempt failed, or
    /// `stop` detached the tap on purpose.
    fn resync(&mut self) -> Option<Size> {
        for attempt in 0..RECONNECT_ATTEMPTS {
            if self.shared.stopped.load(Ordering::SeqCst) {
                return None; // deliberate detach: never resubscribe
            }
            if attempt > 0 {
                std::thread::sleep(RECONNECT_DELAY);
            }
            let Ok(mut stream) = UnixStream::connect(&self.socket_path) else {
                continue;
            };
            let Ok(snapshot) = handshake(&mut stream, self.want_input, &self.token) else {
                continue;
            };
            let Ok(read_clone) = stream.try_clone() else {
                continue;
            };
            if let Ok(mut current) = self.shared.stream.lock() {
                *current = stream;
            }
            // `stop` may have fired between the check at the top and the
            // swap above — it would have shut down the OLD stream to no
            // effect. Re-check now that the swap is visible, and close the
            // fresh connection ourselves.
            if self.shared.stopped.load(Ordering::SeqCst) {
                let _ = read_clone.shutdown(Shutdown::Both);
                return None;
            }
            self.stream = read_clone;
            self.pending = protocol::snapshot_to_ansi(&snapshot);
            return Some(snapshot_size(&snapshot));
        }
        None
    }

    /// The feed is over: wake the `wait` closure, then report the end. The
    /// pty-reader thread returns on `Ok(None)` and drops us.
    fn finish(&mut self) -> io::Result<Option<ReadEvent>> {
        self.eof_tx.take();
        Ok(None)
    }
}

impl SourceReader for TapReader {
    fn read_event(&mut self) -> io::Result<Option<ReadEvent>> {
        loop {
            if !self.pending.is_empty() {
                return Ok(Some(ReadEvent::Output(std::mem::take(&mut self.pending))));
            }
            match protocol::read_frame(&mut self.stream) {
                Ok(Some((FRAME_OUTPUT, payload))) => {
                    if !payload.is_empty() {
                        return Ok(Some(ReadEvent::Output(payload)));
                    }
                }
                Ok(Some((FRAME_RESIZE, payload))) => {
                    if let Some((rows, cols)) = protocol::decode_resize(&payload) {
                        return Ok(Some(ReadEvent::Resize(clamp_tap_size(cols, rows))));
                    }
                }
                Ok(Some((FRAME_KEYFRAME, payload))) => {
                    // Not part of the steady-state protocol today, but a
                    // keyframe is always safe to honour: its geometry first,
                    // then its repaint bytes (queued in `pending`).
                    if let Some(snapshot) = Snapshot::decode(&payload) {
                        self.pending = protocol::snapshot_to_ansi(&snapshot);
                        return Ok(Some(ReadEvent::Resize(snapshot_size(&snapshot))));
                    }
                }
                Ok(Some((FRAME_END, _))) | Ok(None) => match self.resync() {
                    // Fresh geometry first; the repaint follows from
                    // `pending` on the next read.
                    Some(size) => return Ok(Some(ReadEvent::Resize(size))),
                    None => return self.finish(),
                },
                Ok(Some(_)) => {} // unknown server frame: skip (forward compat)
                Err(_) => match self.resync() {
                    Some(size) => return Ok(Some(ReadEvent::Resize(size))),
                    None => return self.finish(),
                },
            }
        }
    }
}

/// The writer half: frames session input as `Input` frames onto the current
/// connection. Runs on the detached pty-writer thread, so blocking is fine —
/// but never while holding the stream lock (see [`TapShared`]): the stream
/// is cloned under the lock and written outside it. Write errors are
/// swallowed (`Ok`): during a reconnect window the socket is dead on
/// purpose, and an error here would kill the pty-writer thread — and viewer
/// input with it — for the rest of the session. A truly dead proxy ends the
/// session through the reader's EOF instead.
struct TapWriter {
    shared: Arc<TapShared>,
}

impl Write for TapWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let stream = match self.shared.stream.lock() {
            Ok(current) => current.try_clone(),
            Err(_) => return Ok(buf.len()),
        };
        if let Ok(mut stream) = stream {
            // Chunked so no input, however large a viewer's paste, can
            // exceed the protocol's frame cap (which would panic).
            for chunk in buf.chunks(protocol::MAX_FRAME_LEN) {
                if stream.write_all(&protocol::input_frame(chunk)).is_err() {
                    break;
                }
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::os::unix::net::UnixListener;

    use super::*;

    fn test_snapshot() -> Snapshot {
        Snapshot {
            rows: 24,
            cols: 80,
            cursor_row: 1,
            cursor_col: 2,
            rows_data: vec![b"hi".to_vec()],
        }
    }

    /// A fake proxy on the far end of a socketpair: consume the greeting and
    /// Subscribe, assert what the client asked for, and answer.
    fn expect_subscribe(server: &mut UnixStream, want_input: bool, token: &[u8]) {
        let mut magic = [0u8; 4];
        server.read_exact(&mut magic).expect("greeting");
        assert_eq!(magic, MAGIC);
        let (frame_type, payload) = protocol::read_frame(server)
            .expect("subscribe frame")
            .expect("not eof");
        assert_eq!(frame_type, protocol::FRAME_SUBSCRIBE);
        let (got_want, got_token) = protocol::decode_subscribe(&payload).expect("valid subscribe");
        assert_eq!(got_want, want_input);
        assert_eq!(got_token, token);
    }

    /// A reader over `client` whose resync is already refused (`stopped`), so
    /// tests drive the decode loop without a live proxy.
    fn stopped_reader(client: UnixStream) -> (TapReader, std_mpsc::Receiver<()>) {
        let (eof_tx, eof_rx) = std_mpsc::channel::<()>();
        let reader = TapReader {
            shared: Arc::new(TapShared {
                stream: Mutex::new(client.try_clone().expect("clone")),
                // Already stopped: an End must detach, not reconnect.
                stopped: AtomicBool::new(true),
            }),
            stream: client,
            pending: Vec::new(),
            eof_tx: Some(eof_tx),
            socket_path: PathBuf::from("/nonexistent"),
            want_input: false,
            token: Vec::new(),
        };
        (reader, eof_rx)
    }

    #[test]
    fn handshake_subscribes_and_yields_the_first_keyframe() {
        let (mut client, mut server) = UnixStream::pair().expect("socketpair");
        let snapshot = test_snapshot();
        let served = snapshot.clone();
        let fake_proxy = std::thread::spawn(move || {
            expect_subscribe(&mut server, true, b"tok");
            server
                .write_all(&protocol::hello_frame(true))
                .expect("hello");
            server
                .write_all(&protocol::encode_frame(FRAME_KEYFRAME, &served.encode()))
                .expect("keyframe");
            server // keep the far end open until the client is done
        });
        let got = handshake(&mut client, true, b"tok").expect("handshake");
        assert_eq!(got, snapshot);
        drop(fake_proxy.join().expect("fake proxy"));
    }

    /// An old proxy ignores what we write, serves its one-shot snapshot
    /// blob, and closes — the blob's first bytes parse as an absurd frame
    /// header. A proxy that closes without writing anything reads the same.
    #[test]
    fn handshake_rejects_a_pre_subscriber_proxy() {
        let (mut client, mut server) = UnixStream::pair().expect("socketpair");
        let fake_proxy = std::thread::spawn(move || {
            let _ = server.write_all(&test_snapshot().encode());
            // close without ever reading
        });
        assert!(matches!(
            handshake(&mut client, false, b""),
            Err(Error::ProxyTooOld)
        ));
        fake_proxy.join().expect("fake proxy");

        let (mut client, server) = UnixStream::pair().expect("socketpair");
        drop(server); // immediate EOF
        assert!(matches!(
            handshake(&mut client, false, b""),
            Err(Error::ProxyTooOld)
        ));
    }

    /// A proxy that accepts and then goes silent is *wedged*, not old: the
    /// attach must say so — and tell the user to retry — rather than send
    /// them off to destroy their shell session. Bounded by
    /// [`HANDSHAKE_TIMEOUT`], so a wedged proxy fails the attach fast.
    #[test]
    fn handshake_reports_a_silent_proxy_as_unresponsive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sock");
        let listener = UnixListener::bind(&path).expect("bind");
        let fake_proxy = std::thread::spawn(move || {
            let (conn, _) = listener.accept().expect("accept");
            // Accepted, never read, never answered — and held open, so the
            // client sees silence rather than EOF.
            std::thread::sleep(HANDSHAKE_TIMEOUT + Duration::from_secs(1));
            drop(conn);
        });

        let mut client = UnixStream::connect(&path).expect("connect");
        let start = std::time::Instant::now();
        let result = handshake(&mut client, false, b"");
        assert!(
            matches!(result, Err(Error::ProxyUnresponsive)),
            "a silent proxy must not be reported as too old: {result:?}"
        );
        assert!(
            start.elapsed() < HANDSHAKE_TIMEOUT * 3,
            "the attach must fail within the handshake timeout, took {:?}",
            start.elapsed()
        );
        fake_proxy.join().expect("fake proxy");
    }

    /// The same split after the Hello: a proxy that greets and then stalls
    /// mid-handshake is unresponsive, not a source of "invalid reply".
    #[test]
    fn handshake_reports_a_stall_after_hello_as_unresponsive() {
        let (mut client, mut server) = UnixStream::pair().expect("socketpair");
        let fake_proxy = std::thread::spawn(move || {
            expect_subscribe(&mut server, false, b"");
            server
                .write_all(&protocol::hello_frame(false))
                .expect("hello");
            // ...then nothing, with the connection held open.
            std::thread::sleep(HANDSHAKE_TIMEOUT + Duration::from_secs(1));
        });
        let result = handshake(&mut client, false, b"");
        assert!(
            matches!(result, Err(Error::ProxyUnresponsive)),
            "a stall after Hello must not read as a malformed reply: {result:?}"
        );
        fake_proxy.join().expect("fake proxy");
    }

    /// `--write` without the grant must fail the attach, not silently
    /// downgrade to read-only.
    #[test]
    fn handshake_errors_when_write_is_not_granted() {
        let (mut client, mut server) = UnixStream::pair().expect("socketpair");
        let fake_proxy = std::thread::spawn(move || {
            expect_subscribe(&mut server, true, b"wrong");
            let _ = server.write_all(&protocol::hello_frame(false));
        });
        assert!(matches!(
            handshake(&mut client, true, b"wrong"),
            Err(Error::ProxyInputDenied)
        ));
        fake_proxy.join().expect("fake proxy");
    }

    /// The steady-state decode loop: Output frames surface as `Output`
    /// events, Resize frames as `Resize` events **in wire order between the
    /// surrounding output** (the whole point of the single ordered stream),
    /// unknown frames are skipped, and End — with resync refused because the
    /// tap was stopped — is the end of the stream plus the wait channel
    /// waking.
    #[test]
    fn tap_reader_decodes_output_resize_and_end_in_wire_order() {
        let (client, mut server) = UnixStream::pair().expect("socketpair");
        let (mut reader, eof_rx) = stopped_reader(client);

        server
            .write_all(&protocol::encode_frame(FRAME_OUTPUT, b"hello"))
            .expect("output");
        server
            .write_all(&protocol::resize_frame(30, 100))
            .expect("resize");
        server
            .write_all(&protocol::encode_frame(0x7f, b"future"))
            .expect("unknown");
        server
            .write_all(&protocol::encode_frame(FRAME_OUTPUT, b"world"))
            .expect("output");
        server.write_all(&protocol::end_frame()).expect("end");
        drop(server);

        match reader.read_event().expect("first event") {
            Some(ReadEvent::Output(bytes)) => assert_eq!(bytes, b"hello"),
            _ => panic!("expected the first output chunk"),
        }
        // The resize arrives exactly between the chunks it separated on the
        // wire — never before "hello" or after "world".
        match reader.read_event().expect("second event") {
            Some(ReadEvent::Resize(size)) => assert_eq!(
                size,
                Size {
                    cols: 100,
                    rows: 30
                }
            ),
            _ => panic!("expected the resize between the two output chunks"),
        }
        match reader.read_event().expect("third event") {
            Some(ReadEvent::Output(bytes)) => assert_eq!(bytes, b"world"),
            _ => panic!("expected the second output chunk"),
        }
        assert!(reader.read_event().expect("end").is_none());
        assert!(eof_rx.recv().is_err(), "the end must wake the wait channel");
    }

    /// The attach keyframe is the one geometry that arrives before anything
    /// is minted, so it **refuses** where every later one clamps — the same
    /// rule the subshell path applies in `host_size_from`. A silent clamp
    /// there would have handed viewers a screen a row taller than the host's
    /// real terminal for the whole session.
    #[test]
    fn a_degenerate_attach_geometry_is_refused_not_floored() {
        // Reachable values: the proxy's own clamp is `(1, 1)`.
        for (cols, rows) in [(0, 0), (1, 1), (80, 1), (0, 24), (80, 0)] {
            let snapshot = Snapshot {
                cols,
                rows,
                ..test_snapshot()
            };
            let Err(Error::ProxyTerminalTooSmall { cols: c, rows: r }) = attach_size(&snapshot)
            else {
                panic!("{cols}x{rows} must be refused at attach, not floored");
            };
            // The refusal quotes back what the proxy reported, unclamped.
            assert_eq!((c, r), (cols, rows));
        }

        // A survivable geometry passes through untouched — no bar row is
        // reserved, because the tap feeds a headless session.
        assert_eq!(
            attach_size(&test_snapshot()).expect("80x24 is survivable"),
            Size { cols: 80, rows: 24 }
        );
        // Exactly on the floor is fine; one row under is not.
        assert!(
            attach_size(&Snapshot {
                cols: crate::MIN_COLS,
                rows: crate::MIN_CHILD_ROWS,
                ..test_snapshot()
            })
            .is_ok()
        );
        assert!(
            attach_size(&Snapshot {
                cols: crate::MIN_COLS,
                rows: crate::MIN_CHILD_ROWS - 1,
                ..test_snapshot()
            })
            .is_err()
        );
    }

    /// The refusal copy is user-facing: pure ASCII, quotes the size, names the
    /// minimum, and — unlike the subshell path's — never mentions a warning
    /// bar row, because a tapped session draws no bar.
    #[test]
    fn proxy_terminal_too_small_display() {
        let copy = Error::ProxyTerminalTooSmall { cols: 80, rows: 1 }.to_string();
        assert!(copy.is_ascii());
        assert!(copy.contains("80x1"));
        assert!(copy.contains("--active"));
        assert!(!copy.contains("warning bar"));
    }

    /// The pty-proxy's own clamp is `(1, 1)` — pinned by its
    /// `zero_dimension_screens_do_not_panic` and
    /// `resize_is_clamped_and_fanned_out` tests — but 1x1 is one of the sizes
    /// that panics the session's vt100 model. So the tap must absorb it on
    /// every geometry path that arrives **after** the link is out in the
    /// world, where refusing would kill a live session.
    #[test]
    fn a_degenerate_proxy_geometry_is_floored_not_honoured() {
        let floor = Size {
            cols: crate::MIN_COLS,
            rows: crate::MIN_CHILD_ROWS,
        };

        // Path 1: a keyframe header seen mid-stream — the resync repaint and
        // the out-of-band `FRAME_KEYFRAME`. (The *attach* keyframe refuses
        // instead; see the test above.)
        for (cols, rows) in [(0, 0), (1, 1), (80, 1), (0, 24)] {
            let snapshot = Snapshot {
                cols,
                rows,
                ..test_snapshot()
            };
            let got = snapshot_size(&snapshot);
            assert!(
                got.cols >= crate::MIN_COLS && got.rows >= crate::MIN_CHILD_ROWS,
                "{cols}x{rows} floored to {got:?}"
            );
        }
        assert_eq!(
            snapshot_size(&Snapshot {
                cols: 1,
                rows: 1,
                ..test_snapshot()
            }),
            floor
        );
        // A survivable geometry passes through untouched, bar row and all
        // (the tap feeds a headless session, which draws no bar).
        assert_eq!(snapshot_size(&test_snapshot()), Size { cols: 80, rows: 24 });

        // Path 2: a mid-session Resize frame.
        let (client, mut server) = UnixStream::pair().expect("socketpair");
        let (mut reader, _eof_rx) = stopped_reader(client);
        server
            .write_all(&protocol::resize_frame(1, 1))
            .expect("resize");
        server
            .write_all(&protocol::resize_frame(0, 0))
            .expect("resize");
        drop(server);
        match reader.read_event().expect("first resize") {
            Some(ReadEvent::Resize(size)) => assert_eq!(size, floor, "1x1 must be floored"),
            _ => panic!("expected the 1x1 resize event"),
        }
        match reader.read_event().expect("second resize") {
            Some(ReadEvent::Resize(size)) => assert_eq!(size, floor, "0x0 must be floored"),
            _ => panic!("expected the 0x0 resize event"),
        }
    }

    /// A large payload arrives intact as one event — the owned-event seam
    /// has no caller buffer to split across, and no byte may be lost.
    #[test]
    fn tap_reader_delivers_large_payloads_intact() {
        let (client, mut server) = UnixStream::pair().expect("socketpair");
        let (mut reader, _eof_rx) = stopped_reader(client);
        let payload: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        server
            .write_all(&protocol::encode_frame(FRAME_OUTPUT, &payload))
            .expect("output");

        match reader.read_event().expect("event") {
            Some(ReadEvent::Output(bytes)) => assert_eq!(bytes, payload),
            _ => panic!("expected the payload as one output event"),
        }
    }

    /// Losing the feed reconnects to the proxy socket, re-subscribes, and
    /// yields the fresh geometry first, then the keyframe as one clean
    /// repaint — resize before repaint, so the model re-wraps at the right
    /// width before processing the repaint bytes.
    #[test]
    fn tap_reader_resyncs_after_losing_the_feed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sock");
        let listener = UnixListener::bind(&path).expect("bind");
        let fake_proxy = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("accept");
            expect_subscribe(&mut conn, false, b"");
            conn.write_all(&protocol::hello_frame(false))
                .expect("hello");
            conn.write_all(&protocol::encode_frame(
                FRAME_KEYFRAME,
                &test_snapshot().encode(),
            ))
            .expect("keyframe");
            conn // keep the reconnected feed open until the client is done
        });

        let (client, server) = UnixStream::pair().expect("socketpair");
        let (eof_tx, eof_rx) = std_mpsc::channel::<()>();
        let shared = Arc::new(TapShared {
            stream: Mutex::new(client.try_clone().expect("clone")),
            stopped: AtomicBool::new(false),
        });
        let mut reader = TapReader {
            shared: Arc::clone(&shared),
            stream: client,
            pending: Vec::new(),
            eof_tx: Some(eof_tx),
            socket_path: path,
            want_input: false,
            token: Vec::new(),
        };
        drop(server); // the feed dies

        match reader.read_event().expect("geometry after resync") {
            Some(ReadEvent::Resize(size)) => {
                assert_eq!(size, Size { cols: 80, rows: 24 });
            }
            _ => panic!("resync must re-announce geometry before the repaint"),
        }
        match reader.read_event().expect("repaint after resync") {
            Some(ReadEvent::Output(bytes)) => {
                assert_eq!(bytes, protocol::snapshot_to_ansi(&test_snapshot()));
            }
            _ => panic!("the repaint must follow the resize"),
        }

        // A deliberate stop ends the next resync immediately: end + wake.
        let conn = fake_proxy.join().expect("fake proxy");
        shared.stopped.store(true, Ordering::SeqCst);
        drop(conn);
        assert!(reader.read_event().expect("end").is_none());
        assert!(eof_rx.recv().is_err(), "the end must wake the wait channel");
    }

    /// Input is framed onto the current stream, never written raw.
    #[test]
    fn tap_writer_frames_input() {
        let (client, mut server) = UnixStream::pair().expect("socketpair");
        let mut writer = TapWriter {
            shared: Arc::new(TapShared {
                stream: Mutex::new(client),
                stopped: AtomicBool::new(false),
            }),
        };
        writer.write_all(b"keys").expect("write");
        let (frame_type, payload) = protocol::read_frame(&mut server)
            .expect("frame")
            .expect("not eof");
        assert_eq!(frame_type, protocol::FRAME_INPUT);
        assert_eq!(payload, b"keys");
    }
}
