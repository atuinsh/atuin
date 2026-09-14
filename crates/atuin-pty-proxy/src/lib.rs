#[cfg(unix)]
mod capture;
#[cfg(unix)]
mod cwd_updater;
#[cfg(unix)]
mod debug;
#[cfg(unix)]
mod osc133;
#[cfg(unix)]
mod pty_proxy;
#[cfg(unix)]
mod runtime;
#[cfg(unix)]
mod screen;

#[cfg(unix)]
pub use capture::{CaptureConfig, CommandCapture, CommandCaptureSink};
#[cfg(unix)]
pub use pty_proxy::{PtyProxy, Shell, init_script};
#[cfg(unix)]
pub use screen::{is_pty_proxy_child, parent_socket_path};

/// Drive the OSC 133 parser over `data`, returning the number of markers found.
///
/// Exposed only so benchmarks can exercise the parser directly; not part of the stable public API.
#[cfg(unix)]
#[doc(hidden)]
#[must_use]
pub fn bench_osc133_parse(data: &[u8]) -> usize {
    let mut parser = osc133::Parser::new();
    let mut chunks = parser.push(data);
    let count = chunks.by_ref().count();
    std::hint::black_box(chunks.trailing_data());
    count
}

#[cfg(not(unix))]
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
