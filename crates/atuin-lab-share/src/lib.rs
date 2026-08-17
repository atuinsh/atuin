#![deny(unsafe_code)]
#![allow(clippy::disallowed_methods)]

//! Experimental terminal sharing for atuin (`atuin lab share`).
//!
//! # Known limitations
//!
//! The CLI re-renders the child shell from its own `vt100` model rather than
//! passing bytes straight through, so the real terminal never sees the child's
//! output and **terminal queries would go unanswered**. `vt100` 0.16.2 models no
//! device-report sequences.
//!
//! * `\x1b[6n` (Cursor Position Report) and `\x1b[c` (Primary Device Attributes)
//!   are intercepted and answered synthetically. Without this, TUIs that probe on
//!   startup hang.
//! * Mouse reporting, sixel/kitty graphics, and other sequences `vt100` does not
//!   model are unsupported in v1.
//! * Intercepting `Ctrl-\` costs the host the ability to send `SIGQUIT` to the
//!   child (raw mode disables `ISIG`, so `0x1c` arrives as a plain byte).
//!
//! # Module map
//!
//! Everything below [`run_share`] except [`crypto`] and [`lifecycle`] is
//! crate-private; the modules split along the session's data flow:
//!
//! * [`crypto`] — end-to-end encryption: the per-session AES-256-GCM key, the
//!   sealed-blob wire format, and the URL-fragment key encoding (public: it
//!   pins the wire format the viewer implements too).
//! * `source` — the seam between the session and whatever produces its bytes:
//!   `SourceParts` and the `SessionSource` trait, so the session runs the
//!   same loop over any byte-faithful source.
//! * `subshell` — the child shell on its PTY (`portable-pty`), split into the
//!   parts the session's task topology needs via `SessionSource`.
//! * `proxy_tap` — the `--active` source: a subscriber tap on a running
//!   `atuin pty-proxy`, speaking the framed protocol from
//!   `atuin_pty_proxy::protocol`, behind the same `SessionSource` seam.
//! * [`lifecycle`] — the backgrounded `--active` session's coordination
//!   files: the pidfile whose exclusive lock means "a share is running", the
//!   URL file `--url` reprints, and the pre-runtime daemonize step (public:
//!   the CLI's spawning parent, `--stop`, and `--url` flows consume it).
//! * `session` — the heart of the crate: one central `select!` task owning all
//!   session state, plus the four bridged threads covering the blocking edges
//!   (PTY read and write, raw-mode stdin, terminal write). `session::screen`
//!   fuses the `vt100` model with the `seq` counter and the keyframe cadence.
//! * `render` — what the host sees: the warning bar, the compositor, and the
//!   keyframe bytes viewers replay.
//! * `query` — synthetic answers to the terminal probes (CPR / DA) that the
//!   compositing model would otherwise swallow.
//! * `transport` — the hub client: Phoenix channel over WebSocket, reconnect
//!   with backoff, session resume via the secret host token. Also the E2EE
//!   seam: it owns the session key, seals outbound frames, opens viewer input,
//!   and appends the key fragment to the join URL.
//! * `protocol` — the minimal Phoenix v2 JSON codec and the base64 helpers.
//! * `backpressure` — the latest-wins outbound queue and the reconnect backoff,
//!   both pure state machines.
//! * `error` — the crate's typed [`Error`] (the one unconditional module; the
//!   non-unix stub needs it too).

#[cfg(unix)]
use std::io::{IsTerminal as _, Write as _};

use url::Url;

#[cfg(unix)]
mod backpressure;
#[cfg(unix)]
pub mod crypto;
mod error;
#[cfg(unix)]
pub mod lifecycle;
#[cfg(unix)]
mod protocol;
#[cfg(unix)]
mod proxy_tap;
#[cfg(unix)]
mod query;
#[cfg(unix)]
mod render;
#[cfg(unix)]
mod session;
#[cfg(unix)]
mod source;
#[cfg(unix)]
mod subshell;
#[cfg(unix)]
mod transport;

pub use error::{Error, Result};

/// Child-shell terminal dimensions, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// Width, in character cells.
    pub cols: u16,
    /// Height, in rows.
    pub rows: u16,
}

/// Narrowest grid `vt100` 0.16.2 survives.
///
/// A zero-column grid panics with an index-out-of-bounds inside vt100's
/// `row.rs` (`Row::cells` indexes a zero-cell row) the moment anything is
/// drawn. One column is genuinely fine: 1x24 renders, composites and
/// keyframes cleanly.
///
/// Do NOT "tidy" this to 0, and do not assume its partner is 1 either — the
/// row floor is deliberately different, see [`MIN_CHILD_ROWS`].
pub(crate) const MIN_COLS: u16 = 1;

