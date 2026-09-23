use std::io::{self, IsTerminal};

use atuin_client::auth::{self, AuthClient, AuthResponse};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::sync::{ClientSource, SyncError, SyncSession};
use atuin_client::settings::{Settings, SyncAuth};
use atuin_common::encryption::paseto_v4;
use atuin_common::fs;
use atuin_common::utils::env_nonempty;
use clap::Parser;
use eyre::{Context, Result, bail};
use rpassword::prompt_password;

use super::PasswordArg;
use crate::i18n::fl;

const KEY_ENV: &str = "ATUIN_ENCRYPTION_KEY";

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub username: Option<String>,

    #[clap(long, short, help = fl!("arg-password"))]
    pub password: Option<PasswordArg>,

    #[clap(long, short, help = fl!("arg-account-login-key"))]
    pub key: Option<String>,

    #[clap(long, short, help = fl!("arg-totp-code"))]
    pub totp_code: Option<String>,

    #[clap(long, hide = true)]
    pub from_registration: bool,
}

/// Read a line from stdin, returning `None` at end of input. The distinction
/// matters for the key prompts, which re-prompt on a blank line but must not
/// spin forever once stdin is exhausted.
fn get_input() -> Result<Option<String>> {
    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        return Ok(None);
    }
    Ok(Some(input.trim_end_matches(&['\r', '\n'][..]).to_string()))
}

impl Cmd {
    pub async fn run(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        match settings.resolve_sync_auth().await {
            SyncAuth::Hub { .. } => {
                println!("{}", fl!("account-hub-authenticated"));
                println!("{}", fl!("account-run-logout"));
                return Ok(());
            }
            SyncAuth::Legacy { .. } => {
                println!("{}", fl!("login-legacy-logged-in"));
                println!("{}", fl!("account-run-logout"));
                return Ok(());
            }
            SyncAuth::HubViaCli { .. } => {
                println!("{}", fl!("login-upgrading-legacy"));
            }
            SyncAuth::NotLoggedIn { .. } => {}
        }

        if settings.is_hub_sync() {
            self.run_hub_login(settings, store).await?;
        } else {
            self.run_legacy_login(settings, store).await?;
        }

        verify_key_against_remote(settings, store, self.interactive()).await
    }

    /// Whether a rejected key can be corrected by asking for another one.
    fn interactive(&self) -> bool {
        self.scripted_key().is_none() && io::stdin().is_terminal()
    }

    fn scripted_key(&self) -> Option<String> {
        self.key.clone().or_else(|| env_nonempty(KEY_ENV)?.into_string().ok())
    }

