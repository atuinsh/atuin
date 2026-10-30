use std::io::{self, Write as _};
use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_common::harnesstools::{AnyHarness, Harness, mcp};
use atuin_common::userio;
use clap::{Parser, Subcommand};
use eyre::{Result, bail};

use crate::i18n::fl;

#[derive(Subcommand, Debug)]
enum Action {
    #[command(about = fl!("cmd-mcp-install"))]
    Install {
        #[arg(value_name = "AGENT", help = fl!("arg-mcp-install-agent"))]
        agent: Option<String>,
    },
}

/// `atuin mcp` serves over stdio; `atuin mcp install [agent]` registers it.
#[derive(Parser, Debug)]
pub struct Cmd {
    #[command(subcommand)]
    action: Option<Action>,
}

impl Cmd {
    pub async fn run(self, db: &Sqlite, settings: Settings) -> Result<()> {
        match self.action {
            None => Box::pin(atuin_ai::mcp::run(db, Arc::new(settings))).await,
            Some(Action::Install { agent: Some(agent) }) => {
                install(AnyHarness::from_name(&agent)?).await?;
                print_capture_tip();
                Ok(())
            }
            Some(Action::Install { agent: None }) => install_all().await,
        }
    }
}

async fn install_all() -> Result<()> {
    let found: Vec<_> =
        AnyHarness::all().iter().copied().filter(|&harness| mcp::config_exists(harness)).collect();
    if found.is_empty() {
        eprintln!(
            "No supported agent config files found. Use `atuin mcp install <agent>` to connect an \
             agent explicitly."
        );
        return Ok(());
    }

    let names: Vec<_> = found.iter().map(Harness::name).collect();
    println!("This will connect atuin's MCP server to: {}", names.join(", "));
    println!("Tip: connect a single agent with `atuin mcp install <agent>`.");
    print!("Continue? [Y/n] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    // End of input declines, so Ctrl-D or a closed stdin installs nothing.
    if io::stdin().read_line(&mut answer)? == 0
        || !matches!(answer.trim().to_lowercase().as_str(), "" | "y" | "yes")
    {
        return Ok(());
    }

    let mut failed = Vec::new();
    for harness in found {
        if let Err(err) = install(harness).await {
            eprintln!("Could not install for {}: {err:#}", harness.name());
            failed.push(harness.name());
        }
    }
    print_capture_tip();
    if !failed.is_empty() {
        bail!("MCP installation failed for: {}", failed.join(", "));
    }
    Ok(())
}

async fn install(harness: AnyHarness) -> Result<()> {
    mcp::install(harness).await?;
    userio::log::info!("mcp-restart-harness", harness = harness.name());
    Ok(())
}

fn print_capture_tip() {
    userio::log::info!("mcp-capture-tip");
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::all_agents(None)]
    #[case::explicit(Some("codex"))]
    fn install_accepts_an_optional_agent(#[case] agent: Option<&str>) {
        let mut args = vec!["mcp", "install"];
        args.extend(agent);
        let cmd = Cmd::try_parse_from(args).unwrap();
        let Some(Action::Install { agent: parsed }) = cmd.action else {
            panic!("expected install");
        };
        assert_eq!(parsed.as_deref(), agent);
    }

    #[rstest]
    fn bare_mcp_still_serves() {
        assert!(Cmd::try_parse_from(["mcp"]).unwrap().action.is_none());
    }
}
