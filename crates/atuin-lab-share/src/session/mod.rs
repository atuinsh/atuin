//! The share session: one central `select!` task that owns all session state,
//! plus the bridged threads that cover the blocking edges tokio cannot — the
//! PTY read and write always, and with a host terminal ([`HostUi`]) the
//! raw-mode stdin read and the terminal write too. Headless mode
//! (`host: None`) runs the same loop with the host-facing pieces absent — not
//! idle: a session with null stdio would never see a parked thread on it
//! exit. No shipping caller runs headless today; the mode is what the
//! session's own tests drive it through.

mod screen;

use std::io::{Read, Write};

use serde_json::Value;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use self::screen::{FRAME_INTERVAL, KEYFRAME_TICK, ScreenState};
use crate::Size;
use crate::backpressure::Frame;
use crate::protocol::b64_decode;
use crate::render::{Compositor, StatusBar, WriteMode};
use crate::source::{ReadEvent, SourceParts, SourceReader};

/// The host-side kill switch: Ctrl-\. Raw mode disables `ISIG`, so it arrives
/// as a plain byte rather than raising `SIGQUIT`.
const KILL_SWITCH_BYTE: u8 = 0x1c;
/// Read-buffer size for host stdin (keystrokes arrive in small bursts).
const STDIN_READ_BUF: usize = 1024;
/// Capacity of the pty-reader → session channel: ~8 KiB per chunk, so a few
/// hundred KiB of buffered output before the reader briefly parks.
const PTY_CHANNEL_CAP: usize = 32;
/// Capacity of the stdin-reader → session channel (keystroke bursts are tiny).
const STDIN_CHANNEL_CAP: usize = 32;
/// Capacity of the session → pty-writer channel. It only fills when the child
/// has stopped reading its own input — see [`SessionTask::forward_to_child`].
const PTY_WRITE_CHANNEL_CAP: usize = 32;

/// Where the join URL goes on every `Connected` in headless mode — see
/// [`Session::url_sink`].
pub(crate) type UrlSink = Box<dyn Fn(&str) + Send>;

/// Session → hub events.
pub(crate) enum Outbound {
    /// One chunk of raw child output, stamped with its `seq`.
    Output(Frame),
    /// A full-screen repaint, stamped with its `seq` (see the seq invariant on
    /// [`ScreenState`]).
    Keyframe(Frame),
    /// The host's terminal geometry (already minus the bar row), for the hub
    /// to negotiate viewer sizes against.
    HostSize {
        cols: u16,
        rows: u16,
    },
    /// Kill switch / clean exit: end the session and invalidate the link now.
    End,
}

/// Hub → session events.
pub(crate) enum Inbound {
    /// Viewer keystrokes. As minted by [`Inbound::from_event`] this still
    /// holds the sealed wire blob (`nonce || ciphertext || tag`); the
    /// transport authenticates, replay-checks, and decrypts it before
    /// forwarding, so by the time the session receives this variant the bytes
    /// are plaintext. Only forwarded to the child in write mode — enforced
    /// here as well as on the hub (defence in depth, spec §8), gating the
    /// already-decrypted bytes.
    Input(Vec<u8>),
    /// The hub's negotiated child size (smallest connected viewer).
    SetSize {
        cols: u16,
        rows: u16,
    },
    /// The current viewer count, for the warning bar.
    Participants(u32),
    /// The hub dropped its replay buffer; send a fresh keyframe.
    RequestKeyframe,
    /// Viewer input is disabled for the rest of this process: the transport's
    /// never-forget replay ledger is full, so every further viewer keystroke
    /// is refused (`transport::INPUT_NONCE_CAP`). Sent exactly once, by the
    /// transport, and **never minted from a hub event** — the hub has no say
    /// in it and [`Inbound::from_event`] cannot produce it.
    ///
    /// The session owns the host-facing half of the notice because it owns the
    /// bar: an `eprintln!` alone is erased by the next composite, and the
    /// condition is permanent (see [`StatusBar::input_disabled`]).
    InputDisabled,
    /// Joined (or re-joined) successfully. `fresh_session` is true when the hub
    /// issued a *different* public token than we last advertised — i.e. our
    /// resume was rejected or expired and the hub created a brand-new session.
    /// The old link is dead in that case and the new one must be re-printed.
    ///
    /// The public token itself stays in the transport (it feeds the
    /// `fresh_session` comparison there); the session only ever prints
    /// `join_url`, which already embeds it.
    Connected {
        join_url: String,
        fresh_session: bool,
    },
    Disconnected,
}

impl Inbound {
    /// Map a hub → CLI channel event onto an `Inbound`. Unknown events yield
    /// `None` and are ignored, so the hub can add events without breaking older
    /// clients.
    ///
    /// Deliberately keyless: for `input` the b64 decode yields the sealed
    /// E2EE blob, which the transport opens before dispatching to the session.
    pub(crate) fn from_event(event: &str, payload: &Value) -> Option<Self> {
        match event {
            "input" => {
                b64_decode(payload["data"].as_str().unwrap_or_default()).ok().map(Self::Input)
            }
            "set_size" => Some(Self::SetSize {
                cols: u16::try_from(payload["cols"].as_u64()?).ok()?,
                rows: u16::try_from(payload["rows"].as_u64()?).ok()?,
            }),
            "participants" => {
                Some(Self::Participants(u32::try_from(payload["count"].as_u64()?).ok()?))
            }
            "request_keyframe" => Some(Self::RequestKeyframe),
            _ => None,
        }
    }
}

/// Where a child resize came from — which decides what the host is told after
/// the common apply steps.
#[derive(Clone, Copy)]
enum ResizeSource {
    /// SIGWINCH: the host's own window changed; the hub gets the new maximum
    /// (`host_size`) to renegotiate against.
    HostWindow,
    /// The hub's `set_size` negotiation; the host gets an explanatory notice
    /// when something actually changed (spec §6).
    Hub,
}

/// How the central `select!` loop ended.
enum LoopExit {
    /// The child exited on its own; the wait arm collected its code.
    Exited(i32),
    /// The host pressed Ctrl-\; the child was just killed and must be reaped.
    KillSwitch,
    /// A stop signal (SIGTERM from `--stop`, SIGHUP from a dying terminal)
    /// asked a headless session to end; the source was just stopped —
    /// detached, never killed — and must be waited out like the kill switch.
    Stopped,
}

