#[cfg(unix)]
mod capture;
#[cfg(unix)]
mod compositor;
#[cfg(unix)]
mod debug;
#[cfg(unix)]
mod oracle;
#[cfg(unix)]
mod osc133;
#[cfg(unix)]
pub mod protocol;
#[cfg(unix)]
mod pty_proxy;
#[cfg(unix)]
mod runtime;
#[cfg(unix)]
mod screen;
#[cfg(unix)]
mod suggest;

#[cfg(unix)]
pub use capture::{CommandCapture, CommandCaptureSink};
#[cfg(unix)]
pub use oracle::{Candidate, CompletionOracleHandle, OracleShell, OracleWarmer, find_in_path};
#[cfg(unix)]
pub use pty_proxy::{PtyProxy, RunOptions, Shell, init_script};
#[cfg(unix)]
pub use suggest::{Suggestion, SuggestionProvider, SuggestionSource, SyntaxClass, SyntaxSpan};

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
