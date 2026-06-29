use clap::Parser;
use eyre::{Result, bail};

use atuin_client::{
    auth::{self, MutateResponse},
    settings::Settings,
};
use rpassword::prompt_password;

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub current_password: Option<String>,

    #[clap(long, short)]
    pub new_password: Option<String>,

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

        let current_password = self.current_password.clone().unwrap_or_else(|| {
            prompt_password("Please enter the current password: ")
                .expect("Failed to read from input")
        });

        if current_password.is_empty() {
            bail!("please provide the current password");
        }

        let new_password = self.new_password.clone().unwrap_or_else(|| {
            prompt_password("Please enter the new password: ").expect("Failed to read from input")
        });

        if new_password.is_empty() {
            bail!("please provide a new password");
        }

        let mut totp_code = self.totp_code.clone();

        loop {
            let response = client
                .change_password(&current_password, &new_password, totp_code.as_deref())
                .await?;

            match response {
                MutateResponse::Success => break,
                MutateResponse::TwoFactorRequired => {
                    totp_code = Some(super::login::or_user_input(None, "two-factor code"));
                }
            }
        }

        println!("Account password successfully changed!");

        Ok(())
    }
}