/// The state the central `select!` task owns outright — no locks, no atomics:
/// every arm of the loop runs on this one task, so each field has exactly one
/// writer by construction.
struct SessionTask {
    /// The child's screen model, its `seq` counter, and the keyframe cadence.
    /// Single-owner: frames are minted *and sent* from this task only, so they
    /// enter the channel in seq order structurally — see [`ScreenState`].
    screen: ScreenState,
    /// Queues input for the pty-writer thread, which owns the sole PTY writer.
    /// The three paths that feed the child — host stdin, inbound viewer
    /// `input`, and query replies — are all arms of the same loop, so bytes
    /// enter the channel in one deterministic order by construction.
    child_tx: mpsc::Sender<Vec<u8>>,
    /// Applies negotiated sizes to the source — for the subshell this holds
    /// the boxed PTY master (though not the only master fd); a source that
    /// owns no terminal supplies a no-op. See [`SourceParts::resizer`].
    resizer: Box<dyn Fn(Size) + Send>,
    out_tx: UnboundedSender<Outbound>,
    write: WriteMode,
    /// Whether CPR/DA probes are answered synthetically — see
    /// [`SourceParts::answer_queries`].
    answer_queries: bool,
    /// Whether the hub's `set_size` negotiation is applied to the source —
    /// see [`SourceParts::follows_hub_resize`].
    follows_hub_resize: bool,
    /// The rows available to the child: the host's real terminal height
    /// **minus the bar row**, subtracted exactly once at the source
    /// (`run_share` at startup, [`Self::handle_winch`] on SIGWINCH). Never the
    /// real terminal height — see [`Compositor`].
    physical: Size,
    /// Negotiated child PTY size (what `set_size` last asked for, clamped).
    child: Size,
    /// Current viewer count, for the bar.
    viewers: u32,
    /// Whether viewer input has been permanently disabled, for the bar. Set
    /// once, by [`Inbound::InputDisabled`], and never cleared — the condition
    /// it reports lasts as long as the process does. The *enforcement* lives
    /// in the transport (which stops delivering `Input` at all); this field is
    /// only how the host gets to see it.
    input_disabled: bool,
    /// Screen needs repainting.
    dirty: bool,
    /// Hands composed frames to the terminal-writer thread. Exists exactly
    /// when a host terminal does; `None` in headless mode, where there is
    /// nothing to compose onto and every host-facing side effect (frames,
    /// resize notices) is gated on it.
    frame_tx: Option<mpsc::Sender<Vec<u8>>>,
    /// Where the join URL goes on every `Connected` — see
    /// [`Session::url_sink`].
    url_sink: Option<UrlSink>,
}

impl SessionTask {
    /// One chunk of child output: run it through the model, send the frames it
    /// minted, answer its device queries, and schedule a repaint. A closed
    /// transport channel is not fatal — the shell keeps running locally.
    fn handle_pty_chunk(&mut self, chunk: &[u8]) {
        let outcome = self.screen.process_chunk(chunk);
        let _ = self.out_tx.send(Outbound::Output(outcome.output));
        if let Some(keyframe) = outcome.keyframe {
            let _ = self.out_tx.send(Outbound::Keyframe(keyframe));
        }
        // Only when nothing else would answer: a source whose real terminal
        // sees the probe answers for itself, and a synthetic reply on top
        // would double-answer — see [`SourceParts::answer_queries`].
        if self.answer_queries && !outcome.replies.is_empty() {
            self.forward_to_child(&outcome.replies);
        }
        self.dirty = true;
    }

    /// One chunk of host keystrokes. Returns `true` when the kill switch
    /// fired: the bytes before it were forwarded and `Outbound::End` was sent
    /// — the caller kills the child and breaks to teardown.
    fn handle_stdin_chunk(&mut self, chunk: &[u8]) -> bool {
        // Kill switch (see `KILL_SWITCH_BYTE`). Everything before it in the
        // same chunk is still delivered to the child.
        if let Some(pos) = chunk.iter().position(|&b| b == KILL_SWITCH_BYTE) {
            if pos > 0 {
                self.forward_to_child(&chunk[..pos]);
            }
            // Tell the hub explicitly — never rely on socket closure, which
            // would leave the link alive for the hub's 30 s grace period.
            let _ = self.out_tx.send(Outbound::End);
            return true;
        }
        // The host's own keystrokes are always forwarded, regardless of
        // `--write` (that flag governs *viewer* input only).
        self.forward_to_child(chunk);
        false
    }

    fn handle_inbound(&mut self, msg: Inbound) {
        match msg {
            Inbound::Input(bytes) => self.handle_input(&bytes),
            Inbound::SetSize { cols, rows } => {
                // A source whose own terminal is authoritative ignores the
                // hub's ask — see [`SourceParts::follows_hub_resize`].
                if self.follows_hub_resize {
                    self.apply_resize(Size { cols, rows }, ResizeSource::Hub);
                }
            }
            Inbound::Participants(count) => self.handle_participants(count),
            Inbound::RequestKeyframe => self.handle_request_keyframe(),
            Inbound::InputDisabled => self.handle_input_disabled(),
            Inbound::Connected {
                join_url,
                fresh_session,
            } => self.handle_connected(&join_url, fresh_session),
            Inbound::Disconnected => self.handle_disconnected(),
        }
    }

    /// Viewer keystrokes from the hub.
    fn handle_input(&mut self, bytes: &[u8]) {
        // Defence in depth (spec §8): a read-only session must never execute
        // viewer keystrokes, even if the hub sends them. The hub enforces this
        // too; we do not depend on it.
        if !self.write.is_write_enabled() {
            return;
        }
        self.forward_to_child(bytes);
    }

    fn handle_participants(&mut self, count: u32) {
        self.viewers = count;
        self.dirty = true;
    }

    /// Viewer input just died for the rest of the process (see
    /// [`Inbound::InputDisabled`]). Two host-facing effects, deliberately
    /// both:
    ///
    /// * the bar flips to `INPUT DISABLED` and **stays** there — the only
    ///   surface a repaint cannot erase, and the only one a host who was away
    ///   from the keyboard will still find; and
    /// * one line explaining *why*, which the bar has no room for. That line
    ///   is expendable (a repaint composites over it); the bar is not.
    ///
    /// The session does not end and nothing else changes: output keeps
    /// flowing, viewers keep watching, the host's own keystrokes are
    /// unaffected. Idempotent — the transport sends this once, and a second
    /// one would only repaint.
    fn handle_input_disabled(&mut self) {
        if self.input_disabled {
            return;
        }
        self.input_disabled = true;
        self.dirty = true;
        eprintln!(
            "\r\n[atuin lab share] viewer input disabled: the replay-protection budget is spent. \
             Output and viewing continue; restart the share to re-enable typing.\r"
        );
    }

    /// The hub lost its replay buffer; answer with a fresh keyframe right away
    /// — the child may never write again on its own. Minting and sending from
    /// this one task keeps the seq invariant intact by construction.
    fn handle_request_keyframe(&mut self) {
        let keyframe = self.screen.emit_keyframe();
        let _ = self.out_tx.send(Outbound::Keyframe(keyframe));
    }

