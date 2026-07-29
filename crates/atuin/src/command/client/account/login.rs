use std::io::{self, IsTerminal};

use clap::Parser;
use eyre::{Context, Result, bail};
use tokio::{fs::File, io::AsyncWriteExt};

use atuin_client::{
    auth::{self, AuthResponse},
    encryption::{Key, decode_key, encode_key, load_key},
    record::sqlite_store::SqliteStore,
    record::store::Store,
    record::sync::{self, SyncError},
    settings::{Settings, SyncAuth},
};
use rpassword::prompt_password;

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub username: Option<String>,

    #[clap(long, short)]
    pub password: Option<String>,

    /// The encryption key for your account
    #[clap(long, short)]
    pub key: Option<String>,

    /// The two-factor authentication code for your account, if any
    #[clap(long, short)]
    pub totp_code: Option<String>,

    #[clap(long, hide = true)]
    pub from_registration: bool,
}

fn get_input() -> Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
}

impl Cmd {
    pub async fn run(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        match settings.resolve_sync_auth().await {
            SyncAuth::Hub { .. } => {
                println!("You are authenticated with Atuin Hub.");
                println!("Run 'atuin logout' to log out.");
                return Ok(());
            }
            SyncAuth::Legacy { .. } => {
                println!("You are logged in to your sync server.");
                println!("Run 'atuin logout' to log out.");
                return Ok(());
            }
            SyncAuth::HubViaCli { .. } => {
                println!(
                    "You have a legacy sync session. \
                     Continuing login to upgrade to full Hub authentication."
                );
            }
            SyncAuth::NotLoggedIn { .. } => {}
        }

        if settings.is_hub_sync() {
            self.run_hub_login(settings, store).await?;
        } else {
            self.run_legacy_login(settings, store).await?;
        }

        // Verify the key can decrypt the data on the server, prompting for a
        // new key until it does. Candidates are only held in memory; the
        // store is re-encrypted once, after a key is confirmed correct.
        let stored_key: [u8; 32] = load_key(settings)
            .context("could not load encryption key for verification")?
            .into();
        let mut key = stored_key;

        while !verify_key_against_remote(settings, &key).await? {
            if self.key.is_none() && io::stdin().is_terminal() {
                println!(
                    "\nThat encryption key does not match the data on the server.\n\
                     Find the correct key by running 'atuin key' on a machine that already \
                     syncs successfully, and try again.\n"
                );
                eprint!("Please enter encryption key [blank to log out]: ");

                let mut line = String::new();
                let eof = io::stdin().read_line(&mut line)? == 0;
                let input = line.trim_end_matches(['\r', '\n']).to_string();

                if !eof && !input.is_empty() {
                    match normalize_key(input) {
                        Ok(encoded) => key = decode_key(encoded)?.into(),
                        Err(e) => println!("{e}"),
                    }
                    continue;
                }
            }

            // Give up: roll back the saved session so the user is not left
            // half-authenticated with a key that can't read the data.
            if let Ok(meta) = Settings::meta_store().await {
                let _ = meta.delete_session().await;
                let _ = meta.delete_hub_session().await;
            }
            crate::print_error::print_error(
                "Wrong encryption key",
                "The encryption key on this machine does not match the data on the server. \
                 You have been logged out.\n\n\
                 To fix this, find your existing key by running `atuin key` on a machine that \
                 already syncs successfully, then run `atuin login` again here with that key.",
            );
            std::process::exit(1);
        }

        if key != stored_key {
            println!("\nRe-encrypting local store with the new key");
            store.re_encrypt(&stored_key, &key).await?;

            println!("Writing new key");
            let mut file = File::create(&settings.key_path).await?;
            file.write_all(encode_key(Key::from_slice(&key))?.as_bytes())
                .await?;
        }

        Ok(())
    }

