#![deny(unsafe_code)]

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
//! Everything below [`run_share`] is crate-private; the modules split along the
//! session's data flow:
//!
//! * `subshell` — the child shell on its PTY (`portable-pty`), split into the
//!   parts the session's task topology needs.
//! * `session` — the heart of the crate: one central `select!` task owning all
//!   session state, plus the four bridged threads covering the blocking edges
//!   (PTY read and write, raw-mode stdin, terminal write). `session::screen`
//!   fuses the `vt100` model with the `seq` counter and the keyframe cadence.
//! * `render` — what the host sees: the warning bar, the compositor, and the
//!   keyframe bytes viewers replay.
//! * `query` — synthetic answers to the terminal probes (CPR / DA) that the
//!   compositing model would otherwise swallow.
//! * `transport` — the hub client: Phoenix channel over WebSocket, reconnect
//!   with backoff, session resume via the secret host token.
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
mod error;
#[cfg(unix)]
mod protocol;
#[cfg(unix)]
mod query;
#[cfg(unix)]
mod render;
#[cfg(unix)]
mod session;
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

/// Options for a share session. The caller resolves `hub_url` and `api_token`
/// (both come from `Settings`, and the token accessor is `async`) so that
/// `run_share` needs no tokio runtime of its own.
#[derive(Debug, Clone)]
pub struct ShareOptions {
    /// Allow viewers to type into the shared shell (the `--write` flag).
    pub write: bool,
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
/// # Errors
///
/// Returns an error if stdin/stdout are not a terminal, if the host terminal
/// size cannot be read, if the subshell cannot be spawned, or if the session
/// loop fails to start. See [`Error`] for the full set of failure modes.
#[cfg(unix)]
pub async fn run_share(opts: ShareOptions) -> Result<()> {
    check_terminal()?;

    // `--write` is a bool at the CLI boundary; convert it exactly once, here,
    // and pass the typed mode everywhere below.
    let write = render::WriteMode::from_flag(opts.write);

    // One-time warning (spec §8): output and typed secrets are visible.
    eprintln!(
        "⚠  Terminal sharing is experimental. Everything shown here — including \
         secrets you type — is visible to anyone with the link.{}",
        if write.is_write_enabled() {
            " WRITE MODE: they can run commands on your machine."
        } else {
            ""
        }
    );

    let host_size = read_host_size()?;

    // session -> transport: unbounded, so the session's `send` is synchronous
    // and never blocks its select loop.
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<session::Outbound>();
    // transport -> session: the same shape in the other direction.
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel::<session::Inbound>();

    let join_url = connect_to_hub(&opts, write, out_rx, in_tx).await?;
    let sh = spawn_subshell(host_size, &join_url, write)?;

    // Raw mode is enabled here (not in `Session::run`) so the session stays
    // unit-testable without touching the test runner's terminal. The guard
    // restores everything when it drops, on any exit path.
    crossterm::terminal::enable_raw_mode()?;
    let guard = TermGuard;

    let session = session::Session {
        subshell: sh,
        physical: host_size,
        write,
        out_tx,
        in_rx,
        stdin: Box::new(std::io::stdin()),
        stdout: Box::new(std::io::stdout()),
    };
    // The session is a future on this runtime; its blocking edges (PTY reads
    // and writes, stdin, terminal writes, the child wait) live on threads it
    // manages itself. The guard above restores the terminal on every path out
    // of the await — including an unwind.
    let code = session.run().await?;

    // Explicit: `std::process::exit` below does not run destructors.
    drop(guard);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
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

/// Measure phase: the host's terminal, with the warning-bar row reserved.
///
/// The bar row is subtracted exactly ONCE, here at the source. `host_size` is
/// what the hub negotiates against, and the `set_size` it returns is applied
/// to the child PTY directly — never subtract the bar row a second time.
/// (SIGWINCH re-measures in `session`, the other of the two subtraction sites.)
#[cfg(unix)]
fn read_host_size() -> Result<Size> {
    let (cols, rows) = crossterm::terminal::size()?;
    Ok(Size {
        cols,
        rows: rows.saturating_sub(1).max(1),
    })
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
/// The transport is a task on our runtime; the session is awaited directly by
/// `run_share`. Neither needs a thread or runtime of its own.
#[cfg(unix)]
async fn connect_to_hub(
    opts: &ShareOptions,
    write: render::WriteMode,
    out_rx: tokio::sync::mpsc::UnboundedReceiver<session::Outbound>,
    in_tx: tokio::sync::mpsc::UnboundedSender<session::Inbound>,
) -> Result<String> {
    // How long to wait for the first hub connection before giving up. The
    // transport keeps retrying with backoff during this window.
    const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

    // transport -> here: the first join URL, delivered exactly once.
    let (url_tx, url_rx) = tokio::sync::oneshot::channel::<String>();

    // Install the rustls provider before the transport connects (process-global,
    // idempotent). The transport used to do this on its own thread; it now runs
    // on our runtime, so we do it here.
    atuin_common::tls::ensure_crypto_provider();

    eprintln!("Connecting to {} …", opts.hub_url);
    tokio::spawn(
        transport::Transport::new(opts.hub_url.clone(), opts.api_token.clone(), write, url_tx)
            .run(out_rx, in_tx),
    );

    match tokio::time::timeout(CONNECT_TIMEOUT, url_rx).await {
        Ok(Ok(url)) => Ok(url),
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