    fn handle_connected(&mut self, join_url: &str, fresh_session: bool) {
        // A rejected/expired resume makes the hub mint a NEW session with a
        // NEW public token, silently. If we kept advertising the old URL,
        // viewers could never rejoin — so re-surface it whenever the token
        // changed. With a host terminal that means re-printing prominently;
        // a headless session hands EVERY Connected URL to its sink, whose
        // owner keeps the persisted copy current — the `fresh_session`
        // rewrite is exactly the moment a stored URL goes stale.
        match &self.url_sink {
            Some(sink) => sink(join_url),
            None if fresh_session => println!(
                "\r\n  Reconnected as a NEW session -- the previous link is dead.\r\n  New link: \
                 {join_url}\r"
            ),
            None => println!("\r\n  Share this link: {join_url}\r"),
        }
        let keyframe = self.screen.emit_keyframe();
        let _ = self.out_tx.send(Outbound::Keyframe(keyframe));
        self.dirty = true;
    }

    fn handle_disconnected(&mut self) {
        // Keep running: the subshell must survive transport blips.
        self.dirty = true;
    }

    /// SIGWINCH: the host's own window changed. Re-clamp the current child
    /// size against the new physical maximum and tell the hub about it.
    fn handle_winch(&mut self) {
        let Ok((cols, rows)) = crossterm::terminal::size() else {
            return;
        };
        self.apply_host_window(cols, rows);
    }

    /// The pure half of [`Self::handle_winch`], split out so the clamp is
    /// testable without a real tty behind `crossterm::terminal::size()`.
    ///
    /// [`crate::clamp_host_size`] reserves the bar row — the second of the two
    /// documented at-the-source subtractions (`run_share` does the other at
    /// startup) — and floors the result at what `vt100` survives. It
    /// **clamps** where startup refuses: a host who drags their window down to
    /// two rows must get an unreadable session, never a dead one, and by the
    /// time SIGWINCH fires the link is already out in the world.
    fn apply_host_window(&mut self, cols: u16, rows: u16) {
        self.physical = crate::clamp_host_size(cols, rows);
        self.apply_resize(self.child, ResizeSource::HostWindow);
    }

    /// A source-originated resize ([`ReadEvent::Resize`], applied at its
    /// exact position in the reader stream): the source's own terminal
    /// changed, its geometry is authoritative, and it carries no bar row to
    /// reserve. Routed through the same apply path as SIGWINCH, so the model
    /// repaint and the `Outbound::HostSize` re-advertisement stay in one
    /// place.
    fn handle_source_resize(&mut self, size: Size) {
        self.physical = size;
        self.apply_resize(size, ResizeSource::HostWindow);
    }

    /// Apply a child resize from either source: clamp, resize the PTY, resize
    /// the screen model (emitting the repaint keyframe), then tell whoever the
    /// source demands.
    ///
    /// The clamp is applied **locally and immediately**, without waiting for
    /// the hub: if the host shrinks their window, a child still sized to the
    /// old (taller) geometry would be painted past the last visible row,
    /// scrolling the status bar off the screen. It also keeps resize working
    /// while the transport is down.
    fn apply_resize(&mut self, want: Size, source: ResizeSource) {
        let clamped = self.clamp_child(want);
        let previous = self.child;
        self.child = clamped;
        (self.resizer)(clamped);
        // The keyframe is minted in the same call as the model resize — an
        // idle child would otherwise leave every viewer resized but never
        // repainted (spec §5.3). See [`ScreenState::set_size`].
        let keyframe = self.screen.set_size(clamped);
        let _ = self.out_tx.send(Outbound::Keyframe(keyframe));
        self.dirty = true;

        match source {
            ResizeSource::HostWindow => {
                // Let the hub renegotiate against the new maximum. A closed
                // transport is fine — the local resize above already took
                // effect.
                let _ = self.out_tx.send(Outbound::HostSize {
                    cols: self.physical.cols,
                    rows: self.physical.rows,
                });
            }
            ResizeSource::Hub => {
                // Explain the resize to the host (spec §6) — but only when
                // something actually changed, and only when there is a host
                // terminal to explain it to (`frame_tx` exists exactly when
                // one does).
                if self.frame_tx.is_some()
                    && let Some(notice) = resize_notice(previous, clamped, self.viewers)
                {
                    eprintln!("\r\n[atuin lab share] {notice}\r");
                }
            }
        }
    }

    /// Clamp a requested child size to what physically fits below the bar,
    /// then to what `vt100` survives.
    ///
    /// The floors run **after** the `min`, so they win outright: a hub that
    /// negotiates 80x1, or a physical window already below the floor, still
    /// yields a grid the screen model can be built on. See [`crate::MIN_COLS`]
    /// and [`crate::MIN_CHILD_ROWS`] for why the row floor is 2 and not 1 —
    /// 1x1 is itself a panicking size, so `.max(1)` here was no protection at
    /// all.
    fn clamp_child(&self, want: Size) -> Size {
        Size {
            cols: want.cols.min(self.physical.cols).max(crate::MIN_COLS),
            rows: want.rows.min(self.physical.rows).max(crate::MIN_CHILD_ROWS),
        }
    }

    /// Service the keyframe cadence while the child is silent: no PTY chunks
    /// arrive then, so a pending request (initial keyframe, overflow resync)
    /// or the periodic cadence would otherwise sit unserviced until the child
    /// next writes — which may be never (spec §5.3).
    fn service_keyframes(&mut self) {
        if !self.screen.keyframe_due() {
            return;
        }
        let keyframe = self.screen.emit_keyframe();
        let _ = self.out_tx.send(Outbound::Keyframe(keyframe));
    }

    /// Compose a frame and hand it to the terminal-writer thread, if anything
    /// changed. `try_send` on a full channel leaves `dirty` set, so the next
    /// tick recomposes with the newest state — frames coalesce rather than
    /// queue behind a slow terminal.
    fn render_if_dirty(&mut self) {
        // Headless: no host terminal, nothing to compose (the render tick
        // that drives this is also absent from the select loop then).
        let Some(frame_tx) = &self.frame_tx else {
            return;
        };
        if !self.dirty {
            return;
        }
        self.dirty = false;
        // Rows available to the child: the bar row was already subtracted
        // once, at the source. The compositor clamps against this and adds the
        // bar row back itself, so it must not be subtracted again here.
        let avail = self.physical;
        let bar = StatusBar {
            viewers: self.viewers,
            write: self.write,
            input_disabled: self.input_disabled,
        }
        .render(avail.cols);
        let frame = Compositor { avail }.composite(self.screen.screen(), self.child, &bar);
        if frame_tx.try_send(frame).is_err() {
            // Writer busy (or gone): stay dirty and recompose next tick.
            self.dirty = true;
        }
    }

