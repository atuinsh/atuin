//! Crate-level error type. Unconditional (no `cfg(unix)`): the non-unix
//! `run_share` stub reports [`Error::UnsupportedPlatform`], so this module must
//! compile everywhere.
//!
//! Every variant's `#[error]` string is byte-identical to the message the crate
//! displayed before it had a typed error, so nothing user-visible changes. The
//! sole consumer (`lab.rs`) converts to `eyre::Report` via the blanket
//! `From<E: std::error::Error>`.
//!
//! Two variants sketched during the refactor are deliberately absent, because
//! both would be dead code:
//!
//! * `Wait` — the pre-typed session never surfaced a failed `wait`/`try_wait`
//!   as an error; both paths mapped it to an exit code
//!   (`subshell.wait().unwrap_or(0)`, `Err(_) => break 0`), and the session
//!   preserves exactly that mapping.
//! * `SessionFailed` ("share session panicked: …") — the session future is now
//!   awaited directly instead of via a `spawn_blocking` wrapper, so a panic
//!   inside it unwinds straight through `run_share` (the terminal guard still
//!   restores the tty on the way out) rather than being repackaged as an
//!   error.

use url::Url;

/// The `--active` refusal copy, verbatim from the feasibility evaluation
/// (minus its leading `error:` prefix, which the CLI's error reporter
/// supplies). This is a first-class UX surface, not a stub: it states the
/// OS truth, the works-right-now path, and the one-line rc change that makes
/// `--active` work for every future session. Byte-pinned by a test.
const ACTIVE_SHARE_REFUSAL: &str = r#"--active can only share a session whose terminal atuin already controls.

This shell is not running under atuin pty-proxy (ATUIN_PTY_PROXY_SOCKET is not
set) or tmux. No tool can retroactively tap a terminal it does not own -- this
is an operating-system boundary, not a missing atuin feature.

To share right now:
    atuin lab share        # starts a shareable subshell; type exit when done

To make --active work for future sessions, add to your ~/.zshrc:
    eval "$(atuin pty-proxy init zsh)""#;

