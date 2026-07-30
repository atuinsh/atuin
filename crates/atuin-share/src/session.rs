//! The share session: the threads that glue the child PTY, the local
//! compositor and the hub transport together.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::Size;
use crate::keyframe::keyframe_bytes;
use crate::query;
use crate::render::{composite, render_bar};
use crate::subshell::Subshell;

/// Bytes of child output after which a fresh keyframe is emitted.
const KEYFRAME_BYTES: u64 = 256 * 1024;
/// Maximum interval between keyframes.
const KEYFRAME_INTERVAL: Duration = Duration::from_secs(5);
/// Render coalescing interval (~60 fps).
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// How often the keyframe ticker checks the cadence. Small enough that a
/// keyframe requested while the child is silent goes out promptly.
const KEYFRAME_TICK: Duration = Duration::from_millis(100);

/// Session → hub events.
pub enum Outbound {
    Output {
        seq: u64,
        data: Vec<u8>,
    },
    Keyframe {
        seq: u64,
        data: Vec<u8>,
    },
    HostSize {
        cols: u16,
        rows: u16,
    },
    /// Kill switch / clean exit: end the session and invalidate the link now.
    End,
}

/// Hub → session events.
pub enum Inbound {
    Input(Vec<u8>),
    SetSize {
        cols: u16,
        rows: u16,
    },
    Participants(u32),
    /// The hub dropped its replay buffer; send a fresh keyframe.
    RequestKeyframe,
    /// Joined (or re-joined) successfully. `fresh_session` is true when the hub
    /// issued a *different* public token than we last advertised — i.e. our
    /// resume was rejected or expired and the hub created a brand-new session.
    /// The old link is dead in that case and the new one must be re-printed.
    Connected {
        // The public view token. Part of the plan's declared `Inbound`
        // contract, but the session only ever prints `join_url` (which already
        // embeds it), so the loop destructures this as `token: _`.
        #[allow(
            dead_code,
            reason = "part of the declared Inbound contract, no in-crate reader"
        )]
        token: String,
        join_url: String,
        fresh_session: bool,
    },
    Disconnected,
}

/// State shared across the session's threads.
struct Shared {
    /// The child's screen model. **The parser lock is also the `seq` lock** —
    /// see `emit_output_locked`.
    parser: Mutex<vt100::Parser>,
    /// Monotonic sequence counter for `output`/`keyframe`, starting at 1.
    seq: AtomicU64,
    /// Set by any thread that wants a keyframe. Serviced by the reader when the
    /// child is producing output, and otherwise by the keyframe ticker — the
    /// child may be silent for minutes and the repaint must not wait for it.
    want_keyframe: AtomicBool,
    /// When the last keyframe went out, driving the periodic cadence. Shared
    /// (rather than local to the reader) so the ticker and the reader agree on
    /// the interval no matter which of them last emitted.
    last_keyframe: Mutex<Instant>,
    /// Child bytes emitted since the last keyframe, for the byte-based cadence.
    bytes_since_keyframe: AtomicU64,
    /// Screen needs repainting.
    dirty: AtomicBool,
    /// Session is shutting down.
    shutdown: AtomicBool,
    /// Current viewer count, for the bar.
    viewers: AtomicU64,
    /// Host's physical terminal size, minus the bar row. Updated on SIGWINCH.
    phys_cols: AtomicU16,
    phys_rows: AtomicU16,
    /// Negotiated child PTY size (what `set_size` last asked for, clamped).
    child_cols: AtomicU16,
    child_rows: AtomicU16,
}

impl Shared {
    /// Build the shared state for a session whose host terminal has `physical`
    /// rows *available to the child* — i.e. the real terminal height minus the
    /// bar row, already subtracted once by `run_share` (spec §6).
    fn new(physical: Size) -> Self {
        Self {
            parser: Mutex::new(vt100::Parser::new(physical.rows, physical.cols, 0)),
            seq: AtomicU64::new(1),
            want_keyframe: AtomicBool::new(true), // initial keyframe
            last_keyframe: Mutex::new(Instant::now()),
            bytes_since_keyframe: AtomicU64::new(0),
            dirty: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            viewers: AtomicU64::new(0),
            phys_cols: AtomicU16::new(physical.cols),
            phys_rows: AtomicU16::new(physical.rows),
            child_cols: AtomicU16::new(physical.cols),
            child_rows: AtomicU16::new(physical.rows),
        }
    }