    /// Queue bytes for the child, best-effort. The write itself happens on
    /// the pty-writer thread: a PTY master write blocks once the kernel's tty
    /// input queue fills (a child that stopped reading stdin — SIGSTOPped, a
    /// wedged TUI, a viewer pasting a blob in `--write` mode), and a blocking
    /// write here would freeze every arm of the select loop — rendering,
    /// resize, child-exit detection, and the Ctrl-\ kill switch with it. On a
    /// full channel the chunk is dropped: the child already has a full kernel
    /// queue plus a full channel of unread input, and parking the loop to
    /// preserve more would trade the whole session's liveness for it. A dead
    /// PTY surfaces as child exit via the wait arm.
    fn forward_to_child(&mut self, bytes: &[u8]) {
        let _ = self.child_tx.try_send(bytes.to_vec());
    }
}

/// The "pty-reader" bridge: `portable-pty`'s reader is a blocking `Read` with
/// no async form, and `#![deny(unsafe_code)]` rules out fd surgery — but more
/// importantly the reads must *continue* after the session stops listening
/// (see the drain comment below), which only a plain thread independent of
/// runtime state can honour.
///
/// Its EOF comes from the **slave** side: a master read returns only once the
/// child *and every descendant that inherited the tty* have exited (the
/// reader holds its own dup of the master fd, so the session dropping its
/// master handles at teardown does not produce one). A background process the
/// shell left holding the slave — `nohup`, `disown`, a daemonizing dev server
/// — can postpone that EOF indefinitely, so like the stdin-reader this thread
/// is deliberately **detached**: it is never joined, discards chunks once the
/// session drops the receiver (drain-only mode, below), and dies with the
/// process at the latest.
fn pty_reader_loop(mut reader: Box<dyn SourceReader>, tx: mpsc::Sender<ReadEvent>) {
    loop {
        let event = match reader.read_event() {
            Ok(Some(event)) => event,
            // Child exited, PTY closed, or the tap's feed ended for good.
            Ok(None) | Err(_) => return,
        };
        // **Keep draining the PTY even when the session is gone.** On
        // BSD/macOS a session leader blocks inside `exit()` until its
        // controlling tty's output queue has drained, so a reader that stops
        // reading while the child is still alive wedges the child in the
        // "exiting" state and the `child.wait()` on the blocking pool — which
        // teardown awaits right after `kill()` — never returns. A send error
        // only means the session dropped the receiver at teardown: discard the
        // event (drain-only mode) and read on until the end.
        let _ = tx.blocking_send(event);
    }
}

/// The "stdin-reader" bridge: a raw-mode blocking `read(2)` on stdin cannot be
/// cancelled (`tokio::io::stdin` would just park a blocking-pool task past
/// shutdown instead of fixing that). The thread is deliberately **detached**:
/// it blocks on its source, so joining it would hang teardown — it is leaked
/// and exits with the process.
fn stdin_reader_loop(mut stdin: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; STDIN_READ_BUF];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if tx.blocking_send(buf[..n].to_vec()).is_err() {
            return; // session gone
        }
    }
}

/// The "pty-writer" bridge: owns the sole PTY writer. A master write blocks
/// once the kernel's tty input queue fills, and must not stall the central
/// task — see [`SessionTask::forward_to_child`]. Like the stdin-reader it is
/// deliberately **detached**: it blocks on its sink, so joining it could hang
/// teardown behind a child that stopped reading input; it exits when the
/// channel closes or the PTY dies, and with the process at the latest.
fn pty_writer_loop(mut writer: Box<dyn Write + Send>, mut rx: mpsc::Receiver<Vec<u8>>) {
    while let Some(bytes) = rx.blocking_recv() {
        if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
            return; // dead PTY; the wait arm reports the child's exit
        }
    }
}

/// The "terminal-writer" bridge: a raw-mode tty write can block indefinitely
/// on flow control (Ctrl-S) and must not stall the session's emit path — the
/// central task hands frames over with `try_send` and moves on. Exits when the
/// frame channel closes at teardown, or when the terminal breaks.
fn terminal_writer_loop(mut stdout: Box<dyn Write + Send>, mut rx: mpsc::Receiver<Vec<u8>>) {
    while let Some(frame) = rx.blocking_recv() {
        if stdout.write_all(&frame).is_err() || stdout.flush().is_err() {
            return;
        }
    }
}

/// Receive the next host-stdin chunk. The receiver only exists in host mode
/// and the select arm is guarded so this is only polled while one does, but a
/// `None` receiver still answers like a closed channel rather than panicking.
/// Cancellation-safe: it wraps nothing but `Receiver::recv`, which is.
async fn recv_host_stdin(stdin_rx: &mut Option<mpsc::Receiver<Vec<u8>>>) -> Option<Vec<u8>> {
    match stdin_rx {
        Some(rx) => rx.recv().await,
        None => None,
    }
}

/// Receive from an optionally-installed signal listener (SIGWINCH exists
/// only in host mode; SIGINT/SIGTERM/SIGHUP only headless). Same guarded-arm
/// shape as [`recv_host_stdin`]. Cancellation-safe: it wraps nothing but
/// `Signal::recv`, which is.
async fn recv_signal(sig: &mut Option<Signal>) -> Option<()> {
    match sig {
        Some(sig) => sig.recv().await,
        None => None,
    }
}

/// The host-facing explanation for a `set_size`, or `None` when there is
/// nothing to report.
///
/// Two things it deliberately avoids saying. A `set_size` that matches the size
/// already applied is announced not at all — the hub's first negotiation
/// normally just echoes the host's own geometry, and reporting that as a resize
/// at startup is simply false. And a genuine resize is only blamed on a viewer
/// when one is actually connected; with nobody watching, the change came from
/// the hub's own negotiation (spec §6).
fn resize_notice(previous: Size, applied: Size, viewers: u32) -> Option<String> {
    if applied == previous {
        return None;
    }
    let dims = format!("{}x{}", applied.cols, applied.rows);
    Some(if viewers == 0 {
        format!("resized to {dims}")
    } else {
        format!("resized to {dims} -- a viewer's screen is smaller")
    })
}

/// The host-facing half of a session: the interactive terminal it composites
/// onto and reads keystrokes from. Absent (`None` on [`Session::host`]) for a
/// headless session.
pub(crate) struct HostUi {
    /// Host keystrokes; handed to the detached stdin-reader thread.
    pub(crate) stdin: Box<dyn Read + Send>,
    /// The host terminal; handed to the terminal-writer thread.
    pub(crate) stdout: Box<dyn Write + Send>,
}

