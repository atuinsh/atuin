use std::io::{self, Write};
use std::path::PathBuf;

use atuin_client::{
    database::{Database, Sqlite},
    settings::Settings,
};
use colored::Colorize;
use eyre::Result;
use toml_edit::{DocumentMut, value};

use super::import;

pub async fn run(settings: &Settings) -> Result<()> {
    // Step 1: Offer to import shell history if the user has very few commands.
    // Sqlite::new handles creating the DB and running migrations if it doesn't exist yet.
    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = Sqlite::new(db_path, settings.local_timeout).await?;
    let history_count = db.history_count(false).await.unwrap_or(0);

    if history_count < 10 {
        let do_import = prompt(
            "Import shell history",
            "You have fewer than 10 commands in Atuin. Import your existing shell history?",
        )?;

        if do_import {
            println!();
            if let Err(e) = import::Cmd::Auto.run(&db).await {
                println!(
                    "{warn} History import failed: {e}",
                    warn = "!".bold().bright_yellow()
                );
                println!("  You can retry later with 'atuin import auto'.");
            }
            println!();
        }
    }

    // Step 2: Offer to sign up for the hub if not already logged in
    let logged_in = atuin_client::hub::is_logged_in().await.unwrap_or(false);
    if !logged_in {
        println!();
        println!("  {title}", title = "Atuin Hub".bold().bright_blue());
        println!("  Sync your history across all your machines:");
        println!("    - End-to-end encrypted — only you can read your data");
        println!("    - Access your history from any device");
        println!("    - Never lose your history, even if you wipe a machine");
        println!();

        let do_signup = prompt("Sign up for Atuin Hub", "Create a free sync account?")?;

        if do_signup {
            let hub_address = settings
                .active_hub_endpoint()
                .unwrap_or_else(|| Settings::DEFAULT_HUB_ENDPOINT.to_string());

            match hub_signup(&hub_address).await {
                Ok(()) => {
                    println!(
                        "{check} Successfully authenticated with Atuin Hub!",
                        check = "✓".bold().bright_green()
                    );
                }
                Err(e) => {
                    println!(
                        "{warn} Hub sign up did not complete: {e}",
                        warn = "!".bold().bright_yellow()
                    );
                    println!("  You can sign up later with 'atuin login'.");
                }
            }
            println!();
        }
    }

    // Step 3: Configure features (AI, Daemon)
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

async fn hub_signup(hub_address: &str) -> Result<()> {
    let session = atuin_client::hub::HubAuthSession::start(hub_address).await?;

    println!();
    println!("  Open this URL to sign up:");
    println!("  {url}", url = session.auth_url.bold().underline());
    println!();
    println!("  Waiting for authentication...");

    let token = session
        .wait_for_completion(
            atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
            atuin_client::hub::DEFAULT_POLL_INTERVAL,
        )
        .await?;

    atuin_client::hub::save_session(&token).await?;

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