    /// Hub login: use the browser flow unless the username was provided for headless use.
    async fn run_hub_login(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        let endpoint = settings.hub_endpoint();

        if let Some(username) = &self.username {
            // Headless login via v0 API (for CI / scripting).
            let client = auth::auth_client(settings).await;

            let password = PasswordArg::resolve(self.password.as_ref(), io::stdin().lock())?
                .unwrap_or_else(read_user_password);

            self.prompt_and_store_key(settings, store).await?;

            let mut totp_code = self.totp_code.clone();

            let (session, auth_type) = loop {
                let response = client.login(username, &password, totp_code.as_deref()).await?;

                match response {
                    AuthResponse::Success { session, auth_type } => break (session, auth_type),
                    AuthResponse::TwoFactorRequired => {
                        let Some(code) = read_user_input(&fl!("prompt-two-factor-code")) else {
                            bail!(fl!("login-totp-required"));
                        };
                        totp_code = Some(code);
                    }
                }
            };

            let meta = Settings::meta_store().await?;
            let is_hub_token = auth_type.as_deref() == Some("hub") || session.starts_with("atapi_");

            if is_hub_token {
                meta.save_hub_session(&session).await?;
            } else {
                meta.save_session(&session).await?;
                println!("\n{}", fl!("account-not-migrated-note"));
                println!("{}", fl!("account-not-migrated-hint"));
            }
        } else {
            // Interactive login via browser OAuth flow.
            if self.from_registration {
                paseto_v4::Key::try_load_or_generate(&settings.key_path)
                    .await
                    .context(fl!("login-key-generate-failed"))?;
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

        println!("{}", fl!("login-success"));
        Ok(())
    }

    /// Legacy login: always prompt for username/password interactively
    /// (or accept them via flags).
    async fn run_legacy_login(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        let username = or_user_input(self.username.clone(), &fl!("prompt-username"));
        let password = PasswordArg::resolve(self.password.as_ref(), io::stdin().lock())?
            .unwrap_or_else(read_user_password);

        self.prompt_and_store_key(settings, store).await?;

        let client = auth::auth_client(settings).await;
        let response = client.login(&username, &password, None).await?;

        match response {
            AuthResponse::Success { session, .. } => {
                Settings::meta_store().await?.save_session(&session).await?;
            }
            AuthResponse::TwoFactorRequired => {
                // Legacy server doesn't support 2FA, so this shouldn't happen.
                bail!(fl!("login-legacy-unexpected-2fa"));
            }
        }

        println!("{}", fl!("login-legacy-success"));
        Ok(())
    }

    async fn ensure_hub_session(&self, _settings: &Settings, hub_address: &url::Url) -> Result<()> {
        tracing::info!("Authenticating with Atuin Hub...");

        let session = atuin_client::hub::HubAuthSession::start(hub_address).await?;
        println!("{}", fl!("account-hub-open-url"));
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

        println!("{}", fl!("login-key-important"));
        println!("{}", fl!("login-key-same-everywhere"));
        println!("{}", fl!("login-key-find"));
        println!("{}", fl!("login-key-secret"));
        println!(
            "\n{} \n",
            fl!("login-key-read-more", url = atuin_common::docs::url("guide/sync/#login"))
        );

        let interactive = self.interactive();
        let mut flag_key = self.scripted_key();

        loop {
            let key = match flag_key.take() {
                Some(key) => key,
                None => match read_user_input(&fl!("prompt-key-or-existing")) {
                    Some(key) => key,
                    // Stdin is exhausted, so re-prompting would spin forever.
                    None => bail!(fl!("login-no-key-provided")),
                },
            };

            if key.is_empty() {
                if !fs::exists(key_path).await.unwrap_or(false) {
                    let msg = fl!("login-no-key-found");
                    if !interactive {
                        bail!(msg);
                    }
                    println!("\n{msg}\n");
                    continue;
                }

                paseto_v4::Key::try_load_from_path(key_path).await.context(fl!(
                    "login-key-file-invalid",
                    path = key_path.to_string_lossy().into_owned()
                ))?;

                return Ok(());
            }

            // The key may be EITHER base64 or a bip39 mnemonic.
            match paseto_v4::Key::try_from_mnemonic(&key) {
                Ok(key) => return store_key(settings, store, &key).await,
                Err(err) if interactive => {
                    println!("\n{}\n", fl!("login-key-try-again", error = err.to_string()));
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}

/// Write the key to the key file, re-encrypting the local store first if it was
/// previously encrypted with a different key.
async fn store_key(settings: &Settings, store: &SqliteStore, key: &paseto_v4::Key) -> Result<()> {
    let key_path = &settings.key_path;

    if !fs::exists(key_path).await.unwrap_or(false) {
        key.try_write_path(key_path).await?;
        return Ok(());
    }

    let current_key = paseto_v4::Key::try_load_from_path(key_path).await?;
    if *key == current_key {
        return Ok(());
    }

    println!("\n{}", fl!("login-reencrypting"));
    store.re_encrypt(&current_key, key).await?;

    println!("{}", fl!("login-writing-key"));
    key.overwrite_path(key_path).await?;

    Ok(())
}

async fn verify_key_against_remote(
    settings: &Settings,
    store: &SqliteStore,
    interactive: bool,
) -> Result<()> {
    let mut key = paseto_v4::Key::try_load_from_path(&settings.key_path)
        .await
        .context(fl!("login-key-load-failed"))?;

    // Build the session once (this hits the network). The key can change between retries below, so
    // each iteration re-keys the shared session rather than reconnecting.
    let session = SyncSession::builder()
        .store(store.clone())
        .client_source(ClientSource::FromSettings {
            settings,
            caps: None,
        })
        .build()
        .connect()
        .await?;
    loop {
        let check = session.keyed(&key).key_valid().await;
        match check {
            // Only persist a key the server has confirmed can read the data, so
            // that cancelling out of a retry leaves the local store as it was.
            None => return store_key(settings, store, &key).await,
            Some(SyncError::WrongKey) => {
                if !interactive {
                    logout_wrong_key().await;
                }

                println!("\n{}", fl!("login-key-mismatch"));
                println!("{}", fl!("login-key-find-correct"));

                let input = read_user_input(&fl!("prompt-key-or-logout"));
                match input {
                    Some(input) if !input.is_empty() => {
                        match paseto_v4::Key::try_from_mnemonic(&input) {
                            Ok(candidate) => key = candidate,
                            Err(err) => {
                                println!(
                                    "\n{}",
                                    fl!("login-key-try-again", error = err.to_string())
                                );
                            }
                        }
                    }
                    // A blank line or exhausted stdin both mean "give up".
                    _ => logout_wrong_key().await,
                }
            }
            Some(e) => {
                // Non-key error (e.g. transient network issue). Don't fail the
                // login — the user is authenticated and can sync later when the
                // network recovers.
                tracing::warn!("could not verify encryption key against remote: {e}");
                return Ok(());
            }
        }
    }
}

/// Roll back the saved session so the user is not left in a half-authenticated
/// state with a key that can't read the data, then exit.
async fn logout_wrong_key() -> ! {
    if let Ok(meta) = Settings::meta_store().await {
        let _ = meta.delete_session().await;
        let _ = meta.delete_hub_session().await;
    }
    crate::print_error::print_error(&fl!("login-wrong-key-title"), &fl!("login-wrong-key-body"));
    std::process::exit(1);
}

#[must_use]
pub(super) fn or_user_input(value: Option<String>, prompt: &str) -> String {
    value.unwrap_or_else(|| read_user_input(prompt).unwrap_or_default())
}

#[must_use]
pub(super) fn read_user_password() -> String {
    let password = prompt_password(format!("{}: ", fl!("prompt-password")));
    password.expect("Failed to read from input")
}

/// Returns `None` if stdin reached end of input before a line was read.
fn read_user_input(prompt: &str) -> Option<String> {
    eprint!("{prompt}: ");
    get_input().expect("Failed to read from input")
}

#[cfg(test)]
mod tests {
    use atuin_common::encryption::paseto_v4;
    use rstest::rstest;

    #[rstest]
    fn mnemonic_round_trip() {
        let key = paseto_v4::Key::from([
            3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8, 9, 7, 9, 3, 2, 3, 8, 4, 6, 2, 6, 4, 3, 3, 8, 3, 2,
            7, 9, 5,
        ]);
        let phrase = bip39::Mnemonic::from_entropy(key.as_bytes(), bip39::Language::English)
            .unwrap()
            .into_phrase();
        let mnemonic = bip39::Mnemonic::from_phrase(&phrase, bip39::Language::English).unwrap();
        assert_eq!(mnemonic.entropy(), key.as_bytes().as_slice());
        assert_eq!(
            phrase,
            "adapt amused able anxiety mother adapt beef gaze amount else seat alcohol cage \
             lottery avoid scare alcohol cactus school avoid coral adjust catch pink"
        );
    }
}