/// One share session, assembled by `run_share` (struct literal) and consumed
/// by [`Self::run`].
///
/// Does **not** touch terminal modes — `run_share` owns raw mode via an RAII
/// guard, which keeps the session unit-testable without touching the test
/// runner's terminal.
pub(crate) struct Session {
    /// The source, already split into the pieces the task topology needs —
    /// see [`SourceParts`] for each field's invariant.
    pub(crate) parts: SourceParts,
    /// The session geometry. Host mode: the host terminal size **minus the
    /// bar row** — subtracted exactly once, by `run_share`; see
    /// [`SessionTask::physical`]. Headless mode: the source's full size —
    /// there is no bar.
    pub(crate) physical: Size,
    /// Whether viewer keystrokes may reach the child.
    pub(crate) write: WriteMode,
    /// Session → transport events.
    pub(crate) out_tx: UnboundedSender<Outbound>,
    /// Transport → session events.
    pub(crate) in_rx: UnboundedReceiver<Inbound>,
    /// The interactive host terminal, or `None` to run headless: no
    /// stdin-reader or terminal-writer thread is spawned, no compositor
    /// frames or render tick, no SIGWINCH listener, no resize notices — the
    /// pieces are absent, not idle (see [`Self::run`]). A headless session
    /// instead listens for SIGINT/SIGTERM/SIGHUP as its stop request.
    pub(crate) host: Option<HostUi>,
    /// Where the join URL goes on every `Connected`: `None` prints it to the
    /// host terminal; `Some` hands it to the caller — a headless session has
    /// no terminal, so its owner persists the URL, rewriting it whenever a
    /// reconnect mints a fresh session and link.
    pub(crate) url_sink: Option<UrlSink>,
}

impl Session {
    /// Run the share session until the source ends or the host presses
    /// Ctrl-\. Returns the source's exit code.
    ///
    /// # Errors
    ///
    /// Returns an error if the SIGWINCH listener cannot be installed or a
    /// bridge thread cannot be spawned.
    pub(crate) async fn run(self) -> crate::Result<i32> {
        let Self {
            parts,
            physical,
            write,
            out_tx,
            mut in_rx,
            host,
            url_sink,
        } = self;
        let SourceParts {
            reader,
            writer,
            resizer,
            mut stop,
            wait,
            bootstrap,
            answer_queries,
            follows_hub_resize,
        } = parts;
        let headless = host.is_none();

        // Registering the listeners needs the caller's runtime — guaranteed,
        // since `run_share` awaits us on it. Host mode watches SIGWINCH (the
        // host's own window drives resizes). A headless session has no window
        // of its own, but it is the thing `--stop` (SIGTERM), the foreground
        // debug run's Ctrl-C (SIGINT — the flag documents "Ctrl-C stops", so
        // it must run the End-emitting teardown, not the default
        // kill-the-process disposition), or a dying terminal (SIGHUP; not
        // expected under setsid, handled identically) signals — all end it
        // gracefully, so teardown's `Outbound::End` still fires. Tap EOF via
        // the wait arm stays the authoritative end signal; the signals are
        // best-effort corroboration.
        let mut winch = if headless {
            None
        } else {
            Some(signal(SignalKind::window_change())?)
        };
        let mut sigint = if headless {
            Some(signal(SignalKind::interrupt())?)
        } else {
            None
        };
        let mut sigterm = if headless {
            Some(signal(SignalKind::terminate())?)
        } else {
            None
        };
        let mut sighup = if headless {
            Some(signal(SignalKind::hangup())?)
        } else {
            None
        };

        // Tell the hub our starting geometry immediately, so its very first
        // negotiation has a host dimension to work with.
        let _ = out_tx.send(Outbound::HostSize {
            cols: physical.cols,
            rows: physical.rows,
        });

        // The bridged threads; why each one exists is on its loop. The
        // PTY pair always exists; the stdin-reader and terminal-writer exist
        // only with a host terminal — headless mode leaves them ABSENT, not
        // idle: there is no tty behind them, and a parked thread on a
        // daemon's null stdio would never exit. Only the terminal writer is
        // ever joined — the others block on sources or sinks that may never
        // yield again, so they are detached.
        let (pty_tx, mut pty_rx) = mpsc::channel::<ReadEvent>(PTY_CHANNEL_CAP);
        let _pty_reader = std::thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || pty_reader_loop(reader, pty_tx))?;
        let (child_tx, child_rx) = mpsc::channel::<Vec<u8>>(PTY_WRITE_CHANNEL_CAP);
        let _pty_writer = std::thread::Builder::new()
            .name("pty-writer".into())
            .spawn(move || pty_writer_loop(writer, child_rx))?;

        let mut stdin_rx = None;
        let mut frame_tx = None;
        let mut term_writer = None;
        if let Some(HostUi { stdin, stdout }) = host {
            let (stdin_tx, rx) = mpsc::channel::<Vec<u8>>(STDIN_CHANNEL_CAP);
            std::thread::Builder::new()
                .name("stdin-reader".into())
                .spawn(move || stdin_reader_loop(stdin, stdin_tx))?;
            stdin_rx = Some(rx);
            let (tx, frame_rx) = mpsc::channel::<Vec<u8>>(1);
            frame_tx = Some(tx);
            term_writer = Some(
                std::thread::Builder::new()
                    .name("terminal-writer".into())
                    .spawn(move || terminal_writer_loop(stdout, frame_rx))?,
            );
        }

        // The source's exit: one bounded blocking call on the blocking pool,
        // replacing the old 20 ms `try_wait` poll. The code mapping lives in
        // the source's `wait` closure — for the subshell it is the one the
        // session has always applied: the child's own code when the wait
        // succeeds (non-`i32` codes clamp to 1), 0 when it fails.
        let mut wait_handle = tokio::task::spawn_blocking(wait);

        let mut task = SessionTask {
            screen: ScreenState::new(physical),
            child_tx,
            resizer,
            out_tx,
            write,
            answer_queries,
            follows_hub_resize,
            physical,
            child: physical,
            viewers: 0,
            input_disabled: false,
            dirty: true,
            frame_tx,
            url_sink,
        };

        // Chunk 0: a source-provided snapshot of what its terminal already
        // showed, fed through the normal output path before the loop starts —
        // the hub sees it as the first `Output` frame and the initial keyframe
        // reflects it. Absent for a subshell, which starts blank.
        if let Some(chunk) = bootstrap {
            task.handle_pty_chunk(&chunk);
        }

        // Both tickers sleep first (like the threads they replace) and skip
        // missed ticks rather than bursting to catch up.
        let mut keyframe_tick = interval_at(Instant::now() + KEYFRAME_TICK, KEYFRAME_TICK);
        keyframe_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut render_tick = interval_at(Instant::now() + FRAME_INTERVAL, FRAME_INTERVAL);
        render_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // A closed channel must stop being polled — `recv` on one returns
        // `None` immediately, which would spin the loop. The `*_open` flags
        // start false for pieces the mode leaves absent (host stdin, the
        // signal listeners), so those arms are skipped outright rather than
        // polled against a `None` receiver.
        let mut pty_open = true;
        let mut stdin_open = stdin_rx.is_some();
        let mut hub_open = true;
        let mut winch_open = winch.is_some();
        let mut sigint_open = sigint.is_some();
        let mut sigterm_open = sigterm.is_some();
        let mut sighup_open = sighup.is_some();
        // The render tick only matters with a host terminal to paint; a
        // headless session must not wake ~60 times a second for a no-op.
        let render_open = task.frame_tx.is_some();