/// Shortest **child** grid `vt100` 0.16.2 survives, i.e. rows available to the
/// shell *after* the warning bar's row has been subtracted.
///
/// A one-row grid panics with a subtract-overflow inside vt100's `grid.rs`
/// (`prev_pos.row -= scrolled` underflows when the only row scrolls away), so
/// two is the floor and one is not — measured directly, at every width: a
/// 1-row grid panics, a 2-row grid does not. As *physical* subshell sizes,
/// that puts the boundary between 80x2 (panics) and 80x3 (clean), and it is
/// why `.max(1)` was never protection: 1x1 is itself a panicking size.
///
/// The physical terminal therefore needs `MIN_CHILD_ROWS + 1` rows on the
/// subshell path (bar row plus child rows) and `MIN_CHILD_ROWS` on the
/// tap/headless path, which draws no bar.
pub(crate) const MIN_CHILD_ROWS: u16 = 2;

/// The child geometry a host terminal of `cols` x `rows` yields, floored at
/// what `vt100` survives.
///
/// The bar row is subtracted here and **only** here on each of the two
/// documented at-the-source paths ([`read_host_size`] at startup and
/// `session::SessionTask::handle_winch` on SIGWINCH); `Size` values that leave
/// this function are already bar-adjusted, so nothing downstream may subtract
/// again.
///
/// This is the *clamping* variant, for paths that cannot refuse: a mid-session
/// shrink must not kill a live session. Startup refuses instead, via
/// [`host_size_from`] — a session too small to render is better declined
/// before anything is minted than silently shown at a size the host cannot
/// read.
#[cfg(unix)]
pub(crate) fn clamp_host_size(cols: u16, rows: u16) -> Size {
    Size {
        cols: cols.max(MIN_COLS),
        rows: rows.saturating_sub(1).max(MIN_CHILD_ROWS),
    }
}

/// The startup variant of [`clamp_host_size`]: same arithmetic, but a terminal
/// below the minimum is **refused** rather than clamped.
///
/// Pure, and separate from [`read_host_size`] purely so the threshold is
/// unit-testable without a real tty behind `crossterm::terminal::size()`.
///
/// # Errors
///
/// [`Error::TerminalTooSmall`] when `cols < MIN_COLS`, or when fewer than
/// [`MIN_CHILD_ROWS`] rows remain once the bar row is reserved. A `0x0`
/// winsize — what `script -q /dev/null CMD > file` and other CI-shaped
/// invocations hand us — lands here, which is the whole point: it used to
/// panic deep inside vt100, *after* the hub session was minted and the join
/// link printed.
#[cfg(unix)]
fn host_size_from(cols: u16, rows: u16) -> Result<Size> {
    let child_rows = rows.saturating_sub(1);
    if cols < MIN_COLS || child_rows < MIN_CHILD_ROWS {
        return Err(Error::TerminalTooSmall { cols, rows });
    }
    Ok(Size {
        cols,
        rows: child_rows,
    })
}

/// Options for a share session. The caller resolves `hub_url` and `api_token`
/// (both come from `Settings`, and the token accessor is `async`) so that
/// `run_share` needs no tokio runtime of its own.
#[derive(Debug, Clone)]
pub struct ShareOptions {
    /// Allow viewers to type into the shared shell (the `--write` flag).
    pub write: bool,
    /// Skip the confirmation prompt (the `--yes` flag). The warning lines are
    /// still printed; only the `Continue? [y/N]` question is skipped.
    pub yes: bool,
    /// Share the session already running in this terminal (the `--active`
    /// flag) by attaching to the PTY owner that was interposed at shell
    /// startup (`atuin pty-proxy`), instead of spawning a fresh subshell.
    /// Never spawns anything: without a cooperating owner it refuses.
    pub active: bool,
    /// Run the `--active` session attached to this terminal (the hidden
    /// `--foreground` debug flag): the join URL goes to stderr and Ctrl-C
    /// stops sharing. No daemonize, no pidfile, no URL file — the default
    /// `--active` path re-execs a daemonized child (`internal_daemon`)
    /// instead.
    pub foreground: bool,
    /// Run as the re-exec'd daemonized child of `--active` (the hidden
    /// `--internal-daemon` flag): hold the [`lifecycle`] pidfile lock for
    /// the session's lifetime and persist the join URL to the URL file —
    /// rewriting it whenever a reconnect mints a fresh session — instead of
    /// printing it. The CLI already forked this process into the background
    /// before its runtime was built.
    pub internal_daemon: bool,
    /// Base URL of the share hub (from settings, or `ATUIN_LAB_HUB_URL`).
    pub hub_url: Url,
    /// The Hub API token authenticating the WebSocket connection.
    pub api_token: String,
}

