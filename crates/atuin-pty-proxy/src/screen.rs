use std::io::Write;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::compositor::{Compositor, lock_unpoisoned};

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

/// Serve screen snapshots to local clients (e.g. `atuin search`, which uses
/// them to restore the screen area its popup covered).
pub(crate) fn spawn_socket_server<W: Write + Send + 'static>(
    sock_path: PathBuf,
    compositor: Arc<Mutex<Compositor<W>>>,
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
        };

        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };

            let data = encode_screen(lock_unpoisoned(&compositor).screen());
            let _ = stream.write_all(&data);
            let _ = stream.flush();
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
fn encode_screen(screen: &vt100::Screen) -> Vec<u8> {
    let (rows, cols) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();

    let mut buf: Vec<u8> = Vec::with_capacity(256 + (rows as usize * cols as usize));
    buf.extend_from_slice(&rows.to_be_bytes());
    buf.extend_from_slice(&cols.to_be_bytes());
    buf.extend_from_slice(&cursor_row.to_be_bytes());
    buf.extend_from_slice(&cursor_col.to_be_bytes());

    for row_bytes in screen.rows_formatted(0, cols) {
        let len = row_bytes.len() as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&row_bytes);
    }

    buf
}

#[cfg(test)]
mod tests {
    use super::encode_screen;

    #[test]
    fn encode_screen_wire_format_is_stable() {
        let mut parser = vt100::Parser::new(3, 20, 0);
        parser.process(b"hello");
        let data = encode_screen(parser.screen());

        assert_eq!(u16::from_be_bytes([data[0], data[1]]), 3);
        assert_eq!(u16::from_be_bytes([data[2], data[3]]), 20);
        assert_eq!(u16::from_be_bytes([data[4], data[5]]), 0);
        assert_eq!(u16::from_be_bytes([data[6], data[7]]), 5);

        // Three length-prefixed rows follow.
        let mut offset = 8;
        let mut rows = 0;
        while offset + 4 <= data.len() {
            let len = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4 + len;
            rows += 1;
        }
        assert_eq!(rows, 3);
        assert_eq!(offset, data.len());
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
