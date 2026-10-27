use std::io::{self, Write};

use atuin_client::settings::Settings;
use colored::Colorize;
use eyre::Result;
use toml_edit::{DocumentMut, value};
use tracing::instrument;

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

        println!("{check} Settings updated successfully", check = "✓".bold().bright_green());
    } else {
        println!("{check} No settings changed", check = "✓".bold().bright_green());
    }

    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub enum DefaultAnswer {
    Yes,
    #[cfg_attr(not(feature = "octavo"), expect(dead_code))]
    No,
}

pub fn prompt(
    feature: &str,
    description: &str,
    note: Option<&str>,
    default: DefaultAnswer,
) -> Result<bool> {
    let q = match default {
        DefaultAnswer::Yes => "[Y/n]",
        DefaultAnswer::No => "[y/N]",
    };

    println!("> Enable {feature}?", feature = feature.bold().bright_blue());
    if let Some(note) = note {
        println!("  {description}");
        print!("  {note} {q} ", q = q.bold());
    } else {
        print!("  {description} {q} ", q = q.bold());
    }

    io::stdout().flush().ok();

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(match input.trim().to_lowercase().as_str() {
        "" => matches!(default, DefaultAnswer::Yes),
        "y" | "yes" => true,
        _ => false,
    })
}
