use clap::Parser;
use eyre::{Result, bail};

use super::login::or_user_input;
use atuin_client::{
    auth::{self, AuthResponse},
    record::sqlite_store::SqliteStore,
    settings::{Settings, SyncAuth},
};

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

        if settings.is_hub_sync() {
            let required_for_headless = 3;
            let provided = [
                self.username.is_some(),
                self.email.is_some(),
                self.password.is_some(),
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
                (&self.username, &self.email, &self.password)
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
            let password = self
                .password
                .clone()
                .unwrap_or_else(super::login::read_user_password);

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
