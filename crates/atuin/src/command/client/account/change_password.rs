use clap::Parser;
use eyre::{bail, Result};

use atuin_client::{api_client, settings::Settings};
use rpassword::prompt_password;

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub current_password: Option<String>,

    #[clap(long, short)]
    pub new_password: Option<String>,
}

impl Cmd {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        run(settings, &self.current_password, &self.new_password).await
    }
}

pub async fn run(
    settings: &Settings,
    current_password: &Option<String>,
    new_password: &Option<String>,
) -> Result<()> {
    let client = api_client::Client::new(
        &settings.sync_address,
        settings.session_token()?.as_str(),
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    let current_password = current_password.clone().unwrap_or_else(|| {
        prompt_password("Please enter the current password: ").expect("Failed to read from input")
    });

    if current_password.is_empty() {
        bail!("please provide the current password");
    }

    let new_password = new_password.clone().unwrap_or_else(|| {
        prompt_password("Please enter the new password: ").expect("Failed to read from input")
    });

    if new_password.is_empty() {
        bail!("please provide a new password");
    }

    client
        .change_password(current_password, new_password)
        .await?;

    println!("Account password successfully changed!");

    Ok(())
}
