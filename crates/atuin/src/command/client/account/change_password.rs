use atuin_client::auth::{self, AuthClient, MutateResponse};
use atuin_client::settings::Settings;
use clap::Parser;
use clap_couture::Couture;
use eyre::{Result, bail};
use secrecy::{ExposeSecret, SecretString};

use super::prompter;
use crate::i18n::fl;

#[derive(Parser, Debug, Couture)]
pub struct Cmd {
    #[clap(long, short)]
    #[couture(prompt = fl!("prompt-current-password"), secret(allow_argv))]
    pub current_password: SecretString,

    #[clap(long, short)]
    #[couture(prompt = fl!("prompt-new-password"), secret(allow_argv))]
    pub new_password: SecretString,

    #[clap(long, short, help = fl!("arg-totp-code"))]
    pub totp_code: Option<SecretString>,
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        if !settings.logged_in().await? {
            bail!(fl!("account-not-logged-in"));
        }

        let client = auth::auth_client(settings).await;

        let current_password = &self.current_password;

        if current_password.expose_secret().is_empty() {
            bail!(fl!("change-password-provide-current"));
        }

        let new_password = &self.new_password;

        if new_password.expose_secret().is_empty() {
            bail!(fl!("change-password-provide-new"));
        }

        let mut totp_code = self.totp_code.clone();

        loop {
            let response = client
                .change_password(current_password, new_password, totp_code.as_ref())
                .await?;

            match response {
                MutateResponse::Success => break,
                MutateResponse::TwoFactorRequired => {
                    let code = prompter("--totp-code")?.text(&fl!("prompt-two-factor-code"))?;
                    totp_code = Some(code.into());
                }
            }
        }

        println!("{}", fl!("change-password-success"));

        Ok(())
    }
}