    /// Hub login: use the browser flow unless the username was provided for headless use.
    async fn run_hub_login(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        let endpoint = settings.hub_endpoint();

        if let Some(username) = &self.username {
            // Headless login via v0 API (for CI / scripting).
            let client = auth::auth_client(settings).await;

            self.prompt_and_store_key(settings, store).await?;

            let password = self.password.clone().unwrap_or_else(read_user_password);
            let mut totp_code = self.totp_code.clone();

            let (session, auth_type) = loop {
                let response = client
                    .login(username, &password, totp_code.as_deref())
                    .await?;

                match response {
                    AuthResponse::Success { session, auth_type } => break (session, auth_type),
                    AuthResponse::TwoFactorRequired => {
                        totp_code = Some(or_user_input(None, "two-factor code"));
                    }
                }
            };

            let meta = Settings::meta_store().await?;
            let is_hub_token = auth_type.as_deref() == Some("hub") || session.starts_with("atapi_");

            if is_hub_token {
                meta.save_hub_session(&session).await?;
            } else {
                meta.save_session(&session).await?;
                println!("\nNote: Your account has not been fully migrated to Atuin Hub.");
                println!(
                    "Sync will continue to work, but you can visit hub.atuin.sh \
                     to create an account and link it to your existing CLI account."
                );
            }
        } else {
            // Interactive login via browser OAuth flow.
            if self.from_registration {
                load_key(settings)?;
            } else {
                self.prompt_and_store_key(settings, store).await?;
            }

            self.ensure_hub_session(settings, &endpoint).await?;
        }

        // Silently attempt to link CLI account to Hub if one exists
        if let Ok(cli_token) = settings.session_token().await
            && let Err(e) = atuin_client::hub::link_account(&endpoint, &cli_token).await
        {
            tracing::debug!("Could not link CLI account to Hub: {}", e);
        }

        println!("Successfully authenticated.");
        Ok(())
    }

    /// Legacy login: always prompt for username/password interactively
    /// (or accept them via flags).
    async fn run_legacy_login(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        let username = or_user_input(self.username.clone(), "username");
        let password = self.password.clone().unwrap_or_else(read_user_password);

        self.prompt_and_store_key(settings, store).await?;

        let client = auth::auth_client(settings).await;
        let response = client.login(&username, &password, None).await?;

        match response {
            AuthResponse::Success { session, .. } => {
                Settings::meta_store().await?.save_session(&session).await?;
            }
            AuthResponse::TwoFactorRequired => {
                // Legacy server doesn't support 2FA, so this shouldn't happen.
                bail!("unexpected two-factor requirement from legacy server");
            }
        }

        println!("Logged in!");
        Ok(())
    }

    async fn ensure_hub_session(&self, _settings: &Settings, hub_address: &url::Url) -> Result<()> {
        tracing::info!("Authenticating with Atuin Hub...");

        let session = atuin_client::hub::HubAuthSession::start(hub_address).await?;
        println!("Open this URL to continue authenticating with Atuin Hub:");
        println!("{}", session.auth_url);

        let token = session
            .wait_for_completion(
                atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
                atuin_client::hub::DEFAULT_POLL_INTERVAL,
            )
            .await?;

        tracing::info!("Authentication complete, saving session token");

        atuin_client::hub::save_session(&token).await?;

        Ok(())
    }

    async fn prompt_and_store_key(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        let key_path = &settings.key_path;

        println!("Logging in requires the encryption key for your account.");
        println!(
            "To get it, run 'atuin key' on a machine where you are already logged in, and paste it below."
        );
        println!("Do not share this key with anyone.");
        println!(
            "\nRead more here: {} \n",
            atuin_common::docs::url("guide/sync/#login")
        );

        let key_prompt = if key_path.exists() {
            "encryption key [blank to use existing key file]"
        } else {
            "encryption key"
        };

        // The key may be entered as base64 or as a bip39 mnemonic; keep
        // prompting until we get something valid. A key passed via --key
        // fails fast instead.
        let key = match self.key.clone() {
            Some(key) if !key.is_empty() => normalize_key(key)?,
            Some(key) => key, // empty --key: fall through to the existing key file
            None => loop {
                eprint!("Please enter {key_prompt}: ");
                let mut line = String::new();
                let eof = io::stdin().read_line(&mut line)? == 0;
                let input = line.trim_end_matches(['\r', '\n']).to_string();
                let interactive = !eof && io::stdin().is_terminal();

                if input.is_empty() {
                    if key_path.exists() {
                        break input;
                    }

                    let msg = "No encryption key provided, and no existing key was found on this machine.\n\
                               Run 'atuin key' on a machine that is already logged in to get your key.";
                    if !interactive {
                        bail!(msg);
                    }
                    println!("{msg}\n");
                    continue;
                }

                match normalize_key(input) {
                    Ok(encoded) => break encoded,
                    Err(e) if interactive => println!("{e}\n"),
                    Err(e) => return Err(e),
                }
            },
        };

        if key.is_empty() {
            // key_path is known to exist here
            let bytes = fs_err::read_to_string(key_path).context(format!(
                "Existing key file at '{}' could not be read",
                key_path.to_string_lossy()
            ))?;
            if decode_key(bytes).is_err() {
                bail!(format!(
                    "The key in existing key file at '{}' is invalid",
                    key_path.to_string_lossy()
                ));
            }
        } else if !key_path.exists() {
            let mut file = File::create(key_path).await?;
            file.write_all(key.as_bytes()).await?;
        } else {
            // we now know that the user has logged in specifying a key, AND that the key path
            // exists

            // 1. check if the saved key and the provided key match. if so, nothing to do.
            // 2. if not, re-encrypt the local history and overwrite the key
            let current_key: [u8; 32] = load_key(settings)?.into();

            let encoded = key.clone(); // gonna want to save it in a bit
            let new_key: [u8; 32] = decode_key(key)
                .context("Could not decode provided key; is not valid base64-encoded key")?
                .into();

            if new_key != current_key {
                println!("\nRe-encrypting local store with new key");

                store.re_encrypt(&current_key, &new_key).await?;

                println!("Writing new key");
                let mut file = File::create(key_path).await?;
                file.write_all(encoded.as_bytes()).await?;
            }
        }

        Ok(())
    }
}

