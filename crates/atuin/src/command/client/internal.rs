//! Internal subcommands, not for direct use by users.

use atuin_client::settings::Settings;
use atuin_common::logs::LogConfig;

#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    PrepareSearchIndex,
    /// Check whether the current terminal belongs to a live PTY proxy.
    ///
    /// Prints `0` or `1` to stdout. This command is used by shell hooks to determine whether the
    /// PTY proxy is in use.
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
                Some(async |settings| super::search::prepare_index(&settings).await)
            }
            Self::PtyProxyActive => {
                #[cfg(all(unix, feature = "pty-proxy"))]
                let is_proxy_child = atuin_pty_proxy::is_pty_proxy_child();
                #[cfg(not(all(unix, feature = "pty-proxy")))]
                let is_proxy_child = false;

                println!("{}", u8::from(is_proxy_child));
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
