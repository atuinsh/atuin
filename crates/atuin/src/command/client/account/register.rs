use clap::Parser;
use eyre::{Result, bail};

use super::login::or_user_input;
use atuin_client::{api_client, record::sqlite_store::SqliteStore, settings::Settings};

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
    if let Some(_endpoint) = settings.active_hub_endpoint() {
        if settings.hub_session_token().await.is_ok() {
            println!("You are already authenticated with Atuin Hub.");
            println!("Run 'atuin logout' to log out.");
            return Ok(());
        }

        // Login can also handle registration, as the registration piece for Hub auth lives on the server
        // (e.g. create a new Hub account, then log in as normal)
        super::login::Cmd {
            username: None,
            password: None,
            key: None,
            from_registration: true,
        }
        .run(settings, store)
        .await?;
        return Ok(());
    }

    if settings.session_token().await.is_ok() {
        println!("You are already logged in.");
        println!("Run 'atuin logout' to log out.");
        return Ok(());
    }

    println!("Registering for an Atuin Sync account");

    let username = or_user_input(username, "username");
    let email = or_user_input(email, "email");

    let password = password
        .clone()
        .unwrap_or_else(super::login::read_user_password);

    if password.is_empty() {
        bail!("please provide a password");
    }

    let session =
        api_client::register(settings.sync_address.as_str(), &username, &email, &password).await?;

    let meta = Settings::meta_store().await?;
    meta.save_session(&session.session).await?;

    let _key = atuin_client::encryption::load_key(settings)?;

    println!(
        "Registration successful! Please make a note of your key (run 'atuin key') and keep it safe."
    );
    println!(
        "You will need it to log in on other devices, and we cannot help recover it if you lose it."
    );

    Ok(())
}
