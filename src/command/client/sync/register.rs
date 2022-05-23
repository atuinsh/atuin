use clap::{AppSettings, Parser};
use eyre::Result;
use tokio::{fs::File, io::AsyncWriteExt};

use atuin_client::{api_client, settings::Settings};

#[derive(Parser)]
#[clap(setting(AppSettings::DeriveDisplayOrder))]
pub struct Cmd {}

impl Cmd {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        run(settings).await
    }
}

pub async fn run(settings: &Settings) -> Result<()> {
    use super::login::read_user_input;
    let username = read_user_input("username");
    let email = read_user_input("email");
    let password = super::login::read_user_password();

    let session =
        api_client::register(settings.sync_address.as_str(), &username, &email, &password).await?;

    let path = settings.session_path.as_str();
    let mut file = File::create(path).await?;
    file.write_all(session.session.as_bytes()).await?;

    // Create a new key, and save it to disk
    let _key = atuin_client::encryption::new_key(settings)?;

    Ok(())
}
