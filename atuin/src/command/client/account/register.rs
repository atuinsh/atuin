use clap::Parser;
use eyre::{bail, Result};
use tokio::{fs::File, io::AsyncWriteExt};

use atuin_client::{api_client, settings::Settings};

#[derive(Parser)]
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
        run(settings, &self.username, &self.email, &self.password).await
    }
}

pub async fn run(
    settings: &Settings,
    username: &Option<String>,
    email: &Option<String>,
    password: &Option<String>,
) -> Result<()> {
    use super::login::or_user_input;
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

    let path = settings.session_path.as_str();
    let mut file = File::create(path).await?;
    file.write_all(session.session.as_bytes()).await?;

    // Create a new key, and save it to disk
    let _key = atuin_client::encryption::new_key(settings)?;

    Ok(())
}