        // Every arm is cancellation-safe: mpsc `recv`, `Interval::tick`,
        // `Signal::recv`, and `&mut JoinHandle`.
        let exit = loop {
            tokio::select! {
                event = pty_rx.recv(), if pty_open => match event {
                    Some(ReadEvent::Output(chunk)) => task.handle_pty_chunk(&chunk),
                    // A source-originated resize, at its exact position in
                    // the output stream — see [`ReadEvent`].
                    Some(ReadEvent::Resize(size)) => task.handle_source_resize(size),
                    // Reader hit EOF; the wait arm below ends the loop.
                    None => pty_open = false,
                },
                chunk = recv_host_stdin(&mut stdin_rx), if stdin_open => match chunk {
                    Some(chunk) => {
                        if task.handle_stdin_chunk(&chunk) {
                            // Kill switch: `End` is already on the wire (see
                            // `handle_stdin_chunk`); stop the source — kill
                            // the subshell, or merely detach from a source we
                            // do not own — and collect the exit code during
                            // teardown.
                            stop();
                            break LoopExit::KillSwitch;
                        }
                    }
                    // Host stdin closed; the session keeps running.
                    None => stdin_open = false,
                },
                msg = in_rx.recv(), if hub_open => match msg {
                    Some(msg) => task.handle_inbound(msg),
                    // Transport gone; the shell keeps running locally.
                    None => hub_open = false,
                },
                sig = recv_signal(&mut winch), if winch_open => match sig {
                    Some(()) => task.handle_winch(),
                    // Signal driver gone — cannot happen while the runtime is
                    // live; stop polling either way.
                    None => winch_open = false,
                },
                // Headless stop request: `--stop` sends SIGTERM; Ctrl-C in
                // the foreground debug run sends SIGINT; a dying terminal
                // would send SIGHUP. Detach from the source and run the
                // End-emitting teardown.
                sig = recv_signal(&mut sigint), if sigint_open => match sig {
                    Some(()) => {
                        stop();
                        break LoopExit::Stopped;
                    }
                    None => sigint_open = false,
                },
                sig = recv_signal(&mut sigterm), if sigterm_open => match sig {
                    Some(()) => {
                        stop();
                        break LoopExit::Stopped;
                    }
                    None => sigterm_open = false,
                },
                sig = recv_signal(&mut sighup), if sighup_open => match sig {
                    Some(()) => {
                        stop();
                        break LoopExit::Stopped;
                    }
                    None => sighup_open = false,
                },
                _ = keyframe_tick.tick() => task.service_keyframes(),
                _ = render_tick.tick(), if render_open => task.render_if_dirty(),
                code = &mut wait_handle => break LoopExit::Exited(code.unwrap_or(0)),
            }
        };

        // The wait arm can win the `select!` race while chunks the reader had
        // already queued still sit in the channel — the child's parting
        // output (on macOS/BSD the child blocks in `exit()` until the tty
        // output queue drains, so its final bytes being in flight right here
        // is the common case, not a corner). Feed them through the normal
        // path so the hub and the host both see them, then compose one last
        // frame. `try_recv` is bounded, so a descendant still holding the PTY
        // slave open cannot hang teardown here.
        if matches!(exit, LoopExit::Exited(_)) {
            while let Ok(event) = pty_rx.try_recv() {
                match event {
                    ReadEvent::Output(chunk) => task.handle_pty_chunk(&chunk),
                    ReadEvent::Resize(size) => task.handle_source_resize(size),
                }
            }
            // Best-effort: a busy writer drops it, like a missed render tick.
            task.render_if_dirty();
        }

        // Clean exit also ends the session on the hub (link invalidated now,
        // not after the grace period). On the kill-switch path this is the
        // second `End`; the transport stopped at the first, so it is inert.
        let _ = task.out_tx.send(Outbound::End);

        // Dropping the task state drops the resizer closure (for the subshell,
        // the session's handle on the PTY master), `child_tx` (the pty-writer's
        // cue to exit), `frame_tx` (the terminal writer's cue), and `out_tx`
        // (the transport drains the `End` above, then returns).
        drop(task);
        // With the receiver gone the pty-reader flips to drain-only mode if it
        // was parked mid-send. It is never joined: its EOF needs every holder
        // of the PTY slave to exit, and a background process the shell left
        // behind would hang teardown — in raw mode — indefinitely (see
        // `pty_reader_loop`).
        drop(pty_rx);

        let code = match exit {
            LoopExit::Exited(code) => code,
            // The kill switch or a stop signal fired: the source was just
            // stopped; collect its code (a failed or panicked wait still maps
            // to 0). The reader keeps draining in drain-only mode, so the
            // BSD/macOS exit()-drain invariant cannot wedge this wait — see
            // `pty_reader_loop` — and a stopped tap's reader reaches EOF and
            // wakes its wait the same way.
            LoopExit::KillSwitch | LoopExit::Stopped => wait_handle.await.unwrap_or(0),
        };

