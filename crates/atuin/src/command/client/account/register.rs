use atuin_client::auth::{self, AuthClient, AuthResponse};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::{Settings, SyncAuth};
use atuin_common::encryption::paseto_v4;
use clap::Parser;
use clap_couture::Couture;
use eyre::{Result, bail};
use secrecy::{ExposeSecret, SecretString};

use super::prompter;
use crate::i18n::fl;

#[derive(Parser, Debug, Couture)]
pub struct Cmd {
    #[clap(long, short)]
    pub username: Option<String>,

    #[clap(long, short, env = "ATUIN_PASSWORD", hide_env_values = true, help = fl!("arg-password"))]
    #[couture(accept_stdin(dash))]
    pub password: Option<SecretString>,

    #[clap(long, short)]
    pub email: Option<String>,
}

impl Cmd {
    #[allow(clippy::too_many_lines)]
    pub async fn run(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        match settings.resolve_sync_auth().await {
            SyncAuth::Hub { .. } => {
                println!("{}", fl!("account-hub-authenticated"));
                println!("{}", fl!("account-run-logout"));
                return Ok(());
            }
            SyncAuth::Legacy { .. } => {
                println!("{}", fl!("register-legacy-already"));
                println!("{}", fl!("account-run-logout"));
                return Ok(());
            }
            SyncAuth::HubViaCli { .. } => {
                println!("{}", fl!("register-has-legacy-session"));
                println!("{}", fl!("register-logout-first"));
                return Ok(());
            }
            SyncAuth::NotLoggedIn { .. } => {}
        }

        if settings.is_hub_sync() {
            if let (Some(username), Some(email), Some(password)) =
                (&self.username, &self.email, &self.password)
            {
                // Headless registration via v0 API (for CI / scripting).
                let client = auth::auth_client(settings).await;

                if password.expose_secret().is_empty() {
                    bail!(fl!("account-provide-password"));
                }

                let response = client.register(username, email, password).await?;

                match response {
                    AuthResponse::Success { session, auth_type } => {
                        let meta = Settings::meta_store().await?;
                        let is_hub_token = auth_type.as_deref() == Some("hub")
                            || atuin_client::meta::is_hub_token(&session);

                        if is_hub_token {
                            meta.save_hub_session(&session).await?;
                        } else {
                            meta.save_session(&session).await?;
                            println!("\n{}", fl!("account-not-migrated-note"));
                            println!("{}", fl!("account-not-migrated-hint"));
                        }
                    }
                    AuthResponse::TwoFactorRequired => {
                        bail!(fl!("register-unexpected-2fa"));
                    }
                }

                let _key = paseto_v4::Key::try_load_or_generate(&settings.key_path)?;

                println!("{}", fl!("register-success-key"));
                println!("{}", fl!("register-key-warning"));
            } else {
                println!("{}\n", fl!("register-headless-incomplete"));

                // Interactive registration: delegate to the browser OAuth flow.
                // Registration on Hub happens on the website; the CLI just needs
                // to authenticate afterwards.
                super::login::Cmd {
                    username: None,
                    password: None,
                    key: None,
                    totp_code: None,
                    from_registration: true,
                }
                .run(settings, store)
                .await?;
            }
        } else {
            // Legacy registration flow
            println!("{}", fl!("register-legacy-start"));

            let username = match self.username.clone() {
                Some(username) => username,
                None => prompter("--username")?.text(&fl!("prompt-username"))?,
            };
            let email = match self.email.clone() {
                Some(email) => email,
                None => prompter("--email")?.text(&fl!("prompt-email"))?,
            };
            let password: SecretString = match self.password.clone() {
                Some(password) => password,
                None => prompter("--password")?.secret(&fl!("prompt-password"))?,
            };

            if password.expose_secret().is_empty() {
                bail!(fl!("account-provide-password"));
            }

            let session = atuin_client::api_client::register(
                &settings.sync_address,
                &username,
                &email,
                &password,
                &settings.extra_headers,
            )
            .await?;

            let meta = Settings::meta_store().await?;
            meta.save_session(&session.session).await?;

            let _key = paseto_v4::Key::try_load_or_generate(&settings.key_path)?;

            println!("{}", fl!("register-success-key"));
            println!("{}", fl!("register-key-warning"));
        }

        Ok(())
    }
}