/// Everything that can go wrong in `atuin lab share`.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// stdin or stdout is not a TTY; raw mode and the PTY session need both.
    #[error("atuin lab share must run in an interactive terminal")]
    NotATerminal,

    /// The first hub connection did not complete within the connect timeout.
    #[error(
        "couldn't reach the hub at {url} within {timeout_secs}s -- is it running, and is \
         ATUIN_LAB_HUB_URL correct?"
    )]
    HubUnreachable { url: Url, timeout_secs: u64 },

    /// The transport task ended (dropped its oneshot) before delivering the
    /// first join URL.
    #[error("the hub transport stopped before connecting")]
    TransportStopped,

    /// `openpty` failed. Holds `portable-pty`'s error rendered with `{:#}`
    /// (anyhow's alternate, cause-chain form) so the displayed text is
    /// unchanged from the pre-`thiserror` messages.
    #[error("openpty: {0}")]
    OpenPty(String),

    /// Spawning the user's shell on the PTY slave failed. Same `{:#}`
    /// stringification as [`Error::OpenPty`].
    #[error("spawn: {0}")]
    SpawnShell(String),

    /// The confirmation prompt needs an interactive stdin to read the answer
    /// from, and `--yes` was not passed. Raised before anything touches the
    /// network, so declining or failing here leaks nothing.
    #[error("cannot confirm on a non-interactive stdin; pass --yes to share without a prompt")]
    ConfirmationRequired,

    /// A second `--active` share was asked to start while another's pidfile
    /// lock is held: at most one active share per user. The lock — not the
    /// pidfile's existence — is the aliveness signal; see the `lifecycle`
    /// module.
    #[error("an active share is already running")]
    ShareAlreadyRunning,

    /// Forking the `--internal-daemon` child into the background failed.
    /// Holds the `daemonize` crate's error as a string, matching the
    /// [`Error::OpenPty`]/[`Error::SpawnShell`] pattern of not re-exporting
    /// foreign error types.
    #[error("failed to daemonize process: {0}")]
    Daemonize(String),

    /// The `--active` detection ladder found no cooperating PTY owner to
    /// attach to: `ATUIN_PTY_PROXY_SOCKET` is unset, or nothing answers on
    /// it. `--active` never spawns anything, so this is a refusal — the
    /// display is `ACTIVE_SHARE_REFUSAL`, the full instructive copy.
    #[error("{}", ACTIVE_SHARE_REFUSAL)]
    ActiveShareUnsupported,

    /// The proxy behind `ATUIN_PTY_PROXY_SOCKET` predates the subscriber
    /// protocol: its reply to the attach greeting was not a Hello frame (an
    /// old proxy writes its one-shot snapshot blob and closes). The proxy's
    /// version is fixed at shell startup, so the fix is a new session.
    #[error(
        "the running atuin pty-proxy is too old to attach to -- start a new shell session \
         (with the current atuin) and run --active there"
    )]
    ProxyTooOld,

    /// Something accepted the connection on `$ATUIN_PTY_PROXY_SOCKET` and then
    /// went silent: no reply to the attach handshake within
    /// `proxy_tap::HANDSHAKE_TIMEOUT`. Distinct from [`Error::ProxyTooOld`]
    /// because the condition is *transient*: the remedy is to retry, never to
    /// abandon the shell session.
    ///
    /// The copy deliberately does **not** name a cause. "Busy serving another
    /// client" stopped being true when the proxy moved every connection onto
    /// its own thread (a stalled same-uid client can no longer make it silent
    /// to a new connection — pinned by the proxy's
    /// `a_stalled_client_does_not_wedge_the_socket_server`), and the causes
    /// that remain — a non-atuin process squatting on the socket path, a
    /// saturated parser queue, a proxy wedged inside vt100 — are not "another
    /// client" either. So it reports what is actually known (accepted, did not
    /// answer), keeps the retry advice, and points `lsof` at *whatever* holds
    /// the socket rather than at a client that may not exist.
    #[error(
        "the atuin pty-proxy socket accepted the connection but did not answer in time -- \
         wait a moment and retry; if it persists, check what is holding the socket with \
         `lsof $ATUIN_PTY_PROXY_SOCKET`"
    )]
    ProxyUnresponsive,

    /// A current proxy misbehaved mid-attach: it closed before the first
    /// keyframe, or sent a frame that does not decode.
    #[error("the pty-proxy sent an invalid reply while attaching: {0}")]
    ProxyHandshake(String),

    /// The terminal behind the pty-proxy is too small to share: its attach
    /// keyframe reported a geometry below the `vt100` floor (the proxy's own
    /// clamp is `(1, 1)`, and a one-row grid panics the session's model).
    ///
    /// The `--active` counterpart of [`Error::TerminalTooSmall`], and raised
    /// at the same point in the flow: inside `ProxyTap::attach`, before the
    /// session key is minted and before the hub hears from us, so a refusal
    /// here leaves nothing minted anywhere. Separate from that variant
    /// because the tap feeds a **headless** session — no warning bar, so no
    /// row is reserved for one and the minimum is one row lower.
    ///
    /// `cols`/`rows` are what the proxy reported, so the message quotes back
    /// the size of the terminal the user is looking at.
    #[error(
        "the terminal behind the atuin pty-proxy is too small to share: {cols}x{rows} -- \
         atuin lab share needs at least {}x{} -- resize that terminal and run \
         `atuin lab share --active` again",
        crate::MIN_COLS,
        crate::MIN_CHILD_ROWS
    )]
    ProxyTerminalTooSmall { cols: u16, rows: u16 },

    /// `--active --write` asked the proxy for input access and was refused:
    /// the token we sent did not match the proxy's token file.
    #[error(
        "the pty-proxy denied write access -- the token next to ATUIN_PTY_PROXY_SOCKET did \
         not match; share read-only (drop --write) or start a new shell session"
    )]
    ProxyInputDenied,

    /// The proxy's input token (the `token` sibling of the socket) could not
    /// be read. Only raised under `--write`; read-only attaches never touch
    /// the token file.
    #[error("cannot read the pty-proxy token at {path}: {source}")]
    ProxyToken {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    /// The host's terminal is too small for the session to render at all:
    /// `vt100` 0.16.2 panics on a zero-column grid and on a one-row child
    /// grid (see `MIN_COLS` / `MIN_CHILD_ROWS` for the two
    /// panic sites). Raised by `read_host_size`, which runs before the
    /// session key is minted and before the hub hears from us — so a refusal
    /// here leaves nothing minted anywhere.
    ///
    /// `cols`/`rows` are the terminal's real size as measured, not the
    /// bar-adjusted child size, so the message quotes back what the user can
    /// see. A `0x0` here is almost always a pty-less CI-shaped invocation
    /// rather than a genuinely tiny window.
    #[error(
        "terminal is too small to share: {cols}x{rows} -- atuin lab share needs at least \
         {}x{} ({} rows for the shell plus 1 reserved for the warning bar) -- resize the \
         window, or if this is a non-interactive invocation such as \
         `script -q /dev/null ...`, give it a real terminal size",
        crate::MIN_COLS,
        crate::MIN_CHILD_ROWS + 1,
        crate::MIN_CHILD_ROWS,
    )]
    TerminalTooSmall { cols: u16, rows: u16 },

    /// `atuin lab share` is unix-only for now (it needs a PTY).
    #[error("atuin lab share currently supports unix platforms only")]
    UnsupportedPlatform,

    /// Terminal I/O and session startup: crossterm's
    /// `size()`/`enable_raw_mode()`, installing the SIGWINCH listener, and
    /// spawning the session's bridge threads all surface `std::io::Error`.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Crate-wide result alias; the default error type is [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