    /// The rows available to the child: the host's real terminal height **minus
    /// the bar row**, which `run_share` subtracts exactly once at the source.
    /// Never the real terminal height — see `render::composite`.
    fn physical(&self) -> Size {
        Size {
            cols: self.phys_cols.load(Ordering::Relaxed),
            rows: self.phys_rows.load(Ordering::Relaxed),
        }
    }

    fn child(&self) -> Size {
        Size {
            cols: self.child_cols.load(Ordering::Relaxed),
            rows: self.child_rows.load(Ordering::Relaxed),
        }
    }

    /// Clamp a requested child size to what physically fits below the bar.
    fn clamp_child(&self, want: Size) -> Size {
        let phys = self.physical();
        Size {
            cols: want.cols.min(phys.cols).max(1),
            rows: want.rows.min(phys.rows).max(1),
        }
    }

    fn store_child(&self, size: Size) {
        self.child_cols.store(size.cols, Ordering::Relaxed);
        self.child_rows.store(size.rows, Ordering::Relaxed);
    }

    fn request_keyframe(&self) {
        self.want_keyframe.store(true, Ordering::SeqCst);
    }

    /// Restart the periodic and byte-based cadences. Called from both emit
    /// paths so whichever thread produced the keyframe, the next one is due a
    /// full interval later.
    fn note_keyframe_sent(&self) {
        self.bytes_since_keyframe.store(0, Ordering::Relaxed);
        if let Ok(mut last) = self.last_keyframe.lock() {
            *last = Instant::now();
        }
    }

    /// Whether the periodic cadence has come due.
    fn keyframe_overdue(&self) -> bool {
        self.last_keyframe
            .lock()
            .is_ok_and(|last| last.elapsed() >= KEYFRAME_INTERVAL)
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}

/// Emit `Output` (and a `Keyframe` if one was requested) for a chunk of child
/// output. Must be called with `parser` already locked by the caller so that the
/// screen state, the sequence numbers, and the sends stay consistent.
fn emit_output_locked(
    parser: &mut vt100::Parser,
    shared: &Shared,
    out_tx: &Sender<Outbound>,
    chunk: &[u8],
) -> Result<(), std::sync::mpsc::SendError<Outbound>> {
    parser.process(chunk);

    let seq = shared.seq.fetch_add(1, Ordering::SeqCst);
    out_tx.send(Outbound::Output {
        seq,
        data: chunk.to_vec(),
    })?;

    if shared.want_keyframe.swap(false, Ordering::SeqCst) {
        let seq = shared.seq.fetch_add(1, Ordering::SeqCst);
        let data = keyframe_bytes(parser.screen());
        out_tx.send(Outbound::Keyframe { seq, data })?;
        shared.note_keyframe_sent();
    }
    Ok(())
}

/// Emit a keyframe with no accompanying output (startup, resize, idle timer).
///
/// The caller must already hold the parser lock: that lock guards the screen
/// state *and* the `seq` counter together, which is what keeps the spec's
/// invariant — a keyframe stamped `seq = K` reflects exactly the output bytes
/// stamped `<= K` — true no matter which thread produces the keyframe.
fn emit_keyframe_locked(
    parser: &vt100::Parser,
    shared: &Shared,
    out_tx: &Sender<Outbound>,
) -> Result<(), std::sync::mpsc::SendError<Outbound>> {
    shared.want_keyframe.store(false, Ordering::SeqCst);
    let seq = shared.seq.fetch_add(1, Ordering::SeqCst);
    let data = keyframe_bytes(parser.screen());
    let sent = out_tx.send(Outbound::Keyframe { seq, data });
    shared.note_keyframe_sent();
    sent
}

/// Service keyframe requests the reader cannot: while the child is silent the
/// read loop is parked in `read()`, so a request raised by a resize, a hub
/// `request_keyframe`, or the periodic cadence would otherwise sit unserviced
/// until the child next writes — which may be never (spec §5.3).
fn spawn_keyframe_ticker(
    shared: Arc<Shared>,
    out_tx: Sender<Outbound>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            if shared.is_shutdown() {
                return;
            }
            std::thread::sleep(KEYFRAME_TICK);
            if shared.is_shutdown() {
                return;
            }
            if !shared.want_keyframe.load(Ordering::SeqCst) && !shared.keyframe_overdue() {
                continue;
            }
            let parser = shared.parser.lock().expect("parser lock");
            if emit_keyframe_locked(&parser, &shared, &out_tx).is_err() {
                return; // transport gone; the shell keeps running
            }
        }
    })
}

