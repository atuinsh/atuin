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

/// Entry point for `atuin lab share`.
///
/// Placeholder: the subshell, session loop and hub transport are wired up in
/// later tasks of this plan. The command surface exists and is buildable now.
#[cfg(unix)]
pub fn run_share(_opts: ShareOptions) -> eyre::Result<()> {
    Err(eyre::eyre!("atuin lab share is not implemented yet"))
}

/// `atuin lab share` is unix-only for now (it needs a PTY).
#[cfg(not(unix))]
pub fn run_share(_opts: ShareOptions) -> eyre::Result<()> {
    Err(eyre::eyre!(
        "atuin lab share currently supports unix platforms only"
    ))
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
