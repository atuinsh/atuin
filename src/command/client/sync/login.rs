use std::io::{self, Write};

use clap::{AppSettings, Parser};
use eyre::Result;

use atuin_client::{api_client, settings::Settings};
use atuin_common::api::LoginRequest;
use fs_err::File;
use rpassword::prompt_password;

#[derive(Parser)]
#[clap(setting(AppSettings::DeriveDisplayOrder))]
pub struct Cmd {
    #[clap(long, short)]
    pub username: Option<String>,

    #[clap(long, short)]
    pub password: Option<String>,

    /// The encryption key for your account
    #[clap(long, short)]
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

        let username = or_user_input(&self.username, "username");
        let key = or_user_input(&self.key, "encryption key");
        let password = self.password.clone().unwrap_or_else(read_user_password);
        let session = api_client::login(
            settings.sync_address.as_str(),
            LoginRequest { username, password },
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

pub(super) fn or_user_input(value: &'_ Option<String>, name: &'static str) -> String {
    value.clone().unwrap_or_else(|| read_user_input(name))
}

pub(super) fn read_user_password() -> String {
    let password = prompt_password("Please enter password: ");
    password.expect("Failed to read from input")
}

fn read_user_input(name: &'static str) -> String {
    eprint!("Please enter {}: ", name);
    get_input().expect("Failed to read from input")
}