        // Join the terminal writer (host mode only) off the runtime — the
        // closed frame channel guarantees its exit, and joining it means the
        // final frame reaches the host terminal before raw mode is restored.
        // The pty-reader, pty-writer and stdin-reader threads stay detached:
        // each blocks on a source or sink that may never yield again (a
        // descendant holding the PTY slave, a child that stopped reading
        // input, raw-mode stdin).
        if let Some(term_writer) = term_writer {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = term_writer.join();
            })
            .await;
        }

        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;

    const OLD: Size = Size {
        cols: 120,
        rows: 40,
    };
    const NEW: Size = Size {
        cols: 100,
        rows: 30,
    };

    #[test]
    fn resize_notice_says_nothing_when_the_size_did_not_change() {
        // The hub's first negotiation echoes the host's own geometry; calling
        // that a resize would simply be false.
        assert_eq!(resize_notice(OLD, OLD, 0), None);
        assert_eq!(resize_notice(OLD, OLD, 5), None);
    }

    #[test]
    fn resize_notice_does_not_blame_a_viewer_when_nobody_is_watching() {
        assert_eq!(resize_notice(OLD, NEW, 0), Some("resized to 100x30".to_string()));
    }

    #[test]
    fn resize_notice_blames_a_viewer_only_when_one_is_connected() {
        assert_eq!(
            resize_notice(OLD, NEW, 1),
            Some("resized to 100x30 -- a viewer's screen is smaller".to_string())
        );
        assert_eq!(
            resize_notice(OLD, NEW, 7),
            Some("resized to 100x30 -- a viewer's screen is smaller".to_string())
        );
    }

    /// A [`SessionTask`] with every side effect stubbed out, plus the sizes
    /// its `resizer` was asked for. Enough to drive the resize path directly:
    /// SIGWINCH is not reachable from a unit test, and neither is
    /// `crossterm::terminal::size()`.
    fn resize_task(physical: Size) -> (SessionTask, Arc<Mutex<Vec<Size>>>) {
        let applied = Arc::new(Mutex::new(Vec::<Size>::new()));
        let sink = Arc::clone(&applied);
        let (child_tx, _child_rx) = mpsc::channel::<Vec<u8>>(8);
        let (out_tx, _out_rx) = mpsc::unbounded_channel::<Outbound>();
        let task = SessionTask {
            screen: ScreenState::new(physical),
            child_tx,
            resizer: Box::new(move |size| sink.lock().push(size)),
            out_tx,
            write: WriteMode::from_flag(false),
            answer_queries: false,
            follows_hub_resize: false,
            physical,
            child: physical,
            viewers: 0,
            input_disabled: false,
            dirty: false,
            frame_tx: None,
            url_sink: None,
        };
        (task, applied)
    }

    /// A mid-session shrink must never kill a live session: by the time
    /// SIGWINCH fires the link is out in the world, so this path clamps where
    /// startup refuses. The clamp lands on the floor, not on the 1x1 that
    /// panics `vt100` — the old `.max(1)` clamped straight onto a panicking
    /// size.
    ///
    /// Two physical rows is the worst realistic drag-to-nothing: the bar row
    /// takes one, leaving zero for the child.
    #[test]
    fn a_winch_down_to_nothing_leaves_the_child_on_the_floor() {
        let floor = Size {
            cols: crate::MIN_COLS,
            rows: crate::MIN_CHILD_ROWS,
        };
        for (cols, rows) in [(80, 2), (80, 1), (80, 0), (1, 1), (0, 0)] {
            let (mut task, applied) = resize_task(Size { cols: 80, rows: 23 });
            task.apply_host_window(cols, rows);
            assert!(
                task.child.cols >= crate::MIN_COLS && task.child.rows >= crate::MIN_CHILD_ROWS,
                "winch to {cols}x{rows} left the child at {:?}",
                task.child
            );
            assert_eq!(
                applied.lock().as_slice(),
                [task.child],
                "the source is resized to the clamped geometry, once"
            );
            // ...and the model built at that geometry survives wrapping text.
            let _ = task.screen.process_chunk(b"wrap this well past the end\r\nmore\r\n");
        }
        let (mut task, _) = resize_task(Size { cols: 80, rows: 23 });
        task.apply_host_window(1, 1);
        assert_eq!(task.child, floor);
        assert_eq!(task.physical, floor, "the bar row is subtracted here too");
    }

    /// The hub negotiates the child size too, and a hostile or confused hub
    /// may ask for a panicking one. `clamp_child` floors after the `min`, so
    /// the floor wins outright.
    #[test]
    fn a_hub_negotiated_size_below_the_floor_is_clamped_not_honoured() {
        let (task, _) = resize_task(Size { cols: 80, rows: 23 });
        assert_eq!(task.clamp_child(Size { cols: 0, rows: 0 }), Size {
            cols: crate::MIN_COLS,
            rows: crate::MIN_CHILD_ROWS
        });
        assert_eq!(task.clamp_child(Size { cols: 80, rows: 1 }), Size {
            cols: 80,
            rows: crate::MIN_CHILD_ROWS
        });
        // The physical maximum still wins over an oversized request.
        assert_eq!(
            task.clamp_child(Size {
                cols: 999,
                rows: 999
            }),
            Size { cols: 80, rows: 23 }
        );
    }

    /// The transport's fail-closed signal has to survive on screen: it flips
    /// a bar input the compositor repaints on every tick, not just a line the
    /// next repaint erases. Latched — the condition is permanent, and a
    /// repeated notice would only repaint.
    #[test]
    fn input_disabled_sticks_to_the_bar() {
        let (mut task, _) = resize_task(Size { cols: 80, rows: 23 });
        task.write = WriteMode::from_flag(true);
        assert!(!task.input_disabled);
        assert!(!bar_of(&task).contains("INPUT DISABLED"));

        task.handle_inbound(Inbound::InputDisabled);
        assert!(task.input_disabled);
        assert!(task.dirty, "the bar must be repainted with the new state");
        let bar = bar_of(&task);
        assert!(bar.contains("INPUT DISABLED"));
        assert!(!bar.contains("WRITE ON"));

        // Still there many repaints later: nothing clears it.
        task.dirty = false;
        task.handle_participants(4);
        assert!(bar_of(&task).contains("INPUT DISABLED"));

        // Idempotent.
        task.handle_inbound(Inbound::InputDisabled);
        assert!(task.input_disabled);
    }

    /// The bar the task would composite right now, as text.
    fn bar_of(task: &SessionTask) -> String {
        let bytes = StatusBar {
            viewers: task.viewers,
            write: task.write,
            input_disabled: task.input_disabled,
        }
        .render(task.physical.cols);
        String::from_utf8(bytes).expect("the bar is ASCII")
    }

    mod mock_source {
        use std::sync::{Arc, mpsc as std_mpsc};
        use std::time::Duration;

        use parking_lot::Mutex;

        use super::*;

        /// A blocking [`SourceReader`] over a std channel: the shape of a
        /// source's ordered stream, without a PTY. Yields queued events —
        /// output and in-band resizes alike — and reports the end once the
        /// sender is dropped.
        struct ChannelReader(std_mpsc::Receiver<ReadEvent>);

        impl SourceReader for ChannelReader {
            fn read_event(&mut self) -> std::io::Result<Option<ReadEvent>> {
                Ok(self.0.recv().ok())
            }
        }

        /// A `Write` into shared memory, so the test can observe what reached
        /// the source's input path from the detached pty-writer thread.
        #[derive(Clone)]
        struct SharedWriter(Arc<Mutex<Vec<u8>>>);

        impl Write for SharedWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        /// The next outbound event, bounded — a wedged session fails the test
        /// instead of hanging it.
        async fn next_outbound(rx: &mut UnboundedReceiver<Outbound>) -> Outbound {
            tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("an outbound event within 10s")
                .expect("the session holds its sender while running")
        }

        /// The next outbound event that is not a `Keyframe` — the periodic
        /// cadence may interleave one under a slow test runner, and these
        /// assertions are about the other variants.
        async fn next_non_keyframe(rx: &mut UnboundedReceiver<Outbound>) -> Outbound {
            loop {
                match next_outbound(rx).await {
                    Outbound::Keyframe(_) => {}
                    other => break other,
                }
            }
        }

        /// The mock-source proof that `Session::run` is source-agnostic: built
        /// from plain in-memory shims — no PTY, no subshell — the REAL session
        /// loop still boots from the snapshot chunk, streams source bytes out
        /// as `Output` frames, lands viewer input in the source's writer,
        /// applies in-band source resizes at their position in the stream,
        /// and maps the source's end + `wait` to its exit code.
        #[tokio::test]
        async fn session_run_is_source_agnostic() {
            let (source_tx, source_rx) = std_mpsc::channel::<ReadEvent>();
            let sink = Arc::new(Mutex::new(Vec::new()));
            let (exit_tx, exit_rx) = std_mpsc::channel::<i32>();

            let parts = SourceParts {
                reader: Box::new(ChannelReader(source_rx)),
                writer: Box::new(SharedWriter(Arc::clone(&sink))),
                resizer: Box::new(|_| {}),
                stop: Box::new(|| {}),
                // Blocking-safe and channel-blocked: the test decides when the
                // source "exits", exactly like a child wait.
                wait: Box::new(move || exit_rx.recv().unwrap_or(0)),
                bootstrap: Some(b"boot".to_vec()),
                answer_queries: false,
                follows_hub_resize: false,
            };

            let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Outbound>();
            let (in_tx, in_rx) = mpsc::unbounded_channel::<Inbound>();
            let session = Session {
                parts,
                physical: Size { cols: 80, rows: 24 },
                write: WriteMode::from_flag(true),
                out_tx,
                in_rx,
                host: Some(HostUi {
                    stdin: Box::new(std::io::empty()),
                    stdout: Box::new(std::io::sink()),
                }),
                url_sink: None,
            };
            let run = tokio::spawn(session.run());

            // Startup order is fixed: geometry first, then the bootstrap
            // snapshot as chunk 0 — the very first `Output` frame — with the
            // initial keyframe riding it.
            assert!(matches!(next_outbound(&mut out_rx).await, Outbound::HostSize {
                cols: 80,
                rows: 24
            }));
            match next_outbound(&mut out_rx).await {
                Outbound::Output(frame) => {
                    assert_eq!(frame.seq, 1, "the bootstrap is chunk 0: seq 1");
                    assert_eq!(frame.data, b"boot");
                }
                _ => panic!("the bootstrap must be the first Output frame"),
            }
            assert!(matches!(next_outbound(&mut out_rx).await, Outbound::Keyframe(_)));

            // Live source bytes flow out as `Output` frames.
            source_tx.send(ReadEvent::Output(b"hello".to_vec())).expect("reader alive");
            match next_non_keyframe(&mut out_rx).await {
                Outbound::Output(frame) => assert_eq!(frame.data, b"hello"),
                _ => panic!("source bytes must surface as Output"),
            }

            // Viewer input (write mode) reaches the source's writer via the
            // detached pty-writer thread; poll until it lands.
            in_tx.send(Inbound::Input(b"typed".to_vec())).expect("session alive");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while sink.lock().as_slice() != b"typed" {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "viewer input never reached the source's writer"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            // A source-originated resize — in-band, through the same ordered
            // stream as the output — repaints the model (keyframe) and
            // re-advertises the new geometry to the hub.
            source_tx
                .send(ReadEvent::Resize(Size {
                    cols: 100,
                    rows: 30,
                }))
                .expect("reader alive");
            assert!(matches!(next_outbound(&mut out_rx).await, Outbound::Keyframe(_)));
            assert!(matches!(next_non_keyframe(&mut out_rx).await, Outbound::HostSize {
                cols: 100,
                rows: 30
            }));

            // Source EOF plus the wait closure's code ends the session with
            // exactly that code.
            drop(source_tx);
            exit_tx.send(0).expect("wait closure alive");
            let code = tokio::time::timeout(Duration::from_secs(10), run)
                .await
                .expect("the session ends after EOF + wait")
                .expect("the session task must not panic")
                .expect("the session must not error");
            assert_eq!(code, 0);

            // Teardown told the hub explicitly, never relying on the socket.
            let mut saw_end = false;
            while let Ok(msg) = out_rx.try_recv() {
                saw_end |= matches!(msg, Outbound::End);
            }
            assert!(saw_end, "teardown must send Outbound::End");
        }

        /// The headless proof: with `host: None` there is no `HostUi`, so no
        /// stdin/stdout handle even exists to hand a thread — the
        /// stdin-reader and terminal-writer bridges, the render tick, and the
        /// SIGWINCH listener are absent by construction, which is exactly
        /// what a daemonized session with null stdio requires. The
        /// observable half: the session still runs the full loop, and every
        /// `Connected` — the first join and a fresh-session reconnect alike
        /// — delivers its URL to the sink instead of a terminal.
        #[tokio::test]
        async fn headless_session_reports_urls_to_the_sink() {
            let (source_tx, source_rx) = std_mpsc::channel::<ReadEvent>();
            let sink = Arc::new(Mutex::new(Vec::new()));
            let (exit_tx, exit_rx) = std_mpsc::channel::<i32>();

            let parts = SourceParts {
                reader: Box::new(ChannelReader(source_rx)),
                writer: Box::new(SharedWriter(Arc::clone(&sink))),
                resizer: Box::new(|_| {}),
                stop: Box::new(|| {}),
                wait: Box::new(move || exit_rx.recv().unwrap_or(0)),
                bootstrap: None,
                answer_queries: false,
                follows_hub_resize: false,
            };

            let urls = Arc::new(Mutex::new(Vec::<String>::new()));
            let urls_sink = Arc::clone(&urls);
            let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Outbound>();
            let (in_tx, in_rx) = mpsc::unbounded_channel::<Inbound>();
            let session = Session {
                parts,
                physical: Size { cols: 80, rows: 24 },
                write: WriteMode::from_flag(false),
                out_tx,
                in_rx,
                host: None,
                url_sink: Some(Box::new(move |url| {
                    urls_sink.lock().push(url.to_string());
                })),
            };
            let run = tokio::spawn(session.run());

            assert!(matches!(next_outbound(&mut out_rx).await, Outbound::HostSize {
                cols: 80,
                rows: 24
            }));

            // First join, then a reconnect the hub turned into a NEW session:
            // both URLs must reach the sink, in order — the fresh_session
            // rewrite is the moment a persisted URL goes stale.
            in_tx
                .send(Inbound::Connected {
                    join_url: "https://hub.example/s/one#key".into(),
                    fresh_session: false,
                })
                .expect("session alive");
            in_tx
                .send(Inbound::Connected {
                    join_url: "https://hub.example/s/two#key".into(),
                    fresh_session: true,
                })
                .expect("session alive");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while urls.lock().len() < 2 {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "Connected URLs never reached the sink"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(urls.lock().as_slice(), [
                "https://hub.example/s/one#key".to_string(),
                "https://hub.example/s/two#key".to_string(),
            ]);

            // EOF + wait still end a headless session with the wait's code.
            drop(source_tx);
            exit_tx.send(0).expect("wait closure alive");
            let code = tokio::time::timeout(Duration::from_secs(10), run)
                .await
                .expect("the session ends after EOF + wait")
                .expect("the session task must not panic")
                .expect("the session must not error");
            assert_eq!(code, 0);

            let mut saw_end = false;
            while let Ok(msg) = out_rx.try_recv() {
                saw_end |= matches!(msg, Outbound::End);
            }
            assert!(saw_end, "teardown must send Outbound::End");
        }
    }
}
