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

#[cfg(unix)]
use std::io::{IsTerminal as _, Write as _};

#[cfg(unix)]
mod backpressure;
#[cfg(unix)]
mod keyframe;
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

/// Child-shell terminal dimensions, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

/// Options for a share session. The caller resolves `hub_url` and `api_token`
/// (both come from `Settings`, and the token accessor is `async`) so that
/// `run_share` needs no tokio runtime of its own.
#[derive(Debug, Clone)]
pub struct ShareOptions {
    pub write: bool,
    pub hub_url: String,
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

#[cfg(unix)]
impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[r\x1b[?6l\x1b[?25h\x1b[0m\r\n");
        let _ = out.flush();
    }
}

/// Entry point for `atuin lab share`. Spawns the subshell, connects to the
/// hub, and runs the session until the shell exits or the host presses Ctrl-\.
///
/// Sync and runtime-free by design: the caller (`lab::Cmd::run`) has already
/// awaited the async settings accessors, because this runs inside an existing
/// tokio runtime and creating a nested one would panic.
///
/// # Errors
///
/// Returns an error if stdin/stdout are not a terminal, if the host terminal
/// size cannot be read, if the subshell cannot be spawned, or if the session
/// loop fails to start.
#[cfg(unix)]
pub fn run_share(opts: ShareOptions) -> eyre::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(eyre::eyre!(
            "atuin lab share must run in an interactive terminal"
        ));
    }

    // One-time warning (spec §8): output and typed secrets are visible.
    eprintln!(
        "⚠  Terminal sharing is experimental. Everything shown here — including \
         secrets you type — is visible to anyone with the link.{}",
        if opts.write {
            " WRITE MODE: they can run commands on your machine."
        } else {
            ""
        }
    );

    // The bar row is subtracted exactly ONCE, here at the source. `host_size` is
    // what the hub negotiates against, and the `set_size` it returns is applied
    // to the child PTY directly — never subtract the bar row a second time.
    let (cols, rows) = crossterm::terminal::size()?;
    let host_size = Size {
        cols,
        rows: rows.saturating_sub(1).max(1),
    };

    let sh = subshell::Subshell::spawn(host_size)?;

    let (out_tx, out_rx) = std::sync::mpsc::channel::<session::Outbound>();
    let (in_tx, in_rx) = std::sync::mpsc::channel::<session::Inbound>();

    transport::spawn_transport(opts.hub_url, opts.api_token, opts.write, out_rx, in_tx);

    // Raw mode is enabled here (not in `Session::run`) so the session stays
    // unit-testable without touching the test runner's terminal. The guard
    // restores everything when it drops, on any exit path.
    crossterm::terminal::enable_raw_mode()?;
    let guard = TermGuard;

    let stdin: Box<dyn std::io::Read + Send> = Box::new(std::io::stdin());
    let stdout: Box<dyn std::io::Write + Send> = Box::new(std::io::stdout());
    let code = session::Session::run(sh, host_size, opts.write, out_tx, in_rx, stdin, stdout)?;

    // Explicit: `std::process::exit` below does not run destructors.
    drop(guard);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// `atuin lab share` is unix-only for now (it needs a PTY).
#[cfg(not(unix))]
pub fn run_share(_opts: ShareOptions) -> eyre::Result<()> {
    Err(eyre::eyre!(
        "atuin lab share currently supports unix platforms only"
    ))
}

#[cfg(all(test, unix))]
mod run_tests {
    use super::*;

    #[test]
    fn refuses_when_not_a_tty() {
        // In `cargo nextest`, stdin is not a terminal. This guard is also what
        // keeps the test suite from ever enabling raw mode on the developer's
        // real terminal.
        let err = run_share(ShareOptions {
            write: false,
            hub_url: "wss://x".into(),
            api_token: "t".into(),
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("terminal"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_is_copy_and_eq() {
        let a = Size { cols: 80, rows: 24 };
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn share_options_round_trip() {
        let o = ShareOptions {
            write: true,
            hub_url: "wss://h".into(),
            api_token: "tok".into(),
        };
        assert!(o.write);
        assert_eq!(o.hub_url, "wss://h");
        assert_eq!(o.api_token, "tok");
    }
}
