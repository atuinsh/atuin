use std::path::PathBuf;

use clap::Subcommand;
use eyre::{eyre, Result};

use atuin_run::{markdown::parse, pty::run_pty};
use rustix::path::Arg;

#[derive(Debug, Subcommand)]
pub enum Cmd {
    Markdown { path: String },
}

impl Cmd {
    pub async fn run(&self) -> Result<()> {
        match self {
            Cmd::Markdown { path } => {
                let file = PathBuf::from(path);

                if !file.exists() {
                    return Err(eyre!("File does not exist at {path}"));
                }

                let md = tokio::fs::read_to_string(file).await?;
                let blocks = parse(md.as_str());
                run_pty(blocks).await?;
            }
        }

        Ok(())
    }
}
