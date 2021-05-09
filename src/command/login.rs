use std::fs::File;
use std::io::prelude::*;

use eyre::Result;
use structopt::StructOpt;

use atuin_client::api_client;
use atuin_client::settings::Settings;

#[derive(StructOpt)]
#[structopt(setting(structopt::clap::AppSettings::DeriveDisplayOrder))]
pub struct Cmd {
    #[structopt(long, short)]
    pub username: String,

    #[structopt(long, short)]
    pub password: String,

    #[structopt(long, short, about = "the encryption key for your account")]
    pub key: String,
}

impl Cmd {
    pub fn run(&self, settings: &Settings) -> Result<()> {
        let session_path = atuin_common::utils::data_dir().join("session");

        if session_path.exists() {
            println!(
                "You are already logged in! Please run 'atuin logout' if you wish to login again"
            );

            return Ok(());
        }

        let session = api_client::login(
            settings.sync_address.as_str(),
            self.username.as_str(),
            self.password.as_str(),
        )?;

        let session_path = settings.session_path.as_str();
        let mut file = File::create(session_path)?;
        file.write_all(session.session.as_bytes())?;

        let key_path = settings.key_path.as_str();
        let mut file = File::create(key_path)?;
        file.write_all(self.key.as_bytes())?;

        println!("Logged in!");

        Ok(())
    }
}
