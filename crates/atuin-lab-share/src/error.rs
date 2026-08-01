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
