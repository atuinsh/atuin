use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "lower")]
#[allow(clippy::enum_variant_names, clippy::doc_markdown)]
pub enum Shell {
    /// Zsh setup
    Zsh,
    /// Bash setup
    Bash,
    /// Fish setup
    Fish,
    /// Nu setup
    Nu,
}

#[derive(Args, Debug)]
pub struct Init {
    /// Shell to generate init for. If omitted, attempt auto-detection
    #[arg(value_enum)]
    pub shell: Option<Shell>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Print shell code to initialize atuin pty-proxy on shell startup
    Init(Init),
}

#[derive(Args, Debug)]
pub struct PtyProxy {
    /// Highlight OSC 133 prompt, input, output, and exit-code regions
    #[arg(long)]
    pub debug_osc133: bool,

    /// Path to the shell binary that atuin pty-proxy should spawn.
    /// Defaults to the system login shell. Only valid when no subcommand is given.
    #[arg(long, value_name = "PATH")]
    pub shell: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}
