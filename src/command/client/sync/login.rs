use std::io;

use clap::{AppSettings, Parser};
use eyre::Result;
use tokio::{fs::File, io::AsyncWriteExt};

use atuin_client::{api_client, settings::Settings};
use atuin_common::api::LoginRequest;

#[derive(Parser)]
#[clap(setting(AppSettings::DeriveDisplayOrder))]
pub struct Cmd {}

fn get_input() -> Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        let session_path = atuin_common::utils::data_dir().join("session");

        if session_path.exists() {
            println!(
                "You are already logged in! Please run 'atuin logout' if you wish to login again"
            );

            return Ok(());
        }

        let username = read_user_input("username");
        let password = read_user_password();
        let key = read_user_input("encryption key");

        let session = api_client::login(
            settings.sync_address.as_str(),
            LoginRequest { username, password },
        )
        .await?;

        let session_path = settings.session_path.as_str();
        let mut file = File::create(session_path).await?;
        file.write_all(session.session.as_bytes()).await?;

        let key_path = settings.key_path.as_str();
        let mut file = File::create(key_path).await?;
        file.write_all(key.as_bytes()).await?;

        println!("Logged in!");

        Ok(())
    }
}

pub(super) fn read_user_input(name: &'static str) -> String {
    eprint!("Please enter {}: ", name);
    get_input().expect("Failed to read from input")
}

pub(super) fn read_user_password() -> String {
    let password = rpassword::prompt_password("Please enter password: ");
    password.expect("Failed to read from input")
}
