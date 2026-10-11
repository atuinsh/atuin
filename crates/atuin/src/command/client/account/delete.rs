use atuin_client::auth::{self, AuthClient, MutateResponse};
use atuin_client::settings::Settings;
use clap::Parser;
use clap_couture::{Couture, CoutureParser as _};
use eyre::{Result, bail};
use secrecy::{ExposeSecret, SecretString};

use crate::Atuin;
use crate::i18n::fl;

#[derive(Parser, Debug, Couture)]
pub struct Cmd {
    #[clap(long, short)]
    #[couture(prompt = fl!("prompt-password"), secret(allow_argv), stdin(dash))]
    pub password: SecretString,

    #[clap(long, short, help = fl!("arg-totp-code"))]
    pub totp_code: Option<SecretString>,
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        if !settings.logged_in().await? {
            bail!(fl!("account-not-logged-in"));
        }

        let client = auth::auth_client(settings).await;

        let password = &self.password;

        if password.expose_secret().is_empty() {
            bail!(fl!("delete-provide-password"));
        }

        let mut totp_code = self.totp_code.clone();

        loop {
            let response = client.delete_account(password, totp_code.as_ref()).await?;

            match response {
                MutateResponse::Success => break,
                MutateResponse::TwoFactorRequired => {
                    let code = Atuin::prompter_unwrap().text(&fl!("prompt-two-factor-code"))?;
                    totp_code = Some(code.into());
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
