use clap::Parser;
use eyre::{Result, bail};

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
        run(settings, self.current_password, self.new_password).await
    }
}

pub async fn run(
    settings: &Settings,
    current_password: Option<String>,
    new_password: Option<String>,
) -> Result<()> {
    if let Some(endpoint) = settings.active_hub_endpoint() {
        match settings.hub_session_token().await {
            Ok(_) => {
                println!("You are authenticated with Atuin Hub.");
                println!("Modify your password on Atuin Hub: {endpoint}/settings/account");
                return Ok(());
            }
            Err(_) => {
                println!("You are not currently logged in to Atuin Hub.");
                println!(
                    "Run 'atuin login' to log in to Atuin Hub, or visit {endpoint}/settings/account to change your password."
                );
                return Ok(());
            }
        }
    }

    let client = api_client::Client::new(
        &settings.sync_address,
        settings.sync_auth_token().await?,
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
