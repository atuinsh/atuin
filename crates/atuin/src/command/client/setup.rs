use atuin_client::settings::Settings;

use colored::Colorize;
use eyre::Result;
use std::io::{self, Write};
use toml_edit::{DocumentMut, value};

pub async fn run(_settings: &Settings) -> Result<()> {
    let enable_ai = prompt(
        "Atuin AI",
        "This will enable command generation and other AI features via the question mark key",
    )?;

    let enable_daemon = prompt(
        "Atuin Daemon",
        "This will enable improved search and history sync using a persistent background process",
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

        println!(
            "{check} Settings updated successfully",
            check = "✓".bold().bright_green()
        );
    } else {
        println!(
            "{check} No settings changed",
            check = "✓".bold().bright_green()
        );
    }

    Ok(())
}

pub fn prompt(feature: &str, description: &str) -> Result<bool> {
    println!(
        "> Enable {feature}?",
        feature = feature.bold().bright_blue()
    );
    print!("  {description} {q} ", q = "[Y/n]".bold());
    io::stdout().flush().ok();

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();
    Ok(answer.is_empty() || answer == "y" || answer == "yes")
}
