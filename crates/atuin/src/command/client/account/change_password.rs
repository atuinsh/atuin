use atuin_client::auth::{self, AuthClient, MutateResponse};
use atuin_client::settings::Settings;
use atuin_common::userio;
use atuin_common::userio::{Prompt, Secret};
use clap::Parser;
use eyre::{Result, bail};
use secrecy::{ExposeSecret, SecretString};

use crate::i18n::fl;

#[derive(Parser, Debug)]
pub struct Cmd {
    #[clap(long, short)]
    pub current_password: Option<SecretString>,

    #[clap(long, short)]
    pub new_password: Option<SecretString>,

    #[clap(long, short, help = fl!("arg-totp-code"))]
    pub totp_code: Option<SecretString>,
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        if !settings.logged_in().await? {
            bail!(fl!("account-not-logged-in"));
        }

        let client = auth::auth_client(settings).await;

        let current_password = match self.current_password.clone() {
            Some(password) => password,
            None => Secret::new(&fl!("prompt-current-password")).ask()?.unwrap_or_default(),
        };

        if current_password.expose_secret().is_empty() {
            bail!(fl!("change-password-provide-current"));
        }

        let new_password = match self.new_password.clone() {
            Some(password) => password,
            None => Secret::new(&fl!("prompt-new-password")).ask()?.unwrap_or_default(),
        };

        if new_password.expose_secret().is_empty() {
            bail!(fl!("change-password-provide-new"));
        }

        let mut totp_code = self.totp_code.clone();

        loop {
            let response = client
                .change_password(&current_password, &new_password, totp_code.as_ref())
                .await?;

            match response {
                MutateResponse::Success => break,
                MutateResponse::TwoFactorRequired => {
                    totp_code = Some(
                        Prompt::new(&fl!("prompt-two-factor-code")).ask()?.unwrap_or_default(),
                    );
                }
            }
        }

        userio::log::info!("{}", fl!("change-password-success"));

        Ok(())
    }
}
