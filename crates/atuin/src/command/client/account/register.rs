use clap::Parser;
use eyre::{Result, bail};

use super::login::{env_secret, or_user_input, read_secret_from_stdin};
use atuin_client::{
    auth::{self, AuthResponse},
    record::sqlite_store::SqliteStore,
    settings::{Settings, SyncAuth},
};

const PASSWORD_ENV: &str = "ATUIN_PASSWORD";

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub username: Option<String>,

    /// Account password. Falls back to the `ATUIN_PASSWORD` environment
    /// variable, or `--password-stdin`, before prompting interactively.
    #[clap(long, short, conflicts_with = "password_stdin")]
    pub password: Option<String>,

    /// Read the account password from standard input. Mutually exclusive
    /// with `--password`.
    #[clap(long, conflicts_with = "password")]
    pub password_stdin: bool,

    #[clap(long, short)]
    pub email: Option<String>,
}

impl Cmd {
    /// Resolve the account password from, in order: the `--password` flag,
    /// `--password-stdin`, or the `ATUIN_PASSWORD` environment variable.
    /// Returns `None` if no source was provided — callers decide whether
    /// to prompt interactively or fall through to a different flow.
    ///
    /// # Errors
    /// Returns an error if `--password-stdin` was set and stdin could not be
    /// read.
    fn resolve_password(&self) -> Result<Option<String>> {
        if let Some(p) = &self.password {
            return Ok(Some(p.clone()));
        }
        if self.password_stdin {
            return Ok(Some(read_secret_from_stdin()?));
        }
        Ok(env_secret(PASSWORD_ENV))
    }

    #[allow(clippy::too_many_lines)]
    pub async fn run(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        match settings.resolve_sync_auth().await {
            SyncAuth::Hub { .. } => {
                println!("You are already authenticated with Atuin Hub.");
                println!("Run 'atuin logout' to log out.");
                return Ok(());
            }
            SyncAuth::Legacy { .. } => {
                println!("You are already logged in.");
                println!("Run 'atuin logout' to log out.");
                return Ok(());
            }
            SyncAuth::HubViaCli { .. } => {
                println!(
                    "You already have a sync session. \
                     Run 'atuin login' to upgrade to full Hub authentication."
                );
                println!("Run 'atuin logout' first if you want to register a new account.");
                return Ok(());
            }
            SyncAuth::NotLoggedIn { .. } => {}
        }

        // For Hub sync, only resolve the password (which may read stdin) once
        // we know the headless path is reachable; otherwise a piped secret
        // would be silently consumed before falling through to OAuth.
        let resolved_password = if settings.is_hub_sync() {
            if self.username.is_some() && self.email.is_some() {
                self.resolve_password()?
            } else {
                None
            }
        } else {
            // Legacy registration always needs the password.
            self.resolve_password()?
        };

        if settings.is_hub_sync() {
            let required_for_headless = 3;
            let provided = [
                self.username.is_some(),
                self.email.is_some(),
                resolved_password.is_some(),
            ]
            .iter()
            .filter(|&b| *b)
            .count();
            if provided < required_for_headless {
                println!(
                    "Username, password, and email are all required for headless registration. Continuing with interactive registration.\n"
                );
            }

            if let (Some(username), Some(email), Some(password)) =
                (&self.username, &self.email, &resolved_password)
            {
                // Headless registration via v0 API (for CI / scripting).
                let client = auth::auth_client(settings).await;

                if password.is_empty() {
                    bail!("please provide a password");
                }

                let response = client.register(username, email, password).await?;

                match response {
                    AuthResponse::Success { session, auth_type } => {
                        let meta = Settings::meta_store().await?;
                        let is_hub_token =
                            auth_type.as_deref() == Some("hub") || session.starts_with("atapi_");

                        if is_hub_token {
                            meta.save_hub_session(&session).await?;
                        } else {
                            meta.save_session(&session).await?;
                            println!(
                                "\nNote: Your account has not been fully migrated to Atuin Hub."
                            );
                            println!(
                                "Sync will continue to work, but you can visit hub.atuin.sh \
                                to create a new Hub account and link it to your existing CLI account."
                            );
                        }
                    }
                    AuthResponse::TwoFactorRequired => {
                        bail!("unexpected two-factor requirement during registration");
                    }
                }

                let _key = atuin_client::encryption::load_key(settings)?;

                println!(
                    "Registration successful! Please make a note of your key (run 'atuin key') and keep it safe."
                );
                println!(
                    "You will need it to log in on other devices, and we cannot help recover it if you lose it."
                );
            } else {
                // Interactive registration: delegate to the browser OAuth flow.
                // Registration on Hub happens on the website; the CLI just needs
                // to authenticate afterwards.
                super::login::Cmd {
                    username: None,
                    password: None,
                    password_stdin: false,
                    key: None,
                    totp_code: None,
                    from_registration: true,
                }
                .run(settings, store)
                .await?;
            }
        } else {
            // Legacy registration flow
            println!("Registering for an Atuin Sync account");

            let username = or_user_input(self.username.clone(), "username");
            let email = or_user_input(self.email.clone(), "email");
            let password = resolved_password.unwrap_or_else(super::login::read_user_password);

            if password.is_empty() {
                bail!("please provide a password");
            }

            let session = atuin_client::api_client::register(
                settings.sync_address.as_str(),
                &username,
                &email,
                &password,
            )
            .await?;

            let meta = Settings::meta_store().await?;
            meta.save_session(&session.session).await?;

            let _key = atuin_client::encryption::load_key(settings)?;

            println!(
                "Registration successful! Please make a note of your key (run 'atuin key') and keep it safe."
            );
            println!(
                "You will need it to log in on other devices, and we cannot help recover it if you lose it."
            );
        }

        Ok(())
    }
}