fn spawn_reader(
    shared: Arc<Shared>,
    mut reader: Box<dyn Read + Send>,
    child_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    out_tx: Sender<Outbound>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];

        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => return, // child exited or PTY closed
                Ok(n) => n,
            };
            // **Keep draining the PTY even while shutting down.** On BSD/macOS a
            // session leader blocks inside `exit()` until its controlling tty's
            // output queue has drained, so a reader that stops reading while the
            // child is still alive wedges the child in the "exiting" state and
            // `Subshell::wait()` — which teardown calls right after `kill()` —
            // never returns. Only the *emitting* stops once the session is over.
            if shared.is_shutdown() {
                continue;
            }
            let chunk = &buf[..n];

            // Cadence: ask for a keyframe before emitting, so it is produced in
            // the same critical section and lands right after this output. The
            // counters live in `Shared` because the ticker services the same
            // cadence when the child goes quiet.
            let total = shared
                .bytes_since_keyframe
                .fetch_add(n as u64, Ordering::Relaxed)
                + n as u64;
            if total >= KEYFRAME_BYTES || shared.keyframe_overdue() {
                shared.request_keyframe();
            }

            let replies = {
                let mut parser = shared.parser.lock().expect("parser lock");
                if emit_output_locked(&mut parser, &shared, &out_tx, chunk).is_err() {
                    return; // transport gone; the shell keeps running
                }
                // Answer device queries using the post-update cursor position.
                query::replies(chunk, parser.screen().cursor_position())
            };
            if !replies.is_empty()
                && let Ok(mut w) = child_writer.lock()
            {
                let _ = w.write_all(&replies);
                let _ = w.flush();
            }
            shared.mark_dirty();
        }
    })
}

fn spawn_renderer(
    shared: Arc<Shared>,
    mut stdout: Box<dyn Write + Send>,
    write_enabled: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            if shared.is_shutdown() {
                return;
            }
            std::thread::sleep(FRAME_INTERVAL);
            if !shared.dirty.swap(false, Ordering::Relaxed) {
                continue;
            }
            // Rows available to the child: the bar row was already subtracted
            // once, by `run_share`. `composite` clamps against this and adds the
            // bar row back itself, so it must not be subtracted again here.
            let avail = shared.physical();
            let child = shared.child();
            let viewers = u32::try_from(shared.viewers.load(Ordering::Relaxed)).unwrap_or(u32::MAX);
            let bar = render_bar(avail.cols, viewers, write_enabled);

            let frame = {
                let parser = shared.parser.lock().expect("parser lock");
                composite(parser.screen(), child, avail, &bar)
            };
            if stdout.write_all(&frame).is_err() || stdout.flush().is_err() {
                return;
            }
        }
    })
}

fn spawn_stdin(
    shared: Arc<Shared>,
    mut stdin: Box<dyn Read + Send>,
    child_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    out_tx: Sender<Outbound>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            if shared.is_shutdown() {
                return;
            }
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            // Kill switch: Ctrl-\ (0x1c). Raw mode disables ISIG, so it arrives
            // as a plain byte rather than raising SIGQUIT. Everything before it
            // in the same chunk is still delivered to the child.
            if let Some(pos) = buf[..n].iter().position(|&b| b == 0x1c) {
                if pos > 0
                    && let Ok(mut w) = child_writer.lock()
                {
                    let _ = w.write_all(&buf[..pos]);
                    let _ = w.flush();
                }
                // Tell the hub explicitly — never rely on socket closure, which
                // would leave the link alive for the hub's 30 s grace period.
                let _ = out_tx.send(Outbound::End);
                shared.shutdown.store(true, Ordering::SeqCst);
                return;
            }
            // The host's own keystrokes are always forwarded, regardless of
            // `--write` (that flag governs *viewer* input only).
            if let Ok(mut w) = child_writer.lock()
                && (w.write_all(&buf[..n]).is_err() || w.flush().is_err())
            {
                return;
            }
        }
    })
}