/// Restores terminal state on every exit path, including panics.
///
/// A composited frame leaves the scroll region set (`\x1b[2;Nr`), may leave
/// origin mode on, and `contents_formatted()` can emit `\x1b[?25l` (cursor
/// hidden). Restoring only raw mode would hand the user a broken terminal, so
/// reset DECSTBM, origin mode, cursor visibility and SGR too. Precedent:
/// `atuin-pty-proxy`'s `runtime.rs` does the same on teardown.
#[cfg(unix)]
struct TermGuard;

/// What [`TermGuard`] writes on drop: reset the scroll region (DECSTBM), origin
/// mode off, cursor visible, SGR reset, then a fresh line for the shell prompt.
#[cfg(unix)]
const TERM_RESTORE: &[u8] = b"\x1b[r\x1b[?6l\x1b[?25h\x1b[0m\r\n";

#[cfg(unix)]
impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let mut out = std::io::stdout();
        let _ = out.write_all(TERM_RESTORE);
        let _ = out.flush();
    }
}

/// Entry point for `atuin lab share`. Spawns the subshell, connects to the
/// hub, and runs the session until the shell exits or the host presses Ctrl-\.
///
/// Runs entirely on the caller's runtime and never builds one of its own: the
/// caller (`lab::Cmd::run`) already lives inside a tokio runtime, and the
/// transport task, the session's timers, and its SIGWINCH listener all attach
/// to it.
///
/// Declining the confirmation prompt returns `Ok(())` without connecting to
/// anything: no session key is minted and the hub never hears from us.
///
/// # Errors
///
/// Returns an error if stdin/stdout are not a terminal, if the confirmation
/// prompt cannot be read (non-interactive stdin without `--yes`), if the host
/// terminal size cannot be read, if the subshell cannot be spawned, or if the
/// session loop fails to start. See [`Error`] for the full set of failure
/// modes.
#[cfg(unix)]
pub async fn run_share(opts: ShareOptions) -> Result<()> {
    if opts.active {
        return run_share_active(opts).await;
    }
    check_terminal()?;

    // `--write` is a bool at the CLI boundary; convert it exactly once, here,
    // and pass the typed mode everywhere below.
    let write = render::WriteMode::from_flag(opts.write);

    // Warning + confirmation gate (spec §8). This runs BEFORE the host size is
    // read, the session key is minted, or the hub hears from us: declining
    // leaves nothing minted anywhere and exits 0. It also runs before raw mode,
    // so the prompt reads a cooked-mode line from the tty.
    if !confirm_share(false, write, opts.yes)? {
        return Ok(());
    }

    let host_size = read_host_size()?;

    // The per-session E2EE key, minted before the hub ever hears from us. It
    // moves into the transport — the only place plaintext meets the wire — and
    // reaches viewers solely as the URL fragment on the join URL, which never
    // appears in an HTTP request. The session and screen model stay key-free.
    //
    // The E2EE session key is deliberately per-process and never persisted.
    // Viewer-input replay protection is a process-lifetime nonce ledger
    // (transport::AcceptedNonces); persisting or reusing this key across
    // processes would make every blob captured before the restart replayable
    // again, reopening the input-replay defect in full.
    let key = crypto::SessionKey::generate();

    // session -> transport: unbounded, so the session's `send` is synchronous
    // and never blocks its select loop.
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<session::Outbound>();
    // transport -> session: the same shape in the other direction.
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel::<session::Inbound>();

    let (join_url, transport) = connect_to_hub(&opts, write, key, out_rx, in_tx).await?;
    let sh = spawn_subshell(host_size, &join_url, write)?;

    // Raw mode is enabled here (not in `Session::run`) so the session stays
    // unit-testable without touching the test runner's terminal. The guard
    // restores everything when it drops, on any exit path.
    crossterm::terminal::enable_raw_mode()?;
    let guard = TermGuard;

    // The session is written against the `SessionSource` seam; the subshell
    // is one source of session bytes (`source::SourceParts` has the split).
    use source::SessionSource as _;
    let session = session::Session {
        parts: sh.into_parts()?,
        physical: host_size,
        write,
        out_tx,
        in_rx,
        host: Some(session::HostUi {
            stdin: Box::new(std::io::stdin()),
            stdout: Box::new(std::io::stdout()),
        }),
        url_sink: None,
    };
    // The session is a future on this runtime; its blocking edges (PTY reads
    // and writes, stdin, terminal writes, the child wait) live on threads it
    // manages itself. The guard above restores the terminal on every path out
    // of the await — including an unwind.
    let run_result = session.run().await;

    // Explicit: `std::process::exit` below does not run destructors.
    drop(guard);
    // The session's teardown queued `Outbound::End`; wait — bounded — for
    // the transport to actually push it, so the hub invalidates the link
    // now, not after its disconnect grace period (see `flush_end`).
    flush_end(transport).await;
    let code = run_result?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Entry point for `atuin lab share --active`: attach to the PTY owner of
/// the session already running in this terminal and share it headlessly.
/// Never spawns anything — the detection ladder either finds a cooperating
/// owner or refuses with the OS truth.
///
/// Differences from the subshell path, all consequences of not owning the
/// terminal: no raw mode and no `TermGuard` (the terminal belongs to the
/// user's shell, untouched), no terminal requirement beyond the confirmation
/// prompt's (which `--yes` lifts), and the session runs with `host: None` —
/// the host-facing threads and compositor are absent, so this future can run
/// under a daemonized process with null stdio.
///
/// The join URL is delivered through the session's `url_sink` on every
/// `Connected`, including the rewrite when a reconnect mints a fresh session:
/// stderr for the foreground debug run, the [`lifecycle`] URL file for the
/// daemonized child (`--url` reprints it; the spawning parent polls it).
///
/// The exit code is always 0 on a clean end: a tap has no child of ours to
/// report on (the tap's `wait` answers 0), so unlike `run_share` there is no
/// `std::process::exit` here.
///
/// # Errors
///
/// Returns an error if no cooperating PTY owner is found
/// ([`Error::ActiveShareUnsupported`], the full refusal copy), if the
/// confirmation prompt cannot be read, if the attach handshake fails (see
/// [`proxy_tap::ProxyTap::attach`]), or if the hub cannot be reached.
#[cfg(unix)]
async fn run_share_active(opts: ShareOptions) -> Result<()> {
    let write = render::WriteMode::from_flag(opts.write);

    // The ladder runs before the confirmation prompt: a user who cannot
    // attach at all should hear that first, not after saying yes.
    let socket_path = detect_active_source()?;

    // Same gate as the subshell path, with the active line added: viewers
    // will see what is ALREADY on this screen. Declining leaves nothing
    // minted anywhere. The daemonized child never prompts: the CLI confirmed
    // in the foreground parent and re-exec'd this process with `--yes`.
    if !confirm_share(true, write, opts.yes)? {
        return Ok(());
    }

    // The daemonized child claims being THE active share before touching the
    // proxy or the hub: colliding with a running share must fail fast, not
    // after minting a session. The guard holds the lock — the aliveness
    // signal `--stop`/`--url` and the spawning parent probe — until this
    // function returns and the session is fully torn down.
    let _pidfile = if opts.internal_daemon {
        let guard = lifecycle::PidfileGuard::acquire(&lifecycle::pidfile_path()).await?;
        // Stale-URL cleanup belongs HERE, behind the lock just won: the
        // child is the only writer of the URL file, so only the lock holder
        // may decide a leftover file (a previous session that died
        // uncleanly) is stale. A lock-free cleanup in the spawning parent
        // could race a concurrent launch and delete the winning session's
        // freshly written URL out from under it.
        lifecycle::remove_url_file(&lifecycle::url_file_path());
        Some(guard)
    } else {
        None
    };

    let tap = proxy_tap::ProxyTap::attach(&socket_path, write)?;
    // The tap's geometry comes from the proxy's keyframe header — the full
    // terminal size, no bar row to reserve (headless sessions draw no bar).
    let physical = tap.size();

    // The per-session E2EE key, minted only after the user confirmed and the
    // tap attached. Same custody as `run_share`: it moves into the transport
    // and reaches viewers solely as the URL fragment.
    //
    // The E2EE session key is deliberately per-process and never persisted.
    // Viewer-input replay protection is a process-lifetime nonce ledger
    // (transport::AcceptedNonces); persisting or reusing this key across
    // processes would make every blob captured before the restart replayable
    // again, reopening the input-replay defect in full. Note this path is the
    // long-lived one (`--active` attaches to a shell that may already have been
    // running for hours), so it is the likelier place for someone to "helpfully"
    // cache a key across restarts. Do not.
    let key = crypto::SessionKey::generate();

    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<session::Outbound>();
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel::<session::Inbound>();

    // The returned URL is deliberately unused: the session's `Connected`
    // handler announces it through the sink below — the steady-state writer,
    // which also re-announces a fresh session's replacement link.
    let (_join_url, transport) = connect_to_hub(&opts, write, key, out_rx, in_tx).await?;

    // Where the join URL goes on every `Connected`, including the rewrite
    // when a reconnect mints a fresh session: the foreground debug run
    // prints to stderr; the daemonized child persists it for `--url` and for
    // the spawning parent's startup poll — tagged with the spawn id the
    // parent passed down, so the parent only ever accepts a URL written by
    // the child IT spawned (never a stale file's, never a racing winner's).
    // That write is best-effort — losing the URL file must not kill a
    // healthy session.
    let url_sink: session::UrlSink = if opts.internal_daemon {
        let url_file = lifecycle::url_file_path();
        let owner = std::env::var(lifecycle::SPAWN_ID_ENV).ok();
        Box::new(move |url: &str| {
            let _ = lifecycle::write_url_file(&url_file, url, owner.as_deref());
        })
    } else {
        Box::new(|url: &str| {
            eprintln!("Sharing this session at: {url}");
        })
    };

    use source::SessionSource as _;
    let session = session::Session {
        parts: tap.into_parts()?,
        physical,
        write,
        out_tx,
        in_rx,
        host: None,
        url_sink: Some(url_sink),
    };
    // Runs until tap EOF (shell exit, proxy death, `--stop`'s SIGTERM) or,
    // in the foreground debug run, Ctrl-C. The tap's exit code is always 0;
    // nothing to propagate.
    let run_result = session.run().await;
    // The session's teardown queued `Outbound::End`; wait — bounded — for
    // the transport to actually push it before this process exits, so
    // `--stop` invalidates the hub link now, not after the hub's disconnect
    // grace period (see `flush_end`).
    flush_end(transport).await;
    // Cleanup before surfacing any error: a dead session must not leave a
    // URL file advertising it. (`--stop` also removes it, as a backstop;
    // the owner cleans first — still holding the pidfile lock.)
    if opts.internal_daemon {
        lifecycle::remove_url_file(&lifecycle::url_file_path());
    }
    let _code = run_result?;
    Ok(())
}

/// Wait — bounded by `END_FLUSH_TIMEOUT` — for the transport task to
/// finish after the session ends.
///
/// The session's every exit path queues `Outbound::End` before dropping its
/// sender, but queueing is not delivery: the CLI's runtime is shut down with
/// a near-zero timeout right after `run_share` returns, so an un-awaited
/// transport task would be dropped mid-send and the hub would only end the
/// session via its multi-second disconnect grace. Awaiting the handle here
/// makes the transport's return — which happens once the `end` push is on
/// the wire, or once delivery has demonstrably failed — part of teardown.
/// Best-effort by design: a wedged hub cannot hold the process hostage
/// beyond the bound.
#[cfg(unix)]
async fn flush_end(transport: tokio::task::JoinHandle<()>) {
    /// How long teardown waits for the final `end` push. Generous for the
    /// connected case (delivery is milliseconds) and enough for the
    /// transport's one immediate post-`End` reconnect attempt when the link
    /// was down.
    const END_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let _ = tokio::time::timeout(END_FLUSH_TIMEOUT, transport).await;
}

/// The parent half of the backgrounded `--active` flow: run the detection
/// ladder and the confirmation prompt on the caller's (interactive)
/// terminal, without attaching to the proxy or connecting to anything.
///
/// The re-exec'd daemon child has null stdio, so the CLI runs this preflight
/// first and then spawns the child with `--yes`; the child re-runs the
/// ladder in its own environment but never prompts. Returns `Ok(false)` when
/// the user declined: nothing was spawned or minted, exit 0.
///
/// # Errors
///
/// [`Error::ActiveShareUnsupported`] when no ladder rung matches, and
/// [`Error::ConfirmationRequired`] on a non-interactive stdin without `yes`.
#[cfg(unix)]
pub fn preflight_active_share(write: bool, yes: bool) -> Result<bool> {
    detect_active_source()?;
    confirm_share(true, render::WriteMode::from_flag(write), yes)
}

/// The `--active` detection ladder: find a cooperating PTY owner for the
/// running session, or refuse with the OS truth.
///
/// One rung today: an `atuin pty-proxy` advertising a live subscriber socket
/// via `$ATUIN_PTY_PROXY_SOCKET` (set and connectable). The probe connection
/// is dropped without a greeting; the proxy's greeting sniff times out and
/// serves it as a harmless legacy snapshot request.
///
/// Extension point — the tmux rung: `$TMUX` set and
/// `tmux display-message -p '#{pane_id}'` succeeding would attach through a
/// tmux-backed `SessionSource` (`pipe-pane -O` for the delta, `capture-pane`
/// for bootstrap). Out of scope for v1; slot it between the proxy rung and
/// the refusal.
///
/// # Errors
///
/// [`Error::ActiveShareUnsupported`] — the full refusal copy, stating both
/// the works-right-now path and the rc change that makes `--active` work for
/// future sessions — when no rung matches.
#[cfg(unix)]
fn detect_active_source() -> Result<std::path::PathBuf> {
    if let Ok(socket) = std::env::var("ATUIN_PTY_PROXY_SOCKET") {
        let path = std::path::PathBuf::from(socket);
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return Ok(path);
        }
    }
    Err(Error::ActiveShareUnsupported)
}

