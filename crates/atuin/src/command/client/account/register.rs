use std::sync::Arc;

use atuin_client::auth::{self, AuthClient, AuthResponse};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::{Settings, SyncAuth};
use atuin_common::encryption::paseto_v4;
use atuin_common::userio;
use atuin_common::userio::{Prompt, Secret};
use clap::Parser;
use eyre::{Result, bail};
use secrecy::ExposeSecret;

use super::PasswordArg;
use crate::i18n::fl;

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub username: Option<String>,

    #[clap(long, short, help = fl!("arg-password"))]
    pub password: Option<PasswordArg>,

    #[clap(long, short)]
    pub email: Option<String>,
}

impl Cmd {
    #[allow(clippy::too_many_lines)]
    pub async fn run(&self, settings: &Arc<Settings>, store: &SqliteStore) -> Result<()> {
        match settings.resolve_sync_auth().await {
            SyncAuth::Hub { .. } => {
                userio::log::info!("{}", fl!("account-hub-authenticated"));
                userio::log::info!("{}", fl!("account-run-logout"));
                return Ok(());
            }
            SyncAuth::Legacy { .. } => {
                userio::log::info!("{}", fl!("register-legacy-already"));
                userio::log::info!("{}", fl!("account-run-logout"));
                return Ok(());
            }
            SyncAuth::HubViaCli { .. } => {
                userio::log::info!("{}", fl!("register-has-legacy-session"));
                userio::log::info!("{}", fl!("register-logout-first"));
                return Ok(());
            }
            SyncAuth::NotLoggedIn { .. } => {}
        }

        if settings.is_hub_sync() {
            // Only resolved once headless registration is reachable, so that
            // `--password -` does not swallow stdin before the browser flow.
            let password = match (&self.username, &self.email) {
                (Some(_), Some(_)) => {
                    PasswordArg::resolve(self.password.as_ref(), std::io::stdin().lock())?
                }
                _ => None,
            };

            if let (Some(username), Some(email), Some(password)) =
                (&self.username, &self.email, &password)
            {
                // Headless registration via v0 API (for CI / scripting).
                let client = auth::auth_client(settings).await;

                if password.expose_secret().is_empty() {
                    bail!(fl!("account-provide-password"));
                }

                let response = client.register(username, email, password).await?;

                match response {
                    AuthResponse::Success { session, auth_type } => {
                        let meta = settings.meta_store().await?;
                        let is_hub_token = auth_type.as_deref() == Some("hub")
                            || atuin_client::meta::is_hub_token(&session);

                        if is_hub_token {
                            meta.save_hub_session(&session).await?;
                        } else {
                            meta.save_session(&session).await?;
                            userio::log::info!("{}", fl!("account-not-migrated-note"));
                            userio::log::info!("{}", fl!("account-not-migrated-hint"));
                        }
                    }
                    AuthResponse::TwoFactorRequired => {
                        bail!(fl!("register-unexpected-2fa"));
                    }
                }

                let _key = paseto_v4::Key::try_load_or_generate(&settings.key_path)?;

                userio::log::info!("{}", fl!("register-success-key"));
                userio::log::info!("{}", fl!("register-key-warning"));
            } else {
                userio::log::info!("{}", fl!("register-headless-incomplete"));

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
            userio::log::info!("{}", fl!("register-legacy-start"));

            let username = match self.username.clone() {
                Some(username) => username,
                None => Prompt::new(&fl!("prompt-username")).ask()?.unwrap_or_default(),
            };
            let email = match self.email.clone() {
                Some(email) => email,
                None => Prompt::new(&fl!("prompt-email")).ask()?.unwrap_or_default(),
            };
            let password =
                match PasswordArg::resolve(self.password.as_ref(), std::io::stdin().lock())? {
                    Some(password) => password,
                    None => Secret::new(&fl!("prompt-password")).ask()?.unwrap_or_default(),
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

            let meta = settings.meta_store().await?;
            meta.save_session(&session.session).await?;

            let _key = paseto_v4::Key::try_load_or_generate(&settings.key_path)?;

            userio::log::info!("{}", fl!("register-success-key"));
            userio::log::info!("{}", fl!("register-key-warning"));
        }

        Ok(())
    }
}
