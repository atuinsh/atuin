use atuin_client::settings::Settings;
use atuin_common::cli_reference::CommandReference;
use clap::CommandFactory;
use eyre::Result;
use serde::Serialize;

/// What the docs site's reference is rendered from: the command tree and `config.toml`'s schema.
#[derive(Serialize)]
struct Reference {
    cli: CommandReference,
    config: serde_json::Value,
}

pub fn run() -> Result<()> {
    let reference = Reference {
        cli: CommandReference::new(crate::Atuin::command()),
        config: Settings::config_schema()?,
    };
    serde_json::to_writer_pretty(std::io::stdout().lock(), &reference)?;
    Ok(())
}
