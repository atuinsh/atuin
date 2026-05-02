use atuin_client::{
    auth::{self, MutateResponse},
    settings::Settings,
};
use clap::Parser;
use eyre::{Result, bail};

use super::login::{or_user_input, read_user_password};

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub password: Option<String>,

    /// The two-factor authentication code for your account, if any
    #[clap(long, short)]
    pub totp_code: Option<String>,
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        if !settings.logged_in().await? {
            bail!("You are not logged in");
        }

        let client = auth::auth_client(settings).await;

        let password = self.password.clone().unwrap_or_else(read_user_password);

        if password.is_empty() {
            bail!("please provide your password");
        }

        let mut totp_code = self.totp_code.clone();

        loop {
            let response = client
                .delete_account(&password, totp_code.as_deref())
                .await?;

            match response {
                MutateResponse::Success => break,
                MutateResponse::TwoFactorRequired => {
                    totp_code = Some(or_user_input(None, "two-factor code"));
                }
            }
        }

        // Clean up sessions from meta store
        let meta = Settings::meta_store().await?;
        meta.delete_session().await?;
        meta.delete_hub_session().await?;

        println!("Your account is deleted");

        Ok(())
    }
}
