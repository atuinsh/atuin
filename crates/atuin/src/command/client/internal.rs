//! Internal subcommands, not for direct use by users.

use std::sync::Arc;

use atuin_client::settings::Settings;
use atuin_common::logs::LogConfig;
use atuin_common::os;

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
    /// Print the process ID, a NUL byte, and the path of the current running executable, without a
    /// trailing newline.
    ///
    /// On Unix, the path prints as its bytes, which need not be UTF-8. On Windows, a path that isn't
    /// valid Unicode is an error. The process id shows whether a shim that ran this command
    /// replaced itself with atuin, since the process keeps its id across `exec`.
    ///
    /// [`launch_path`](atuin_common::atuin_exe::launch_path) uses this command.
    PrintExe,
}

impl Cmd {
    /// Run this command.
    ///
    /// If the command was able to be handled synchronously, this method returns `Ok(None)`.
    /// Otherwise, it returns an async function that, when called, will run the command.
    ///
    /// # Errors
    ///
    /// If a command that is handled synchronously fails.
    pub fn run(
        &self,
    ) -> eyre::Result<Option<impl AsyncFnOnce(Settings) -> eyre::Result<()> + use<>>> {
        match self {
            Self::PrepareSearchIndex => {
                Ok(Some(async |settings| super::search::prepare_index(&Arc::new(settings)).await))
            }
            Self::PtyProxyActive => {
                println!("{}", pty_proxy_answer());
                Ok(None)
            }
            Self::PrintExe => {
                print_exe()?;
                Ok(None)
            }
        }
    }

    pub fn log_config(&self) -> Option<LogConfig> {
        match self {
            // These commands are called from the shell hooks with no stderr; there's no point in
            // initializing logging. Also, commands that are handled synchronously
            // (pty-proxy-active, print-exe) are run before logging is even initialized.
            Self::PrepareSearchIndex | Self::PtyProxyActive | Self::PrintExe => None,
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

/// Write the process ID and the path of the current running executable to stdout, for
/// [`Cmd::PrintExe`].
fn print_exe() -> std::io::Result<()> {
    use std::io::Write as _;

    let exe = os::process::current_exe()?;
    #[cfg(unix)]
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(exe.as_os_str());
    #[cfg(not(unix))]
    let bytes = exe
        .to_str()
        .ok_or_else(|| std::io::Error::other(format!("{} is not valid Unicode", exe.display())))?
        .as_bytes();

    let mut stdout = std::io::stdout().lock();
    write!(stdout, "{}\0", std::process::id())?;
    stdout.write_all(bytes)?;
    stdout.flush()
}
