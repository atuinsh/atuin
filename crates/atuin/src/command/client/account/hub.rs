use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use eyre::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use atuin_client::api_client;
use atuin_client::encryption::{decode_key, encode_key, load_key, Key};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::store::Store;
use atuin_client::settings::Settings;

/// Timeout for the entire auth flow (10 minutes)
const AUTH_TIMEOUT: Duration = Duration::from_secs(600);
/// How often to poll for verification
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Run the Hub authentication flow.
///
/// This is used by both `register` and `login` commands when `hub_sync` is enabled.
/// The flow is identical for both since the Hub web UI handles registration vs login.
pub async fn run(settings: &Settings, store: &SqliteStore) -> Result<()> {
    println!("Authenticating with Atuin Hub...");

    // 1. Request a code from the hub
    let code_response = api_client::hub_request_code(&settings.hub_address)
        .await
        .context("Failed to request authentication code from Hub")?;

    let code = &code_response.code;
    let auth_url = format!("{}/auth/cli?code={}", settings.hub_address, code);

    // 2. Try to open the browser and print the URL regardless
    println!("\nOpening your browser to complete authentication...");
    println!("If it doesn't open, visit this URL:\n");
    println!("  {auth_url}\n");

    if let Err(e) = open::that(&auth_url) {
        tracing::debug!("Failed to open browser: {}", e);
    }

    // 3. Poll for verification with a spinner
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
            .template("{spinner:.cyan} {msg}")
            .expect("valid template"),
    );
    spinner.set_message("Waiting for authorization...");
    spinner.enable_steady_tick(Duration::from_millis(100));

    let start = std::time::Instant::now();
    let session = loop {
        if start.elapsed() > AUTH_TIMEOUT {
            spinner.finish_with_message("Timed out");
            bail!("Authentication timed out. Please try again.");
        }

        tokio::time::sleep(POLL_INTERVAL).await;

        match api_client::hub_verify_code(&settings.hub_address, code).await {
            Ok(verify_response) => {
                if let Some(session) = verify_response.token {
                    spinner.finish_with_message("Authorized!\n");
                    break session;
                }
                // Still pending, continue polling
            }
            Err(e) => {
                // Log the error but keep polling - could be transient
                tracing::debug!("Verification poll failed: {}", e);
            }
        }
    };

    // 4. Save the session token
    let session_path = settings.session_path.as_str();
    let mut file = File::create(session_path)
        .await
        .context("Failed to create session file")?;
    file.write_all(session.as_bytes())
        .await
        .context("Failed to write session file")?;

    // 5. Handle encryption key - always prompt
    // Users will always have a key file by this point (created on first atuin usage)
    // But it might not be the right key for this account if they registered elsewhere
    let key_path = PathBuf::from(settings.key_path.as_str());

    println!();
    println!(
        "If you have already registered on another machine, you will need your encryption key."
    );
    println!("Run 'atuin key' on your other machine to retrieve it.");
    println!();

    let key_input = read_key_input()?;

    if let Some(encoded_key) = key_input {
        // User provided a key - check if we need to re-encrypt
        let current_key: [u8; 32] = load_key(settings)?.into();
        let new_key: [u8; 32] = decode_key(encoded_key.clone())
            .context("Could not decode provided key")?
            .into();

        if new_key != current_key {
            println!("\nRe-encrypting local store with new key...");

            store.re_encrypt(&current_key, &new_key).await?;

            println!("Writing new key");
            let mut file = File::create(&key_path)
                .await
                .context("Failed to create key file")?;
            file.write_all(encoded_key.as_bytes())
                .await
                .context("Failed to write key file")?;
        }
    }
    // If user pressed Enter, we just use the existing key (nothing to do)

    println!("\nAuthentication successful!");
    println!();
    println!("IMPORTANT: Please make a note of your key (run 'atuin key') and keep it safe.");
    println!(
        "You will need it to log in on other devices, and we cannot help recover it if you lose it."
    );

    Ok(())
}

/// Prompt the user for an encryption key.
/// Returns `Some(encoded_key)` if they provided one, None if they pressed Enter.
fn read_key_input() -> Result<Option<String>> {
    print!("Enter your encryption key, or press Enter if you don't already have one: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        return Ok(None);
    }

    // The key may be EITHER base64, or a bip mnemonic
    // Try to normalize to base64
    let encoded = match bip39::Mnemonic::from_phrase(input, bip39::Language::English) {
        Ok(mnemonic) => encode_key(Key::from_slice(mnemonic.entropy()))?,
        Err(err) => {
            match err.downcast_ref::<bip39::ErrorKind>() {
                Some(bip_err) => {
                    match bip_err {
                        // Not a valid mnemonic word - assume they copied the base64 key
                        bip39::ErrorKind::InvalidWord => input.to_string(),
                        bip39::ErrorKind::InvalidChecksum => {
                            bail!("Key mnemonic was not valid")
                        }
                        bip39::ErrorKind::InvalidKeysize(_)
                        | bip39::ErrorKind::InvalidWordLength(_)
                        | bip39::ErrorKind::InvalidEntropyLength(_, _) => {
                            bail!("Key was not the correct length")
                        }
                    }
                }
                _ => {
                    // Unknown error - assume they copied the base64 key
                    input.to_string()
                }
            }
        }
    };

    // Validate the key
    if decode_key(encoded.clone()).is_err() {
        bail!("The provided key was invalid");
    }

    Ok(Some(encoded))
}
