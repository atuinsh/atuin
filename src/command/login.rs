use std::borrow::Cow;
use std::io;

use atuin_common::api::LoginRequest;
use eyre::Result;
use structopt::StructOpt;
use tokio::{fs::File, io::AsyncWriteExt};

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
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        let session_path = atuin_common::utils::data_dir().join("session");

        if session_path.exists() {
            println!(
                "You are already logged in! Please run 'atuin logout' if you wish to login again"
            );

            return Ok(());
        }

        let username = or_user_input(&self.username, "username");
        let password = or_user_input(&self.password, "password");
        let key = or_user_input(&self.key, "encryption key");

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

pub(super) fn or_user_input<'a>(value: &'a Option<String>, name: &'static str) -> Cow<'a, str> {
    value
        .as_deref()
        .map_or_else(|| Cow::Owned(read_user_input(name)), Cow::Borrowed)
}

fn read_user_input(name: &'static str) -> String {
    eprint!("Please enter {}: ", name);
    get_input().expect("Failed to read from input")
}