/// Validate phase: raw mode needs a real terminal on stdin, and the composited
/// frames need one on stdout.
#[cfg(unix)]
fn check_terminal() -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::NotATerminal);
    }
    Ok(())
}

/// The warning shown before every share, pure ASCII, no trailing newline.
///
/// Pure so the exact copy is pinned by unit tests: `active` adds the
/// current-screen-contents line (`--active` attaches to a terminal with
/// history already on it), `write` adds the remote-execution line. Order is
/// fixed — active line, then write line.
#[cfg(unix)]
fn warning_copy(active: bool, write: render::WriteMode) -> String {
    let mut copy = String::from(
        "!! Terminal sharing is experimental.\n  Everything shown here -- including \
         secrets you type -- is visible to anyone with the link.",
    );
    if active {
        copy.push_str(
            "\n  Viewers will see the CURRENT contents of this terminal, including \
             anything already on screen.",
        );
    }
    if write.is_write_enabled() {
        copy.push_str("\n  WRITE MODE: they can run commands on your machine.");
    }
    copy
}

/// Whether a prompt answer means yes: trimmed, ASCII-case-insensitive `y` or
/// `yes`. Anything else — including empty (plain Enter) — is a decline, per
/// the `[y/N]` default.
#[cfg(unix)]
fn is_affirmative(answer: &str) -> bool {
    let answer = answer.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

/// Confirm phase: print the warning, then ask `Continue? [y/N]` on the tty.
///
/// Runs before raw mode (cooked-mode `read_line`, no competing stdin reader)
/// and before anything network-visible, so a decline — `Ok(false)`, after
/// printing `cancelled.` — leaves no session minted anywhere. `--yes` skips
/// the question but never the warning.
///
/// # Errors
///
/// [`Error::ConfirmationRequired`] if stdin is not a terminal and `yes` is
/// false: there is no one to ask, and sharing must never start unconfirmed.
#[cfg(unix)]
fn confirm_share(active: bool, write: render::WriteMode, yes: bool) -> Result<bool> {
    eprintln!("{}", warning_copy(active, write));
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(Error::ConfirmationRequired);
    }
    eprint!("Continue? [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if is_affirmative(&answer) {
        Ok(true)
    } else {
        eprintln!("cancelled.");
        Ok(false)
    }
}

