//! The share session: one central `select!` task that owns all session state,
//! plus the four bridged threads that cover the blocking edges tokio cannot —
//! the PTY read and write, the raw-mode stdin read, and the terminal write.

mod screen;

use std::io::{Read, Write};

use serde_json::Value;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use self::screen::{FRAME_INTERVAL, KEYFRAME_TICK, ScreenState};
use crate::Size;
use crate::backpressure::Frame;
use crate::protocol::b64_decode;
use crate::render::{Compositor, StatusBar, WriteMode};
use crate::subshell::{PtyResizer, Subshell, SubshellParts};

/// The host-side kill switch: Ctrl-\. Raw mode disables `ISIG`, so it arrives
/// as a plain byte rather than raising `SIGQUIT`.
const KILL_SWITCH_BYTE: u8 = 0x1c;
/// Read-buffer size for child PTY output (one chunk per `Outbound::Output`).
const PTY_READ_BUF: usize = 8192;
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

/// Session → hub events.
pub(crate) enum Outbound {
    /// One chunk of raw child output, stamped with its `seq`.
    Output(Frame),
    /// A full-screen repaint, stamped with its `seq` (see the seq invariant on
    /// [`ScreenState`]).
    Keyframe(Frame),
    /// The host's terminal geometry (already minus the bar row), for the hub
    /// to negotiate viewer sizes against.
    HostSize { cols: u16, rows: u16 },
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
            "input" => b64_decode(payload["data"].as_str().unwrap_or_default())
                .ok()
                .map(Self::Input),
            "set_size" => Some(Self::SetSize {
                cols: u16::try_from(payload["cols"].as_u64()?).ok()?,
                rows: u16::try_from(payload["rows"].as_u64()?).ok()?,
            }),
            "participants" => Some(Self::Participants(
                u32::try_from(payload["count"].as_u64()?).ok()?,
            )),
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
    /// Resizes the child PTY. Holds the boxed PTY master — though not the
    /// only master fd; see [`PtyResizer`].
    resizer: PtyResizer,
    out_tx: UnboundedSender<Outbound>,
    write: WriteMode,
    /// The rows available to the child: the host's real terminal height
    /// **minus the bar row**, subtracted exactly once at the source
    /// (`run_share` at startup, [`Self::handle_winch`] on SIGWINCH). Never the
    /// real terminal height — see [`Compositor`].
    physical: Size,
    /// Negotiated child PTY size (what `set_size` last asked for, clamped).
    child: Size,
    /// Current viewer count, for the bar.
    viewers: u32,
    /// Screen needs repainting.
    dirty: bool,
    /// Hands composed frames to the terminal-writer thread.
    frame_tx: mpsc::Sender<Vec<u8>>,
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
        if !outcome.replies.is_empty() {
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
                self.apply_resize(Size { cols, rows }, ResizeSource::Hub);
            }
            Inbound::Participants(count) => self.handle_participants(count),
            Inbound::RequestKeyframe => self.handle_request_keyframe(),
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
        // viewers could never rejoin — so re-print it prominently whenever the
        // token changed.
        if fresh_session {
            println!(
                "\r\n  Reconnected as a NEW session -- the previous link is dead.\r\n  New link: {join_url}\r"
            );
        } else {
            println!("\r\n  Share this link: {join_url}\r");
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
        // Reserve the bar row — the second of the two documented
        // at-the-source subtractions (`run_share` does the other at startup).
        self.physical = Size {
            cols,
            rows: rows.saturating_sub(1).max(1),
        };
        self.apply_resize(self.child, ResizeSource::HostWindow);
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
        self.resizer.resize(clamped);
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
                // something actually changed.
                if let Some(notice) = resize_notice(previous, clamped, self.viewers) {
                    eprintln!("\r\n[atuin lab share] {notice}\r");
                }
            }
        }
    }

    /// Clamp a requested child size to what physically fits below the bar.
    fn clamp_child(&self, want: Size) -> Size {
        Size {
            cols: want.cols.min(self.physical.cols).max(1),
            rows: want.rows.min(self.physical.rows).max(1),
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
        }
        .render(avail.cols);
        let frame = Compositor { avail }.composite(self.screen.screen(), self.child, &bar);
        if self.frame_tx.try_send(frame).is_err() {
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
fn pty_reader_loop(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; PTY_READ_BUF];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => return, // child exited or PTY closed
            Ok(n) => n,
        };
        // **Keep draining the PTY even when the session is gone.** On
        // BSD/macOS a session leader blocks inside `exit()` until its
        // controlling tty's output queue has drained, so a reader that stops
        // reading while the child is still alive wedges the child in the
        // "exiting" state and the `child.wait()` on the blocking pool — which
        // teardown awaits right after `kill()` — never returns. A send error
        // only means the session dropped the receiver at teardown: discard the
        // chunk (drain-only mode) and read on until EOF.
        let _ = tx.blocking_send(buf[..n].to_vec());
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

/// One share session, assembled by `run_share` (struct literal) and consumed
/// by [`Self::run`].
///
/// Does **not** touch terminal modes — `run_share` owns raw mode via an RAII
/// guard, which keeps the session unit-testable without touching the test
/// runner's terminal.
pub(crate) struct Session {
    /// The spawned child shell, still whole; [`Self::run`] splits it.
    pub(crate) subshell: Subshell,
    /// Host terminal size **minus the bar row** — the bar row is subtracted
    /// exactly once, by `run_share`; see [`SessionTask::physical`].
    pub(crate) physical: Size,
    /// Whether viewer keystrokes may reach the child.
    pub(crate) write: WriteMode,
    /// Session → transport events.
    pub(crate) out_tx: UnboundedSender<Outbound>,
    /// Transport → session events.
    pub(crate) in_rx: UnboundedReceiver<Inbound>,
    /// Host keystrokes; handed to the detached stdin-reader thread.
    pub(crate) stdin: Box<dyn Read + Send>,
    /// The host terminal; handed to the terminal-writer thread.
    pub(crate) stdout: Box<dyn Write + Send>,
}

impl Session {
    /// Run the share session until the child exits or the host presses Ctrl-\.
    /// Returns the child's exit code.
    ///
    /// # Errors
    ///
    /// Returns an error if the SIGWINCH listener cannot be installed or a
    /// bridge thread cannot be spawned.
    pub(crate) async fn run(self) -> crate::Result<i32> {
        let Self {
            subshell,
            physical,
            write,
            out_tx,
            mut in_rx,
            stdin,
            stdout,
        } = self;
        let SubshellParts {
            reader,
            writer,
            resizer,
            mut killer,
            mut child,
        } = subshell.into_parts();

        // Registering the listener needs the caller's runtime — guaranteed,
        // since `run_share` awaits us on it.
        let mut winch = signal(SignalKind::window_change())?;

        // Tell the hub our starting geometry immediately, so its very first
        // negotiation has a host dimension to work with.
        let _ = out_tx.send(Outbound::HostSize {
            cols: physical.cols,
            rows: physical.rows,
        });

        // The four bridged threads; each one's raison d'être is on its loop.
        // Only the terminal writer is ever joined — the other three block on
        // sources or sinks that may never yield again, so they are detached.
        let (pty_tx, mut pty_rx) = mpsc::channel::<Vec<u8>>(PTY_CHANNEL_CAP);
        let _pty_reader = std::thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || pty_reader_loop(reader, pty_tx))?;
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(STDIN_CHANNEL_CAP);
        let _stdin_reader = std::thread::Builder::new()
            .name("stdin-reader".into())
            .spawn(move || stdin_reader_loop(stdin, stdin_tx))?;
        let (child_tx, child_rx) = mpsc::channel::<Vec<u8>>(PTY_WRITE_CHANNEL_CAP);
        let _pty_writer = std::thread::Builder::new()
            .name("pty-writer".into())
            .spawn(move || pty_writer_loop(writer, child_rx))?;
        let (frame_tx, frame_rx) = mpsc::channel::<Vec<u8>>(1);
        let term_writer = std::thread::Builder::new()
            .name("terminal-writer".into())
            .spawn(move || terminal_writer_loop(stdout, frame_rx))?;

        // The child's exit: one bounded blocking call on the blocking pool,
        // replacing the old 20 ms `try_wait` poll. The code mapping is the one
        // the session has always applied — the child's own code when the wait
        // succeeds (non-`i32` codes clamp to 1), 0 when it fails.
        let mut wait_handle = tokio::task::spawn_blocking(move || match child.wait() {
            Ok(status) => i32::try_from(status.exit_code()).unwrap_or(1),
            Err(_) => 0,
        });

        let mut task = SessionTask {
            screen: ScreenState::new(physical),
            child_tx,
            resizer,
            out_tx,
            write,
            physical,
            child: physical,
            viewers: 0,
            dirty: true,
            frame_tx,
        };

        // Both tickers sleep first (like the threads they replace) and skip
        // missed ticks rather than bursting to catch up.
        let mut keyframe_tick = interval_at(Instant::now() + KEYFRAME_TICK, KEYFRAME_TICK);
        keyframe_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut render_tick = interval_at(Instant::now() + FRAME_INTERVAL, FRAME_INTERVAL);
        render_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // A closed channel must stop being polled — `recv` on one returns
        // `None` immediately, which would spin the loop.
        let mut pty_open = true;
        let mut stdin_open = true;
        let mut hub_open = true;

        // Every arm is cancellation-safe: mpsc `recv`, `Interval::tick`,
        // `Signal::recv`, and `&mut JoinHandle`.
        let exit = loop {
            tokio::select! {
                chunk = pty_rx.recv(), if pty_open => match chunk {
                    Some(chunk) => task.handle_pty_chunk(&chunk),
                    // Reader hit EOF; the wait arm below ends the loop.
                    None => pty_open = false,
                },
                chunk = stdin_rx.recv(), if stdin_open => match chunk {
                    Some(chunk) => {
                        if task.handle_stdin_chunk(&chunk) {
                            // Kill switch: `End` is already on the wire (see
                            // `handle_stdin_chunk`); terminate the child and
                            // collect its exit code during teardown.
                            let _ = killer.kill();
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
                // `None` (signal driver gone) cannot happen while the runtime
                // is live; treat it as a spurious wakeup either way.
                sig = winch.recv() => if sig.is_some() { task.handle_winch() },
                _ = keyframe_tick.tick() => task.service_keyframes(),
                _ = render_tick.tick() => task.render_if_dirty(),
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
            while let Ok(chunk) = pty_rx.try_recv() {
                task.handle_pty_chunk(&chunk);
            }
            // Best-effort: a busy writer drops it, like a missed render tick.
            task.render_if_dirty();
        }

        // Clean exit also ends the session on the hub (link invalidated now,
        // not after the grace period). On the kill-switch path this is the
        // second `End`; the transport stopped at the first, so it is inert.
        let _ = task.out_tx.send(Outbound::End);

        // Dropping the task state drops the resizer (the session's handle on
        // the PTY master), `child_tx` (the pty-writer's cue to exit),
        // `frame_tx` (the terminal writer's cue), and `out_tx` (the transport
        // drains the `End` above, then returns).
        drop(task);
        // With the receiver gone the pty-reader flips to drain-only mode if it
        // was parked mid-send. It is never joined: its EOF needs every holder
        // of the PTY slave to exit, and a background process the shell left
        // behind would hang teardown — in raw mode — indefinitely (see
        // `pty_reader_loop`).
        drop(pty_rx);

        let code = match exit {
            LoopExit::Exited(code) => code,
            // The kill switch fired: reap the child for its code (a failed or
            // panicked wait still maps to 0). The reader keeps draining the
            // PTY in drain-only mode, so the BSD/macOS exit()-drain invariant
            // cannot wedge this wait — see `pty_reader_loop`.
            LoopExit::KillSwitch => wait_handle.await.unwrap_or(0),
        };

        // Join the terminal writer off the runtime — the closed frame channel
        // guarantees its exit, and joining it means the final frame reaches
        // the host terminal before raw mode is restored. The pty-reader,
        // pty-writer and stdin-reader threads stay detached: each blocks on a
        // source or sink that may never yield again (a descendant holding the
        // PTY slave, a child that stopped reading input, raw-mode stdin).
        let _ = tokio::task::spawn_blocking(move || {
            let _ = term_writer.join();
        })
        .await;

        Ok(code)
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(
            resize_notice(OLD, NEW, 0),
            Some("resized to 100x30".to_string())
        );
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
}
