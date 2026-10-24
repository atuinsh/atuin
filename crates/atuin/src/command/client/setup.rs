use atuin_client::settings::Settings;
use atuin_common::logs::USER;
use atuin_common::prompt::{DefaultAnswer, Prompt};
use colored::Colorize;
use eyre::Result;
use toml_edit::{DocumentMut, value};
use tracing::{info, instrument};

#[instrument(level = "trace", skip_all, err)]
pub async fn run(_settings: &Settings) -> Result<()> {
    let enable_ai = prompt(
        "Atuin AI",
        "This will enable command generation and other AI features via the question mark key",
        Some(
            "By default, Atuin AI only has access to the name and version of your operating \
             system and shell - your shell history is not sent to the AI.",
        ),
        DefaultAnswer::Yes,
    )?;

    let enable_daemon = prompt(
        "Atuin Daemon",
        "This will enable improved search and history sync using a persistent background process",
        None,
        DefaultAnswer::Yes,
    )?;

    let config_file = Settings::get_config_path()?;
    let config_str = tokio::fs::read_to_string(&config_file).await?;
    let mut doc = config_str.parse::<DocumentMut>()?;

    let mut changed = false;
    if enable_ai {
        changed = true;
        if !doc.contains_key("ai") {
            doc["ai"] = toml_edit::table();
        }
        doc["ai"]["enabled"] = value(true);
    }

    if enable_daemon {
        changed = true;
        if !doc.contains_key("daemon") {
            doc["daemon"] = toml_edit::table();
        }
        doc["daemon"]["enabled"] = value(true);
        doc["daemon"]["autostart"] = value(true);
        doc["search_mode"] = value("daemon-fuzzy");
    }

    if changed {
        tokio::fs::write(config_file, doc.to_string()).await?;

        info!(target: USER, "Settings updated successfully");
    } else {
        info!(target: USER, "No settings changed");
    }

    Ok(())
}

pub fn prompt(
    feature: &str,
    description: &str,
    note: Option<&str>,
    default: DefaultAnswer,
) -> Result<bool> {
    let feature = feature.bold().bright_blue();
    let note = note.map(|note| format!("\n  {note}")).unwrap_or_default();
    let question = format!("> Enable {feature}?\n  {description}{note}");
    Ok(Prompt::new(&question).confirm(default)?)
}