/// Watch for terminal resizes.
///
/// The clamp is applied **locally and immediately**, without waiting for the
/// hub: if the host shrinks their window, a child still sized to the old
/// (taller) geometry would be painted past the last visible row, scrolling the
/// status bar off the screen. It also keeps resize working while the transport
/// is down.
#[cfg(unix)]
fn spawn_winch(
    shared: Arc<Shared>,
    subshell_resize: impl Fn(Size) + Send + 'static,
    out_tx: Sender<Outbound>,
) -> eyre::Result<std::thread::JoinHandle<()>> {
    use signal_hook::consts::SIGWINCH;
    use signal_hook::iterator::Signals;

    let mut signals = Signals::new([SIGWINCH])?;
    Ok(std::thread::spawn(move || {
        for _ in signals.forever() {
            if shared.is_shutdown() {
                return;
            }
            let Ok((cols, rows)) = crossterm::terminal::size() else {
                continue;
            };
            // Reserve the bar row.
            let phys = Size {
                cols,
                rows: rows.saturating_sub(1).max(1),
            };
            shared.phys_cols.store(phys.cols, Ordering::Relaxed);
            shared.phys_rows.store(phys.rows, Ordering::Relaxed);

            // Clamp the child immediately — do not wait for the hub's set_size.
            let clamped = shared.clamp_child(shared.child());
            shared.store_child(clamped);
            subshell_resize(clamped);
            {
                // NB: `set_size` lives on `Screen`, not `Parser` — reach it via
                // `screen_mut()`. (`vt100` 0.16.2 has no `Parser::set_size`.)
                // Emit the repaint in the same critical section: the child may
                // never write again, and a viewer that resized without one would
                // be left painting into a stale grid (spec §5.3).
                let mut parser = shared.parser.lock().expect("parser lock");
                parser.screen_mut().set_size(clamped.rows, clamped.cols);
                let _ = emit_keyframe_locked(&parser, &shared, &out_tx);
            }
            shared.mark_dirty();

            // Then let the hub renegotiate against the new maximum.
            if out_tx
                .send(Outbound::HostSize {
                    cols: phys.cols,
                    rows: phys.rows,
                })
                .is_err()
            {
                // Transport gone; the local resize above already took effect.
            }
        }
    }))
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
    let dims = format!("{}×{}", applied.cols, applied.rows);
    Some(if viewers == 0 {
        format!("resized to {dims}")
    } else {
        format!("resized to {dims} — a viewer's screen is smaller")
    })
}

fn spawn_inbound(
    shared: Arc<Shared>,
    in_rx: Receiver<Inbound>,
    child_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    out_tx: Sender<Outbound>,
    subshell_resize: impl Fn(Size) + Send + 'static,
    write_enabled: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(msg) = in_rx.recv() {
            if shared.is_shutdown() {
                return;
            }
            match msg {
                Inbound::Input(bytes) => {
                    // Defence in depth (spec §8): a read-only session must never
                    // execute viewer keystrokes, even if the hub sends them.
                    // The hub enforces this too; we do not depend on it.
                    if !write_enabled {
                        continue;
                    }
                    if let Ok(mut w) = child_writer.lock() {
                        let _ = w.write_all(&bytes);
                        let _ = w.flush();
                    }
                }
                Inbound::SetSize { cols, rows } => {
                    let clamped = shared.clamp_child(Size { cols, rows });
                    let previous = shared.child();
                    shared.store_child(clamped);
                    subshell_resize(clamped);
                    {
                        // `set_size` is on `Screen`, reached via `screen_mut()`.
                        // The keyframe goes out in the same critical section —
                        // an idle child would otherwise leave every viewer
                        // resized but never repainted (spec §5.3).
                        let mut parser = shared.parser.lock().expect("parser lock");
                        parser.screen_mut().set_size(clamped.rows, clamped.cols);
                        let _ = emit_keyframe_locked(&parser, &shared, &out_tx);
                    }
                    shared.mark_dirty();
                    // Explain the resize to the host (spec §6) — but only when
                    // something actually changed.
                    let viewers =
                        u32::try_from(shared.viewers.load(Ordering::Relaxed)).unwrap_or(u32::MAX);
                    if let Some(notice) = resize_notice(previous, clamped, viewers) {
                        eprintln!("\r\n[atuin lab share] {notice}\r");
                    }
                }
                Inbound::Participants(n) => {
                    shared.viewers.store(u64::from(n), Ordering::Relaxed);
                    shared.mark_dirty();
                }
                Inbound::RequestKeyframe => {
                    // The hub lost its replay buffer. Producing the keyframe
                    // under the parser lock keeps the seq invariant intact even
                    // though this is not the reader thread.
                    let parser = shared.parser.lock().expect("parser lock");
                    let _ = emit_keyframe_locked(&parser, &shared, &out_tx);
                }
                Inbound::Connected {
                    token: _,
                    join_url,
                    fresh_session,
                } => {
                    // A rejected/expired resume makes the hub mint a NEW session
                    // with a NEW public token, silently. If we kept advertising
                    // the old URL, viewers could never rejoin — so re-print it
                    // prominently whenever the token changed.
                    if fresh_session {
                        println!(
                            "\r\n  Reconnected as a NEW session — the previous link is dead.\r\n  New link: {join_url}\r"
                        );
                    } else {
                        println!("\r\n  Share this link: {join_url}\r");
                    }
                    let parser = shared.parser.lock().expect("parser lock");
                    let _ = emit_keyframe_locked(&parser, &shared, &out_tx);
                    shared.mark_dirty();
                }
                Inbound::Disconnected => {
                    // Keep running: the subshell must survive transport blips.
                    shared.mark_dirty();
                }
            }
        }
    })
}

