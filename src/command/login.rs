use std::io::prelude::*;
use std::{borrow::Cow, fs::File};
use std::io;

use atuin_common::api::LoginRequest;
use eyre::Result;
use structopt::StructOpt;

use atuin_client::api_client;
use atuin_client::settings::Settings;

#[derive(StructOpt)]
#[structopt(setting(structopt::clap::AppSettings::DeriveDisplayOrder))]
pub struct Cmd {
    #[structopt(long, short)]
    pub username: Option<String>,

    #[structopt(long, short)]
    pub password: Option<String>,

    #[structopt(long, short, about = "the encryption key for your account")]
    pub key: Option<String>,
}

fn get_input() -> Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
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


        // TODO: Maybe get rid of clone
        let username = if let Some(username) = self.username.clone() { username } else {
            eprint!("Please enter username: ");
            get_input().expect("Failed to read username from input")
        };

        let password = if let Some(password) = self.password.clone() { password } else {
            eprint!("Please enter password: ");
            get_input().expect("Failed to read email from input")
        };

        let key = if let Some(key) = self.key.clone() { key } else {
            eprint!("Please enter encryption key: ");
            get_input().expect("Failed to read password from input")
        };

        let session = api_client::login(
            settings.sync_address.as_str(),
            LoginRequest {
                username: Cow::Borrowed(&username),
                password: Cow::Borrowed(&password),
            },
        )?;

        let session_path = settings.session_path.as_str();
        let mut file = File::create(session_path)?;
        file.write_all(session.session.as_bytes())?;

        let key_path = settings.key_path.as_str();
        let mut file = File::create(key_path)?;
        file.write_all(key.as_bytes())?;

        println!("Logged in!");

        Ok(())
    }
}
