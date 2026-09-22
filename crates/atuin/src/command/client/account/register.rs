use atuin_client::auth::{self, AuthClient, AuthResponse};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::{Settings, SyncAuth};
use atuin_common::encryption::paseto_v4;
use clap::Parser;
use eyre::{Result, bail};

use super::login::or_user_input;
use crate::i18n::fl;

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub username: Option<String>,

    #[clap(long, short)]
    pub password: Option<String>,

    #[clap(long, short)]
    pub email: Option<String>,
}

impl Cmd {
    #[allow(clippy::too_many_lines)]
    pub async fn run(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        match settings.resolve_sync_auth().await {
            SyncAuth::Hub { .. } => {
                println!("{}", fl!("register-hub-already"));
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
            let required_for_headless = 3;
            let provided = [self.username.is_some(), self.email.is_some(), self.password.is_some()]
                .iter()
                .filter(|&b| *b)
                .count();
            if provided < required_for_headless {
                println!("{}\n", fl!("register-headless-incomplete"));
            }

            if let (Some(username), Some(email), Some(password)) =
                (&self.username, &self.email, &self.password)
            {
                // Headless registration via v0 API (for CI / scripting).
                let client = auth::auth_client(settings).await;

                if password.is_empty() {
                    bail!(fl!("account-provide-password"));
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
                            println!("\n{}", fl!("account-not-migrated-note"));
                            println!("{}", fl!("register-not-migrated-hint"));
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

            let username = or_user_input(self.username.clone(), &fl!("prompt-username"));
            let email = or_user_input(self.email.clone(), &fl!("prompt-email"));
            let password = self.password.clone().unwrap_or_else(super::login::read_user_password);

            if password.is_empty() {
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
