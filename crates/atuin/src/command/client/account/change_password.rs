use atuin_client::auth::{self, AuthClient, MutateResponse};
use atuin_client::settings::Settings;
use clap::Parser;
use eyre::{Result, bail};
use rpassword::prompt_password;

use crate::i18n::fl;

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub current_password: Option<String>,

    #[clap(long, short)]
    pub new_password: Option<String>,

    #[clap(long, short, help = fl!("arg-totp-code"))]
    pub totp_code: Option<String>,
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        if !settings.logged_in().await? {
            bail!(fl!("account-not-logged-in"));
        }

        let client = auth::auth_client(settings).await;

        let current_password = self.current_password.clone().unwrap_or_else(|| {
            prompt_password(format!("{}: ", fl!("prompt-current-password")))
                .expect("Failed to read from input")
        });

        if current_password.is_empty() {
            bail!(fl!("change-password-provide-current"));
        }

        let new_password = self.new_password.clone().unwrap_or_else(|| {
            prompt_password(format!("{}: ", fl!("prompt-new-password")))
                .expect("Failed to read from input")
        });

        if new_password.is_empty() {
            bail!(fl!("change-password-provide-new"));
        }

        let mut totp_code = self.totp_code.clone();

        loop {
            let response = client
                .change_password(&current_password, &new_password, totp_code.as_deref())
                .await?;

            match response {
                MutateResponse::Success => break,
                MutateResponse::TwoFactorRequired => {
                    totp_code =
                        Some(super::login::or_user_input(None, &fl!("prompt-two-factor-code")));
                }
            }
        }

        println!("{}", fl!("change-password-success"));

        Ok(())
    }
}
