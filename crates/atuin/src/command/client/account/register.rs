use clap::Parser;
use eyre::{Result, bail};

use atuin_client::{api_client, settings::Settings};

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
    pub async fn run(self, settings: &Settings) -> Result<()> {
        run(settings, self.username, self.email, self.password).await
    }
}

pub async fn run(
    settings: &Settings,
    username: Option<String>,
    email: Option<String>,
    password: Option<String>,
) -> Result<()> {
    use super::login::or_user_input;
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
