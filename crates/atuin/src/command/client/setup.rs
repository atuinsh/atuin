use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
#[cfg(feature = "sync")]
use atuin_client::settings::SyncAuth;

use colored::Colorize;
use eyre::Result;
use std::io::{self, Write};
use toml_edit::{DocumentMut, value};

pub async fn run(settings: &Settings, store: &SqliteStore) -> Result<()> {
    #[cfg(feature = "sync")]
    setup_sync(settings, store).await?;

    #[cfg(not(feature = "sync"))]
    let _ = (settings, store);

    let enable_ai = prompt(
        "Atuin AI",
        "This will enable command generation and other AI features via the question mark key",
        Some(
            "By default, Atuin AI only has access to the name and version of your operating system and shell - your shell history is not sent to the AI.",
        ),
    )?;

    let enable_daemon = prompt(
        "Atuin Daemon",
        "This will enable improved search and history sync using a persistent background process",
        None,
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

#[cfg(feature = "sync")]
async fn setup_sync(settings: &Settings, store: &SqliteStore) -> Result<()> {
    if !matches!(
        settings.resolve_sync_auth().await,
        SyncAuth::NotLoggedIn { .. }
    ) {
        println!(
            "{check} Sync is already set up on this machine",
            check = "✓".bold().bright_green()
        );
        return Ok(());
    }

    println!("> Set up {sync}?", sync = "Sync".bold().bright_blue());
    println!("  Back up your shell history and sync it across all of your machines.");
    println!("  Everything is end-to-end encrypted - only you can read your history.");
    println!();
    println!("  Do you already have an Atuin account?");
    println!();
    println!("  {n}) No - create a new account", n = "1".bold());
    println!("  {n}) Yes - log in", n = "2".bold());
    println!("  {n}) Skip sync for now", n = "3".bold());
    println!();

    let choice = loop {
        print!("  Enter a number {q} ", q = "[1/2/3]".bold());
        io::stdout().flush().ok();

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        match input.trim() {
            "1" => break 1,
            "2" => break 2,
            "3" | "" => break 3,
            _ => println!("  Please enter 1, 2, or 3"),
        }
    };

    println!();

    match choice {
        1 => {
            super::account::register::Cmd {
                username: None,
                password: None,
                email: None,
            }
            .run(settings, store)
            .await?;

            println!(
                "\nRun {key} to see your encryption key - store it somewhere safe.",
                key = "'atuin key'".bold()
            );
            println!("You will need it to log in on other machines, and it cannot be recovered.");
        }
        2 => {
            super::account::login::Cmd {
                username: None,
                password: None,
                key: None,
                totp_code: None,
                from_registration: false,
            }
            .run(settings, store)
            .await?;
        }
        _ => {
            println!(
                "  Skipping sync - you can run {register} or {login} at any time",
                register = "'atuin register'".bold(),
                login = "'atuin login'".bold()
            );
        }
    }

    println!();
    Ok(())
}

pub fn prompt(feature: &str, description: &str, note: Option<&str>) -> Result<bool> {
    println!(
        "> Enable {feature}?",
        feature = feature.bold().bright_blue()
    );
    if let Some(note) = note {
        println!("  {description}");
        print!("  {note} {q} ", q = "[Y/n]".bold());
    } else {
        print!("  {description} {q} ", q = "[Y/n]".bold());
    }

    io::stdout().flush().ok();

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();
    Ok(answer.is_empty() || answer == "y" || answer == "yes")
}
