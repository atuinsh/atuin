use clap::{AppSettings, Parser};
use eyre::{eyre, Result};

use atuin_client::api_client;
use atuin_client::encryption::load_encoded_key;
use atuin_client::settings::Settings;

#[derive(Parser)]
#[clap(setting(AppSettings::DeriveDisplayOrder))]
pub struct Cmd {
    /// Confirm account deletion
    #[clap(long)]
    pub delete_my_account: bool,
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        if !self.delete_my_account {
            return Err(eyre!("You need to confirm account deletion"));
        }

        let client = api_client::Client::new(
            &settings.sync_address,
            &settings.session_token,
            load_encoded_key(settings)?,
        )?;

        let response = client.delete_account().await?;

        if response.message == "account deleted" && !settings.session_path.is_empty() {
            std::fs::remove_file(&settings.session_path)?;
        }

        println!("Account deleted!");

        Ok(())
    }
}
