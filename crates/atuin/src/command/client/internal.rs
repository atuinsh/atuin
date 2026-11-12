//! Internal subcommands, not for direct use by users.

use std::sync::Arc;

use atuin_client::settings::Settings;
use atuin_common::logs::LogConfig;

#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    PrepareSearchIndex,
    /// Check whether the current terminal belongs to a live PTY proxy.
    ///
    /// Prints `0` or `1` to stdout. This command is used by shell hooks to determine whether the
    /// PTY proxy is in use.
    ///
    /// Prints `-1` if the answer can't be determined, or if there is some reason that a proxy
    /// should not be spawned even if one isn't running (e.g., we're in a Linux virtual console,
    /// where spawning a proxy would interfere with commands that need direct console access).
    PtyProxyActive,
}

impl Cmd {
    /// Run this command.
    ///
    /// If the command was able to be handled synchronously, this method returns [`None`].
    /// Otherwise, it returns an async function that, when called, will run the command.
    pub fn run(&self) -> Option<impl AsyncFnOnce(Settings) -> eyre::Result<()> + use<>> {
        match self {
            Self::PrepareSearchIndex => {
                Some(async |settings| super::search::prepare_index(&Arc::new(settings)).await)
            }
            Self::PtyProxyActive => {
                println!("{}", pty_proxy_answer());
                None
            }
        }
    }

    pub fn log_config(&self) -> Option<LogConfig> {
        match self {
            // These commands are called from the shell hooks with no stderr; there's no point in
            // initializing logging. Also, commands that are handled synchronously
            // (pty-proxy-active) are run before logging is even initialized.
            Self::PrepareSearchIndex | Self::PtyProxyActive => None,
        }
    }
}

/// The value printed by [`Cmd::PtyProxyActive`]; see its documentation.
#[cfg(all(unix, feature = "pty-proxy"))]
fn pty_proxy_answer() -> i8 {
    use atuin_common::os::unix::tty::TtyId;

    if atuin_pty_proxy::is_pty_proxy_child() {
        1
    } else if TtyId::current().is_some_and(TtyId::is_virtual_console) {
        -1
    } else {
        0
    }
}

#[cfg(not(all(unix, feature = "pty-proxy")))]
fn pty_proxy_answer() -> i8 {
    -1
}
