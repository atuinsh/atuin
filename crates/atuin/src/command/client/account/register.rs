use clap::Parser;
use eyre::{Result, bail};

use super::login::or_user_input;
use atuin_client::{
    auth::{self, AuthResponse},
    record::sqlite_store::SqliteStore,
    settings::Settings,
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
    pub async fn run(self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        run(settings, store, self.username, self.email, self.password).await
    }
}

pub async fn run(
    settings: &Settings,
    store: &SqliteStore,
    username: Option<String>,
    email: Option<String>,
    password: Option<String>,
) -> Result<()> {
    if settings.logged_in().await? {
        if settings.is_hub_sync() {
            println!("You are already authenticated with Atuin Hub.");
        } else {
            println!("You are already logged in.");
        }
        println!("Run 'atuin logout' to log out.");
        return Ok(());
    }

    if settings.is_hub_sync() {
        let required_for_headless = 3;
        let provided = [username.is_some(), email.is_some(), password.is_some()]
            .iter()
            .filter(|&b| *b)
            .count();
        if provided < required_for_headless {
            println!(
                "Username, password, and email are all required for headless registration. Continuing with interactive registration.\n"
            );
        }

        if username.is_some() && email.is_some() && password.is_some() {
            // Headless registration via v0 API (for CI / scripting).
            let client = auth::auth_client(settings).await;

            let username = username.unwrap();
            let email = email.unwrap();
            let password = password.unwrap();

            if password.is_empty() {
                bail!("please provide a password");
            }

            let response = client.register(&username, &email, &password).await?;

            match response {
                AuthResponse::Success { session } => {
                    Settings::meta_store()
                        .await?
                        .save_hub_session(&session)
                        .await?;
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

        let username = or_user_input(username, "username");
        let email = or_user_input(email, "email");
        let password = password
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
