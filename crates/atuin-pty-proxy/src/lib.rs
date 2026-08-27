#[cfg(all(unix, feature = "server"))]
mod capture;
#[cfg(all(unix, feature = "server"))]
mod debug;
#[cfg(all(unix, any(feature = "client", feature = "server")))]
mod ipc;
#[cfg(all(unix, feature = "server"))]
mod osc133;
#[cfg(all(unix, feature = "server"))]
mod pty_proxy;
#[cfg(all(unix, feature = "server"))]
mod runtime;
#[cfg(all(unix, any(feature = "client", feature = "server")))]
mod screen;

#[cfg(all(unix, feature = "server"))]
pub use capture::{CommandCapture, CommandCaptureSink};
#[cfg(all(unix, feature = "client"))]
pub use ipc::{IpcClient, IpcConnection, IpcError};
#[cfg(all(unix, feature = "server"))]
pub use pty_proxy::{PtyProxy, Shell, init_script};
#[cfg(all(unix, any(feature = "client", feature = "server")))]
pub use screen::ScreenSnapshot;

#[cfg(all(not(unix), feature = "server"))]
#[allow(dead_code)]
mod unsupported {
    use clap::{Args, Subcommand};

    #[derive(Args, Debug)]
    pub struct PtyProxy {
        /// Highlight OSC 133 prompt, input, output, and exit-code regions
        #[arg(long)]
        debug_osc133: bool,

        /// Path to the shell binary that atuin pty-proxy should spawn.
        /// Defaults to the system login shell. Only valid when no subcommand is given.
        #[arg(long, value_name = "PATH")]
        shell: Option<std::path::PathBuf>,

        #[command(subcommand)]
        cmd: Option<Cmd>,
    }

    #[derive(Subcommand, Debug)]
    enum Cmd {
        /// Print shell code to initialize atuin pty-proxy on shell startup
        Init(Init),
    }

    #[derive(Args, Debug)]
    struct Init {
        /// Shell to generate init for. If omitted, attempt auto-detection
        shell: Option<String>,
    }
}

#[cfg(not(unix))]
pub use unsupported::PtyProxy;