/// Check that an encryption key can decrypt the data on the server.
/// Returns `Ok(false)` if the key is wrong; transient errors (e.g. network)
/// are treated as verified so login still succeeds.
async fn verify_key_against_remote(settings: &Settings, key: &[u8; 32]) -> Result<bool> {
    let client = sync::build_client(settings).await?;
    let remote_index = match client.record_status().await {
        Ok(idx) => idx,
        Err(e) => {
            tracing::warn!("could not fetch remote status to verify key: {e}");
            return Ok(true);
        }
    };

    match sync::check_encryption_key(&client, &remote_index, key).await {
        Ok(()) => Ok(true),
        Err(SyncError::WrongKey) => Ok(false),
        Err(e) => {
            tracing::warn!("could not verify encryption key against remote: {e}");
            Ok(true)
        }
    }
}

/// A key may be entered either as base64 or as a bip39 mnemonic; normalize to
/// the base64 encoding, validating that it decodes to a real key.
fn normalize_key(key: String) -> Result<String> {
    let encoded = match bip39::Mnemonic::from_phrase(&key, bip39::Language::English) {
        Ok(mnemonic) => encode_key(Key::from_slice(mnemonic.entropy()))?,
        Err(err) => {
            match err {
                // assume they copied in the base64 key
                bip39::ErrorKind::InvalidWord(_) => key,
                bip39::ErrorKind::InvalidChecksum => {
                    bail!("Key mnemonic is not valid")
                }
                bip39::ErrorKind::InvalidKeysize(_)
                | bip39::ErrorKind::InvalidWordLength(_)
                | bip39::ErrorKind::InvalidEntropyLength(_, _) => {
                    bail!("Key is not the correct length")
                }
            }
        }
    };

    if decode_key(encoded.clone()).is_err() {
        bail!("The provided key is invalid - it should be a base64 string or a 24 word mnemonic");
    }

    Ok(encoded)
}

pub(super) fn or_user_input(value: Option<String>, name: &'static str) -> String {
    value.unwrap_or_else(|| read_user_input(name))
}

pub(super) fn read_user_password() -> String {
    let password = prompt_password("Please enter password: ");
    password.expect("Failed to read from input")
}

fn read_user_input(name: &'static str) -> String {
    eprint!("Please enter {name}: ");
    get_input().expect("Failed to read from input")
}

#[cfg(test)]
mod tests {
    use atuin_client::encryption::Key;

    #[test]
    fn mnemonic_round_trip() {
        let key = Key::from([
            3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8, 9, 7, 9, 3, 2, 3, 8, 4, 6, 2, 6, 4, 3, 3, 8, 3, 2,
            7, 9, 5,
        ]);
        let phrase = bip39::Mnemonic::from_entropy(&key, bip39::Language::English)
            .unwrap()
            .into_phrase();
        let mnemonic = bip39::Mnemonic::from_phrase(&phrase, bip39::Language::English).unwrap();
        assert_eq!(mnemonic.entropy(), key.as_slice());
        assert_eq!(
            phrase,
            "adapt amused able anxiety mother adapt beef gaze amount else seat alcohol cage lottery avoid scare alcohol cactus school avoid coral adjust catch pink"
        );
    }
}
