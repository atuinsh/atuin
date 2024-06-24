use clap::Parser;
use eyre::Result;

use atuin_client::{api_client, settings::Settings};

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub token: Option<String>,
}

impl Cmd {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        run(settings, self.token).await
    }
}

pub async fn run(settings: &Settings, token: Option<String>) -> Result<()> {
    let client = api_client::Client::new(
        &settings.sync_address,
        settings.session_token()?.as_str(),
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    let (email_sent, verified) = client.verify(token).await?;

    match (email_sent, verified) {
        (true, false) => {
            println!("Verification sent! Please check your inbox");
        }

        (false, true) => {
            println!("Your account is verified");
        }

        (false, false) => {
            println!("Your Atuin server does not have mail setup. This is not required, though your account cannot be verified. Speak to your admin.");
        }

        _ => {
            println!("Invalid email and verification status. This is a bug. Please open an issue: https://github.com/atuinsh/atuin");
        }
    }

    Ok(())
}
