use std::io::{self, IsTerminal, Read};

use atuin_client::auth::{self, AuthClient, AuthResponse};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::sync::{ClientSource, SyncError, SyncSession};
use atuin_client::settings::{Settings, SyncAuth};
use atuin_common::encryption::paseto_v4;
use clap::Parser;
use eyre::{Context, Result, bail};
use rpassword::prompt_password;

const PASSWORD_ENV: &str = "ATUIN_PASSWORD";
const KEY_ENV: &str = "ATUIN_ENCRYPTION_KEY";

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub username: Option<String>,

    /// Account password. Falls back to the `ATUIN_PASSWORD` environment
    /// variable, or `--password-stdin`, before prompting interactively.
    /// Avoid `--password` in shared environments: it is visible in the
    /// process list.
    #[clap(long, short, conflicts_with = "password_stdin")]
    pub password: Option<String>,

    /// Read the account password from standard input. Mutually exclusive
    /// with `--password`.
    #[clap(long, conflicts_with = "password")]
    pub password_stdin: bool,

    /// The encryption key for your account. Falls back to the
    /// `ATUIN_ENCRYPTION_KEY` environment variable before prompting
    /// interactively. (Distinct from the existing `key_path`/`ATUIN_KEY`
    /// concept, which is a path to a key file — this variable holds the
    /// key contents.)
    #[clap(long, short)]
    pub key: Option<String>,

    /// The two-factor authentication code for your account, if any
    #[clap(long, short)]
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
                    "You have a legacy sync session. Continuing login to upgrade to full Hub \
                     authentication."
                );
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
    ///
    /// A key from `--key` is a scripted input: the caller committed to a value
    /// up front, so a wrong one is an error to report rather than a prompt to
    /// raise. Only a human typing at a terminal gets to try again.
    fn interactive(&self) -> bool {
        self.key.is_none() && io::stdin().is_terminal()
    }

    /// Hub login: use the browser flow unless the username was provided for headless use.
    async fn run_hub_login(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        let endpoint = settings.hub_endpoint();

        if let Some(username) = &self.username {
            // Headless login via v0 API (for CI / scripting).
            let client = auth::auth_client(settings).await;

            self.prompt_and_store_key(settings, store).await?;

            let password = self.resolve_password()?;
            let mut totp_code = self.totp_code.clone();

            let (session, auth_type) = loop {
                let response = client.login(username, &password, totp_code.as_deref()).await?;

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
                    "Sync will continue to work, but you can visit hub.atuin.sh to create an \
                     account and link it to your existing CLI account."
                );
            }
        } else {
            // Interactive login via browser OAuth flow.
            if self.from_registration {
                paseto_v4::Key::try_load_or_generate(&settings.key_path)
                    .context("could not load or generate encryption key")?;
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
        let password = self.resolve_password()?;

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

    /// Resolve the account password from, in order: the `--password` flag,
    /// `--password-stdin`, the `ATUIN_PASSWORD` environment variable, or an
    /// interactive prompt.
    ///
    /// # Errors
    /// Returns an error if `--password-stdin` was set and stdin could not be
    /// read.
    fn resolve_password(&self) -> Result<String> {
        if let Some(p) = &self.password {
            return Ok(p.clone());
        }
        if self.password_stdin {
            return read_secret_from_stdin();
        }
        if let Some(p) = env_secret(PASSWORD_ENV) {
            return Ok(p);
        }
        Ok(read_user_password())
    }

    async fn prompt_and_store_key(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        let key_path = &settings.key_path;

        println!("IMPORTANT");
        println!(
            "If you are already logged in on another machine, you must ensure that the key you \
             use here is the same as the key you used there."
        );
        println!("You can find your key by running 'atuin key' on the other machine.");
        println!("Do not share this key with anyone.");
        println!("\nRead more here: {} \n", atuin_common::docs::url("guide/sync/#login"));

        let interactive = self.interactive();
        let mut flag_key = self.key.clone().or_else(|| env_secret(KEY_ENV));

        loop {
            let key = match flag_key.take() {
                Some(key) => key,
                None => match read_user_input("encryption key [blank to use existing key file]") {
                    Some(key) => key,
                    // Stdin is exhausted, so re-prompting would spin forever.
                    None => bail!("No encryption key provided"),
                },
            };

            if key.is_empty() {
                if !key_path.exists() {
                    let msg = "No key provided and no existing key file found. Please use 'atuin \
                               key' on your other machine, or recover your key from a backup";
                    if !interactive {
                        bail!(msg);
                    }
                    println!("\n{msg}\n");
                    continue;
                }

                paseto_v4::Key::try_load_from_path(key_path).context(format!(
                    "The key in existing key file at '{}' is invalid",
                    key_path.to_string_lossy()
                ))?;

                return Ok(());
            }

            // The key may be EITHER base64 or a bip39 mnemonic.
            match paseto_v4::Key::try_from_mnemonic(&key) {
                Ok(key) => return store_key(settings, store, &key).await,
                Err(err) if interactive => println!("\n{err}. Please try again.\n"),
                Err(err) => return Err(err.into()),
            }
        }
    }
}

/// Write the key to the key file, re-encrypting the local store first if it was
/// previously encrypted with a different key.
async fn store_key(settings: &Settings, store: &SqliteStore, key: &paseto_v4::Key) -> Result<()> {
    let key_path = &settings.key_path;

    if !key_path.exists() {
        key.try_write_path(key_path)?;
        return Ok(());
    }

    let current_key = paseto_v4::Key::try_load_from_path(key_path)?;
    if *key == current_key {
        return Ok(());
    }

    println!("\nRe-encrypting local store with new key");
    store.re_encrypt(&current_key, key).await?;

    println!("Writing new key");
    key.overwrite_path(key_path)?;

    Ok(())
}

async fn verify_key_against_remote(
    settings: &Settings,
    store: &SqliteStore,
    interactive: bool,
) -> Result<()> {
    let mut key = paseto_v4::Key::try_load_from_path(&settings.key_path)
        .context("could not load encryption key for verification")?;

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

                println!(
                    "\nThe encryption key on this machine does not match the data on the server."
                );
                println!(
                    "You can find the correct key by running 'atuin key' on a machine that \
                     already syncs successfully."
                );

                let input = read_user_input("encryption key [blank to log out and cancel]");
                match input {
                    Some(input) if !input.is_empty() => {
                        match paseto_v4::Key::try_from_mnemonic(&input) {
                            Ok(candidate) => key = candidate,
                            Err(err) => println!("\n{err}. Please try again."),
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
    crate::print_error::print_error(
        "Wrong encryption key",
        "The encryption key on this machine does not match the data on the server. You have been \
         logged out.\n\nTo fix this, find your existing key by running `atuin key` on a machine \
         that already syncs successfully, then run `atuin login` again here with that key.",
    );
    std::process::exit(1);
}

#[must_use]
pub(super) fn or_user_input(value: Option<String>, name: &'static str) -> String {
    value.unwrap_or_else(|| read_user_input(name).unwrap_or_default())
}

#[must_use]
pub(super) fn read_user_password() -> String {
    let password = prompt_password("Please enter password: ");
    password.expect("Failed to read from input")
}

/// Returns `None` if stdin reached end of input before a line was read.
fn read_user_input(name: &'static str) -> Option<String> {
    eprint!("Please enter {name}: ");
    get_input().expect("Failed to read from input")
}

/// Return the value of `var` if it is set and non-empty.
pub(super) fn env_secret(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.is_empty())
}

/// Read a secret from stdin, stripping a single trailing newline (CR/LF).
///
/// # Errors
/// Returns an error if stdin cannot be read or does not contain valid UTF-8.
pub(super) fn read_secret_from_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf).context("failed to read secret from stdin")?;
    Ok(buf.trim_end_matches(&['\r', '\n'][..]).to_string())
}

#[cfg(test)]
mod tests {
    use atuin_common::encryption::paseto_v4;
    use clap::Parser;
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn password_and_password_stdin_are_mutually_exclusive() {
        let result = Cmd::try_parse_from(["login", "--password", "x", "--password-stdin"]);
        assert!(result.is_err(), "clap should reject both flags together");
    }

    #[rstest]
    fn password_stdin_parses_without_password_flag() {
        let cmd = Cmd::try_parse_from(["login", "--password-stdin"]).unwrap();
        assert!(cmd.password_stdin);
        assert!(cmd.password.is_none());
    }

    #[rstest]
    fn defaults_leave_stdin_flag_false() {
        let cmd = Cmd::try_parse_from(["login"]).unwrap();
        assert!(!cmd.password_stdin);
        assert!(cmd.password.is_none());
    }

    // The env var names in these tests carry a `_XYZZY` suffix so that no
    // other code in the test binary (now or in the future) is expected to
    // read them. That isolation is what makes the `unsafe` mutations sound
    // under edition 2024's stricter env-mutation contract: no concurrent
    // reader can observe torn state, because there is no concurrent reader.

    #[rstest]
    fn env_secret_returns_none_when_unset() {
        let name = "ATUIN_TEST_ENV_SECRET_UNSET_XYZZY";
        // SAFETY: no other test or production code reads this uniquely-named
        // env var, so a parallel test thread cannot observe this mutation.
        unsafe { std::env::remove_var(name) };
        assert_eq!(env_secret(name), None);
    }

    #[rstest]
    fn env_secret_returns_none_when_empty() {
        let name = "ATUIN_TEST_ENV_SECRET_EMPTY_XYZZY";
        // SAFETY: no other test or production code reads this uniquely-named
        // env var, so a parallel test thread cannot observe this mutation.
        unsafe { std::env::set_var(name, "") };
        assert_eq!(env_secret(name), None);
        // SAFETY: same as above.
        unsafe { std::env::remove_var(name) };
    }

    #[rstest]
    fn env_secret_returns_value_when_set() {
        let name = "ATUIN_TEST_ENV_SECRET_SET_XYZZY";
        // SAFETY: no other test or production code reads this uniquely-named
        // env var, so a parallel test thread cannot observe this mutation.
        unsafe { std::env::set_var(name, "hunter2") };
        assert_eq!(env_secret(name), Some("hunter2".to_string()));
        // SAFETY: same as above.
        unsafe { std::env::remove_var(name) };
    }

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