pub struct Session;

impl Session {
    /// Run the share session until the child exits or the host presses Ctrl-\.
    /// Returns the child's exit code. Does **not** touch terminal modes — see
    /// `run_share`, which owns raw mode via an RAII guard.
    ///
    /// # Errors
    ///
    /// Returns an error if the SIGWINCH handler cannot be installed.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        mut subshell: Subshell,
        physical: Size,
        write: bool,
        out_tx: Sender<Outbound>,
        in_rx: Receiver<Inbound>,
        stdin: Box<dyn Read + Send>,
        stdout: Box<dyn Write + Send>,
    ) -> eyre::Result<i32> {
        let shared = Arc::new(Shared::new(physical));

        let child_writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(subshell.writer()));
        let reader = subshell.reader();

        // `Subshell::resize` needs the master PTY; hand the threads a closure.
        let resize_handle = subshell.resize_handle();
        let resize_for_winch = Arc::clone(&resize_handle);
        let resize_for_inbound = resize_handle;

        // Tell the hub our starting geometry immediately, so its very first
        // negotiation has a host dimension to work with.
        let _ = out_tx.send(Outbound::HostSize {
            cols: physical.cols,
            rows: physical.rows,
        });

        let t_reader = spawn_reader(
            Arc::clone(&shared),
            reader,
            Arc::clone(&child_writer),
            out_tx.clone(),
        );
        let t_render = spawn_renderer(Arc::clone(&shared), stdout, write);
        let t_keyframe = spawn_keyframe_ticker(Arc::clone(&shared), out_tx.clone());
        let t_stdin = spawn_stdin(
            Arc::clone(&shared),
            stdin,
            Arc::clone(&child_writer),
            out_tx.clone(),
        );
        let t_inbound = spawn_inbound(
            Arc::clone(&shared),
            in_rx,
            Arc::clone(&child_writer),
            out_tx.clone(),
            move |s| resize_for_inbound(s),
            write,
        );
        #[cfg(unix)]
        let t_winch = spawn_winch(
            Arc::clone(&shared),
            move |s| resize_for_winch(s),
            out_tx.clone(),
        )?;

        // Wait for either the child to exit or the kill switch to fire.
        let exit_code = loop {
            if shared.is_shutdown() {
                // Kill switch: terminate the child and reap it.
                subshell.kill();
                break subshell.wait().unwrap_or(0);
            }
            match subshell.try_wait() {
                Ok(Some(code)) => break code,
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break 0,
            }
        };

        // Clean exit also ends the session on the hub (link invalidated now,
        // not after the grace period).
        let _ = out_tx.send(Outbound::End);
        shared.shutdown.store(true, Ordering::SeqCst);

        // Dropping the subshell drops the PTY master, which SIGHUPs the child
        // and unblocks the reader thread.
        drop(subshell);
        let _ = t_reader.join();
        let _ = t_render.join();
        let _ = t_keyframe.join();
        // stdin/inbound/winch threads block on their sources; they are detached
        // and exit when the process does. Do not join them or teardown hangs.
        drop(t_stdin);
        drop(t_inbound);
        #[cfg(unix)]
        drop(t_winch);

        Ok(exit_code)
    }
}