/// Measure phase: the host's terminal, with the warning-bar row reserved.
///
/// The bar row is subtracted exactly ONCE, here at the source. `host_size` is
/// what the hub negotiates against, and the `set_size` it returns is applied
/// to the child PTY directly — never subtract the bar row a second time.
/// (SIGWINCH re-measures in `session`, the other of the two subtraction sites.)
///
/// Called at exactly the right moment to *refuse*: `run_share` runs
/// `confirm_share` -> `read_host_size` -> `SessionKey::generate` ->
/// `connect_to_hub`, so an error here means no key, no hub session, and no
/// printed link — nothing is minted anywhere. Keep that order.
///
/// # Errors
///
/// [`Error::Io`] if `crossterm::terminal::size()` fails, and
/// [`Error::TerminalTooSmall`] on a window below the vt100 floor — see
/// [`host_size_from`].
#[cfg(unix)]
fn read_host_size() -> Result<Size> {
    let (cols, rows) = crossterm::terminal::size()?;
    host_size_from(cols, rows)
}

/// Connect phase: spawn the transport task and wait — bounded — for the first
/// join URL.
///
/// This runs BEFORE the shell spawns. The hub mints the session and its join
/// URL, and we want that URL in the child's environment (`ATUIN_SHARE_URL`)
/// from the very first prompt — so it can be retrieved after the printed link
/// scrolls away. It also means an unreachable hub fails with a clear message
/// instead of a shared shell that has no link.
///
/// The session `key` moves into the transport here; the URL delivered back
/// already carries the `#key` fragment the transport appends, so the printed
/// link and `ATUIN_SHARE_URL` need no further handling.
///
/// The transport is a task on our runtime; the session is awaited directly by
/// `run_share`. Neither needs a thread or runtime of its own. The transport's
/// `JoinHandle` is returned alongside the URL so teardown can await the final
/// `end` push ([`flush_end`]) — an unowned handle would let the runtime's
/// near-zero shutdown timeout drop the task with `Outbound::End` still queued.
#[cfg(unix)]
async fn connect_to_hub(
    opts: &ShareOptions,
    write: render::WriteMode,
    key: crypto::SessionKey,
    out_rx: tokio::sync::mpsc::UnboundedReceiver<session::Outbound>,
    in_tx: tokio::sync::mpsc::UnboundedSender<session::Inbound>,
) -> Result<(String, tokio::task::JoinHandle<()>)> {
    // How long to wait for the first hub connection before giving up. The
    // transport keeps retrying with backoff during this window.
    const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

    // transport -> here: the first join URL, delivered exactly once.
    let (url_tx, url_rx) = tokio::sync::oneshot::channel::<String>();

    eprintln!("Connecting to {} ...", opts.hub_url);
    let transport = tokio::spawn(
        transport::Transport::new(
            opts.hub_url.clone(),
            opts.api_token.clone(),
            write,
            key,
            url_tx,
        )
        .run(out_rx, in_tx),
    );

    match tokio::time::timeout(CONNECT_TIMEOUT, url_rx).await {
        Ok(Ok(url)) => Ok((url, transport)),
        Ok(Err(_)) => Err(Error::TransportStopped),
        Err(_) => Err(Error::HubUnreachable {
            url: opts.hub_url.clone(),
            timeout_secs: CONNECT_TIMEOUT.as_secs(),
        }),
    }
}

