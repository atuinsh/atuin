use atuin_client::auth::{self, AuthClient, MutateResponse};
use atuin_client::settings::Settings;
use clap::Parser;
use eyre::{Result, bail};
use secrecy::{ExposeSecret, SecretString};

use super::login::{read_user_input, read_user_password};
use crate::i18n::fl;

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub password: Option<SecretString>,

    #[clap(long, short, help = fl!("arg-totp-code"))]
    pub totp_code: Option<SecretString>,
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        if !settings.logged_in().await? {
            bail!(fl!("account-not-logged-in"));
        }

        let client = auth::auth_client(settings).await;

        let password = self.password.clone().unwrap_or_else(read_user_password);

        if password.expose_secret().is_empty() {
            bail!(fl!("delete-provide-password"));
        }

        let mut totp_code = self.totp_code.clone();

        loop {
            let response = client.delete_account(&password, totp_code.as_ref()).await?;

            match response {
                MutateResponse::Success => break,
                MutateResponse::TwoFactorRequired => {
                    totp_code =
                        Some(read_user_input(&fl!("prompt-two-factor-code")).unwrap_or_default());
                }
            }
        }

        // Clean up sessions from meta store
        let meta = Settings::meta_store().await?;
        meta.delete_session().await?;
        meta.delete_hub_session().await?;

        println!("{}", fl!("delete-success"));

        Ok(())
    }
}
