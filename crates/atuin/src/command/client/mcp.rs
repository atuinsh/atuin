use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_common::harnesstools::{AnyHarness, Harness, mcp};
use clap::{Parser, Subcommand};
use eyre::Result;

use crate::i18n::fl;

#[derive(Subcommand, Debug)]
enum Action {
    #[command(about = fl!("cmd-mcp-install"))]
    Install {
        #[arg(value_name = "AGENT", help = fl!("arg-mcp-install-agent"))]
        agent: String,
    },
}

/// `atuin mcp` serves over stdio, as agents launch it; `atuin mcp install <agent>` registers it.
#[derive(Parser, Debug)]
pub struct Cmd {
    #[command(subcommand)]
    action: Option<Action>,
}

impl Cmd {
    pub async fn run(self, db: &Sqlite, settings: Settings) -> Result<()> {
        match self.action {
            None => Box::pin(atuin_ai::mcp::run(db, Arc::new(settings))).await,
            Some(Action::Install { agent }) => install(&agent).await,
        }
    }
}

async fn install(agent: &str) -> Result<()> {
    let harness = AnyHarness::from_name(agent)?;
    mcp::install(harness).await?;
    eprintln!("Restart {} to load it.", harness.name());
    eprintln!(
        "To let it search command output and agent sessions too, run `atuin config enable \
         output-capture` and `atuin config enable ai-sessions`."
    );
    Ok(())
}