/// Spawn phase: the shared shell, told where its own share lives
/// (`ATUIN_SHARE_URL`) and whether viewers can type (`ATUIN_SHARE_WRITE`).
#[cfg(unix)]
fn spawn_subshell(
    host_size: Size,
    join_url: &str,
    write: render::WriteMode,
) -> Result<subshell::Subshell> {
    subshell::Subshell::spawn(
        host_size,
        &[
            ("ATUIN_SHARE_URL", join_url),
            ("ATUIN_SHARE_WRITE", write.as_env_value()),
        ],
    )
}

/// `atuin lab share` is unix-only for now (it needs a PTY).
#[cfg(not(unix))]
pub async fn run_share(_opts: ShareOptions) -> Result<()> {
    Err(Error::UnsupportedPlatform)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::render::WriteMode;

    /// The warning copy is user-visible and byte-frozen once shipped: pin all
    /// four (active, write) combinations exactly.
    #[test]
    fn warning_copy_read_only() {
        assert_eq!(
            warning_copy(false, WriteMode::ReadOnly),
            "!! Terminal sharing is experimental.\n  Everything shown here -- including \
             secrets you type -- is visible to anyone with the link."
        );
    }

    #[test]
    fn warning_copy_write() {
        assert_eq!(
            warning_copy(false, WriteMode::ReadWrite),
            "!! Terminal sharing is experimental.\n  Everything shown here -- including \
             secrets you type -- is visible to anyone with the link.\n  WRITE MODE: they \
             can run commands on your machine."
        );
    }

    #[test]
    fn warning_copy_active_read_only() {
        assert_eq!(
            warning_copy(true, WriteMode::ReadOnly),
            "!! Terminal sharing is experimental.\n  Everything shown here -- including \
             secrets you type -- is visible to anyone with the link.\n  Viewers will see \
             the CURRENT contents of this terminal, including anything already on screen."
        );
    }

    /// Both extra lines, in fixed order: active line first, write line last.
    #[test]
    fn warning_copy_active_write() {
        assert_eq!(
            warning_copy(true, WriteMode::ReadWrite),
            "!! Terminal sharing is experimental.\n  Everything shown here -- including \
             secrets you type -- is visible to anyone with the link.\n  Viewers will see \
             the CURRENT contents of this terminal, including anything already on screen.\
             \n  WRITE MODE: they can run commands on your machine."
        );
    }

    /// The copy the prompt shows must stay pure ASCII on every path.
    #[test]
    fn warning_copy_is_pure_ascii() {
        for active in [false, true] {
            for write in [WriteMode::ReadOnly, WriteMode::ReadWrite] {
                assert!(warning_copy(active, write).is_ascii());
            }
        }
    }

    #[test]
    fn affirmative_answers() {
        for answer in [
            "y", "Y", "yes", "YES", "Yes", " y ", "y\n", "yes\r\n", "\ty\t",
        ] {
            assert!(is_affirmative(answer), "{answer:?} should confirm");
        }
    }

    /// Plain Enter (the `[y/N]` default), anything unrecognized, and
    /// almost-yeses all decline.
    #[test]
    fn declining_answers() {
        for answer in [
            "", "\n", "n", "N", "no", "yess", "y es", "yeah", "ja", "si", "0", "true",
        ] {
            assert!(!is_affirmative(answer), "{answer:?} should decline");
        }
    }

    /// Error display strings are frozen once shipped; pin the new variant's.
    #[test]
    fn confirmation_required_display() {
        assert_eq!(
            Error::ConfirmationRequired.to_string(),
            "cannot confirm on a non-interactive stdin; pass --yes to share without a prompt"
        );
    }

    /// The one-active-share-per-user refusal, shown by both the daemonized
    /// child and the spawning parent: byte-pin it.
    #[test]
    fn share_already_running_display() {
        assert_eq!(
            Error::ShareAlreadyRunning.to_string(),
            "an active share is already running"
        );
    }

    /// A silent proxy is transient: the copy must say retry and point at
    /// whatever holds the socket — and must never suggest starting a new
    /// shell, which is [`Error::ProxyTooOld`]'s remedy, not this one's. It
    /// must also not *diagnose*: "busy serving another client" is one of
    /// several possible causes and, since connections moved onto their own
    /// threads, no longer the likeliest. Byte-pin it, and keep it pure ASCII.
    #[test]
    fn proxy_unresponsive_display() {
        let copy = Error::ProxyUnresponsive.to_string();
        assert_eq!(
            copy,
            "the atuin pty-proxy socket accepted the connection but did not answer in time \
             -- wait a moment and retry; if it persists, check what is holding the socket \
             with `lsof $ATUIN_PTY_PROXY_SOCKET`"
        );
        assert!(copy.is_ascii());
        assert!(!copy.contains("new shell"));
        assert!(
            !copy.contains("another client"),
            "the copy must not assert a cause it cannot know"
        );
    }

    /// The exact boundary the panic sweep measured, table-driven so the two
    /// floors can never drift apart silently. `cols` is refused at 0 (vt100
    /// `row.rs`), and the CHILD rows — physical minus the bar row — are
    /// refused below 2 (vt100 `grid.rs`), which puts the physical floor at 3.
    #[test]
    fn host_size_refuses_exactly_the_geometries_that_panic() {
        // (cols, rows) observed to panic, so they must be refused.
        for (cols, rows) in [(0, 0), (0, 36), (0, 1), (80, 0), (80, 1), (80, 2), (1, 1)] {
            assert!(
                host_size_from(cols, rows).is_err(),
                "{cols}x{rows} panics vt100 and must be refused"
            );
        }
        // ...and the geometries observed to be clean, with the bar row gone.
        for (cols, rows, want) in [
            (80, 3, Size { cols: 80, rows: 2 }),
            (80, 4, Size { cols: 80, rows: 3 }),
            (1, 24, Size { cols: 1, rows: 23 }),
            (2, 24, Size { cols: 2, rows: 23 }),
            (
                200,
                60,
                Size {
                    cols: 200,
                    rows: 59,
                },
            ),
        ] {
            assert_eq!(
                host_size_from(cols, rows).expect("{cols}x{rows} is a survivable size"),
                want,
                "{cols}x{rows}"
            );
        }
    }

    /// The refusal quotes back the size the user can see (the real terminal),
    /// not the bar-adjusted child size.
    #[test]
    fn host_size_refusal_reports_the_measured_size() {
        let Err(Error::TerminalTooSmall { cols, rows }) = host_size_from(0, 0) else {
            panic!("a 0x0 winsize must be refused");
        };
        assert_eq!((cols, rows), (0, 0));
    }

    /// SIGWINCH clamps where startup refuses, and the clamp lands on the
    /// floor rather than on the panicking 1x1: a host who shrinks their
    /// window to nothing gets an unreadable session, never a dead one.
    #[test]
    fn host_window_clamps_instead_of_refusing() {
        assert_eq!(clamp_host_size(0, 0), Size { cols: 1, rows: 2 });
        assert_eq!(clamp_host_size(80, 2), Size { cols: 80, rows: 2 });
        assert_eq!(clamp_host_size(80, 1), Size { cols: 80, rows: 2 });
        // Above the floor the bar row is still subtracted exactly once.
        assert_eq!(clamp_host_size(80, 24), Size { cols: 80, rows: 23 });
    }

    /// Error display strings are frozen once shipped. This one has to name
    /// the real minimum and the observed size, and stay pure ASCII.
    #[test]
    fn terminal_too_small_display() {
        let copy = Error::TerminalTooSmall { cols: 0, rows: 0 }.to_string();
        assert_eq!(
            copy,
            "terminal is too small to share: 0x0 -- atuin lab share needs at least 1x3 \
             (2 rows for the shell plus 1 reserved for the warning bar) -- resize the \
             window, or if this is a non-interactive invocation such as \
             `script -q /dev/null ...`, give it a real terminal size"
        );
        assert!(copy.is_ascii());
    }

    #[test]
    fn daemonize_display() {
        assert_eq!(
            Error::Daemonize("boom".into()).to_string(),
            "failed to daemonize process: boom"
        );
    }

    /// The `--active` refusal is a first-class UX surface (the feasibility
    /// report's section-4 copy, minus the `error:` prefix the CLI reporter
    /// adds): byte-pin it, and keep it pure ASCII.
    #[test]
    fn active_refusal_copy() {
        let copy = Error::ActiveShareUnsupported.to_string();
        assert_eq!(
            copy,
            "--active can only share a session whose terminal atuin already controls.\n\
             \n\
             This shell is not running under atuin pty-proxy (ATUIN_PTY_PROXY_SOCKET is not\n\
             set) or tmux. No tool can retroactively tap a terminal it does not own -- this\n\
             is an operating-system boundary, not a missing atuin feature.\n\
             \n\
             To share right now:\n\
             \x20   atuin lab share        # starts a shareable subshell; type exit when done\n\
             \n\
             To make --active work for future sessions, add to your ~/.zshrc:\n\
             \x20   eval \"$(atuin pty-proxy init zsh)\""
        );
        assert!(copy.is_ascii());
    }
}
